//! What sign-in keeps in the registry: who signed in with what, who may sign up, and the
//! short-lived state of sign-ins in progress (stored as hashes only).

use anyhow::Context as _;
use pando_proto::{AccountKind, Invitation, Role};

use crate::hosting::{Registry, now_ms};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS identities (
  provider TEXT NOT NULL,
  subject TEXT NOT NULL,
  login TEXT NOT NULL,
  email TEXT,
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY (provider, subject)
);
CREATE TABLE IF NOT EXISTS signup_allow (
  entry TEXT PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS login_states (
  key_hash BLOB PRIMARY KEY,
  provider TEXT NOT NULL,
  nonce TEXT NOT NULL,
  verifier TEXT NOT NULL,
  next TEXT NOT NULL,
  expires_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS email_logins (
  token_hash BLOB PRIMARY KEY,
  email TEXT NOT NULL,
  next TEXT NOT NULL,
  expires_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS device_logins (
  device_hash BLOB PRIMARY KEY,
  user_code TEXT NOT NULL UNIQUE,
  client TEXT NOT NULL,
  host TEXT NOT NULL,
  login TEXT,
  denied INTEGER NOT NULL DEFAULT 0,
  polled_at_ms INTEGER NOT NULL DEFAULT 0,
  expires_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS invitations (
  id TEXT PRIMARY KEY,
  org TEXT NOT NULL,
  email TEXT NOT NULL,
  role INTEGER NOT NULL,
  invited_by TEXT NOT NULL,
  token_hash BLOB NOT NULL UNIQUE,
  created_at_ms INTEGER NOT NULL,
  expires_at_ms INTEGER NOT NULL,
  accepted_at_ms INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS invitations_by_org ON invitations (org);
";

/// Names sign-in never hands out: they look like the console's and the site's own pages.
const RESERVED: &[&str] = &[
    "admin", "api", "login", "settings", "new", "explore", "www", "mail", "support", "help", "security", "status",
    "docs", "pando",
];

/// A login proposed from an email address: its local part, lowercased, with anything but
/// letters, digits, `.`, `_` and `-` turned into `-`, so it is also a valid organisation name.
fn proposed_login(email: &str) -> String {
    let local = email.split('@').next().unwrap_or_default().to_ascii_lowercase();
    let s: String = local
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .take(39)
        .collect();
    let s = s.trim_matches(['.', '-']);
    if s.is_empty() { "user".to_owned() } else { s.to_owned() }
}

/// Where a CLI sign-in (device flow) stands when the CLI asks.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Poll {
    Unknown,
    Expired,
    Denied,
    Pending,
    /// Asked again sooner than the interval allows.
    SlowDown,
    /// Approved by `login`; the request is gone, so this is answered once. Carries the host.
    Approved {
        login: String,
        host: String,
    },
}

fn invitation_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Invitation> {
    Ok(Invitation {
        id: r.get(0)?,
        org: r.get(1)?,
        email: r.get(2)?,
        role: r.get(3)?,
        invited_by: r.get(4)?,
        created_at_ms: r.get(5)?,
        expires_at_ms: r.get(6)?,
    })
}

const INVITATION: &str = "id, org, email, role, invited_by, created_at_ms, expires_at_ms";

impl Registry {
    /// Whether the waitlist lets `email` (lowercased) create an account: the address itself or
    /// its `@domain` is listed.
    pub fn signup_allowed(&self, email: &str) -> anyhow::Result<bool> {
        let domain = email.rsplit_once('@').map(|(_, d)| format!("@{d}")).unwrap_or_default();
        let db = self.db.lock().unwrap();
        Ok(db.query_row(
            "SELECT EXISTS (SELECT 1 FROM signup_allow WHERE entry = ?1 OR entry = ?2)",
            (email, domain),
            |r| r.get(0),
        )?)
    }

    pub fn signup_entries(&self) -> anyhow::Result<Vec<String>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached("SELECT entry FROM signup_allow ORDER BY entry")?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Adds or removes a waitlist entry (already normalised by [`super::signup_entry`]).
    pub fn set_signup_entry(&self, entry: &str, allowed: bool) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        if allowed {
            db.execute("INSERT INTO signup_allow (entry) VALUES (?1) ON CONFLICT DO NOTHING", [entry])?;
        } else {
            db.execute("DELETE FROM signup_allow WHERE entry = ?1", [entry])?;
        }
        Ok(())
    }

    /// The account a provider's subject signs in as, if it has signed in before.
    pub fn identity_login(&self, provider: &str, subject: &str) -> anyhow::Result<Option<String>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached("SELECT login FROM identities WHERE provider = ?1 AND subject = ?2")?;
        Ok(st.query_map((provider, subject), |r| r.get(0))?.next().transpose()?)
    }

    /// Creates a person's account for a first sign-in and records the identity; returns the
    /// login. The login comes from the email's local part, suffixed `-2`, `-3`… when an
    /// account, organisation or `reserved` name already has it. An identity that already
    /// exists keeps its account. `accepted_terms_at_ms` is when the person confirmed their age
    /// and accepted the terms.
    pub fn create_person(
        &self,
        provider: &str,
        subject: &str,
        email: &str,
        reserved: &[String],
        accepted_terms_at_ms: i64,
    ) -> anyhow::Result<String> {
        let base = proposed_login(email);
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let known: Option<String> = tx
            .query_row("SELECT login FROM identities WHERE provider = ?1 AND subject = ?2", (provider, subject), |r| {
                r.get(0)
            })
            .ok();
        if let Some(login) = known {
            return Ok(login);
        }
        let now = now_ms();
        let mut login = base.clone();
        for n in 2.. {
            let taken: bool = reserved.contains(&login)
                || RESERVED.contains(&login.as_str())
                || tx.query_row(
                    "SELECT EXISTS (SELECT 1 FROM accounts WHERE login = ?1)
                       OR EXISTS (SELECT 1 FROM organizations WHERE login = ?1)",
                    [&login],
                    |r| r.get(0),
                )?;
            if !taken {
                break;
            }
            anyhow::ensure!(n < 10_000, "no free login for {base}");
            login = format!("{base}-{n}");
        }
        tx.execute(
            "INSERT INTO accounts (login, display_name, kind, owner, created_at_ms, accepted_terms_at_ms)
             VALUES (?1, ?1, ?2, '', ?3, ?4)",
            (&login, AccountKind::Person as i32, now, accepted_terms_at_ms),
        )?;
        tx.execute(
            "INSERT INTO identities (provider, subject, login, email, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            (provider, subject, &login, email, now),
        )?;
        tx.commit()?;
        drop(db);
        self.invalidate();
        Ok(login)
    }

    /// Lets `provider`'s `subject` sign in as the existing account `login` (how the first
    /// administrator, made with `pandod token`, signs in from a browser). False if that
    /// identity already belongs to another account.
    pub fn link_identity(&self, login: &str, provider: &str, subject: &str) -> anyhow::Result<bool> {
        let email = (provider == "email").then_some(subject);
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO identities (provider, subject, login, email, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT DO NOTHING",
            (provider, subject, login, email, now_ms()),
        )?;
        let owner: String = db.query_row(
            "SELECT login FROM identities WHERE provider = ?1 AND subject = ?2",
            (provider, subject),
            |r| r.get(0),
        )?;
        Ok(owner == login)
    }

    pub(crate) fn put_login_state(
        &self,
        key_hash: &[u8],
        provider: &str,
        nonce: &str,
        verifier: &str,
        next: &str,
        expires_at_ms: i64,
    ) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        db.execute("DELETE FROM login_states WHERE expires_at_ms < ?1", [now_ms()])?;
        db.execute(
            "INSERT INTO login_states (key_hash, provider, nonce, verifier, next, expires_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (key_hash, provider, nonce, verifier, next, expires_at_ms),
        )
        .context("recording a sign-in")?;
        Ok(())
    }

    /// Removes and returns a sign-in in progress (nonce, PKCE verifier, next path), if it is
    /// for `provider` and has not expired. Each can be used once.
    pub(crate) fn take_login_state(
        &self,
        key_hash: &[u8],
        provider: &str,
    ) -> anyhow::Result<Option<(String, String, String)>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(
            "DELETE FROM login_states WHERE key_hash = ?1 RETURNING provider, nonce, verifier, next, expires_at_ms",
        )?;
        let row = st
            .query_map([key_hash], |r| {
                Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, i64>(4)?))
            })?
            .next()
            .transpose()?;
        Ok(row.filter(|r| r.0 == provider && r.4 > now_ms()).map(|r| (r.1, r.2, r.3)))
    }

    pub(crate) fn put_email_login(
        &self,
        token_hash: &[u8],
        email: &str,
        next: &str,
        expires_at_ms: i64,
    ) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        db.execute("DELETE FROM email_logins WHERE expires_at_ms < ?1", [now_ms()])?;
        db.execute(
            "INSERT INTO email_logins (token_hash, email, next, expires_at_ms) VALUES (?1, ?2, ?3, ?4)",
            (token_hash, email, next, expires_at_ms),
        )?;
        Ok(())
    }

    /// Removes and returns an email link's address and next path, if it has not expired.
    pub(crate) fn take_email_login(&self, token_hash: &[u8]) -> anyhow::Result<Option<(String, String)>> {
        let db = self.db.lock().unwrap();
        let mut st =
            db.prepare_cached("DELETE FROM email_logins WHERE token_hash = ?1 RETURNING email, next, expires_at_ms")?;
        let row = st
            .query_map([token_hash], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?
            .next()
            .transpose()?;
        Ok(row.filter(|r| r.2 > now_ms()).map(|r| (r.0, r.1)))
    }

    /// When a person accepted the terms at sign-up (0: never asked).
    pub fn accepted_terms_at(&self, login: &str) -> anyhow::Result<i64> {
        let db = self.db.lock().unwrap();
        Ok(db
            .query_row("SELECT accepted_terms_at_ms FROM accounts WHERE login = ?1", [login], |r| r.get(0))
            .unwrap_or(0))
    }

    // ---- CLI sign-in (device flow) ----

    /// Records a CLI sign-in request; false if the user code is already in use.
    pub(crate) fn put_device_login(
        &self,
        device_hash: &[u8],
        user_code: &str,
        client: &str,
        host: &str,
        expires_at_ms: i64,
    ) -> anyhow::Result<bool> {
        let db = self.db.lock().unwrap();
        db.execute("DELETE FROM device_logins WHERE expires_at_ms < ?1", [now_ms()])?;
        let n = db.execute(
            "INSERT INTO device_logins (device_hash, user_code, client, host, expires_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT DO NOTHING",
            (device_hash, user_code, client, host, expires_at_ms),
        )?;
        Ok(n == 1)
    }

    /// A CLI sign-in waiting for a decision: its client (user agent) and host.
    pub(crate) fn device_login(&self, user_code: &str) -> anyhow::Result<Option<(String, String)>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(
            "SELECT client, host FROM device_logins
             WHERE user_code = ?1 AND login IS NULL AND denied = 0 AND expires_at_ms > ?2",
        )?;
        Ok(st.query_map((user_code, now_ms()), |r| Ok((r.get(0)?, r.get(1)?)))?.next().transpose()?)
    }

    /// Approves (as `login`) or denies a waiting CLI sign-in; false if it isn't waiting.
    pub(crate) fn decide_device_login(&self, user_code: &str, login: Option<&str>) -> anyhow::Result<bool> {
        let db = self.db.lock().unwrap();
        let n = db.execute(
            "UPDATE device_logins SET login = ?2, denied = ?3
             WHERE user_code = ?1 AND login IS NULL AND denied = 0 AND expires_at_ms > ?4",
            (user_code, login, login.is_none() as i64, now_ms()),
        )?;
        Ok(n == 1)
    }

    /// The CLI asking whether its sign-in was approved; asking sooner than `interval_ms` after the
    /// last time is answered `SlowDown`. Finished requests are removed.
    pub(crate) fn poll_device_login(&self, device_hash: &[u8], interval_ms: i64) -> anyhow::Result<Poll> {
        let db = self.db.lock().unwrap();
        let row = db
            .query_row(
                "SELECT login, denied, polled_at_ms, expires_at_ms, host FROM device_logins WHERE device_hash = ?1",
                [device_hash],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, i64>(1)? != 0,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )
            .ok();
        let Some((login, denied, polled, expires, host)) = row else { return Ok(Poll::Unknown) };
        let now = now_ms();
        let done = |p: Poll| -> anyhow::Result<Poll> {
            db.execute("DELETE FROM device_logins WHERE device_hash = ?1", [device_hash])?;
            Ok(p)
        };
        if expires <= now {
            return done(Poll::Expired);
        }
        if denied {
            return done(Poll::Denied);
        }
        if let Some(login) = login {
            return done(Poll::Approved { login, host });
        }
        db.execute("UPDATE device_logins SET polled_at_ms = ?1 WHERE device_hash = ?2", (now, device_hash))?;
        Ok(if now - polled < interval_ms { Poll::SlowDown } else { Poll::Pending })
    }

    // ---- invitations ----

    pub(crate) fn create_invitation(&self, inv: &Invitation, token_hash: &[u8]) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO invitations (id, org, email, role, invited_by, token_hash, created_at_ms, expires_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            (
                &inv.id,
                &inv.org,
                &inv.email,
                inv.role,
                &inv.invited_by,
                token_hash,
                inv.created_at_ms,
                inv.expires_at_ms,
            ),
        )?;
        Ok(())
    }

    /// An organisation's invitations not yet accepted, newest first (expired ones included).
    pub(crate) fn invitations(&self, org: &str) -> anyhow::Result<Vec<Invitation>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(&format!(
            "SELECT {INVITATION} FROM invitations WHERE org = ?1 AND accepted_at_ms = 0 ORDER BY created_at_ms DESC"
        ))?;
        let rows = st.query_map([org], invitation_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Withdraws an invitation not yet accepted; false if there is none.
    pub(crate) fn revoke_invitation(&self, org: &str, id: &str) -> anyhow::Result<bool> {
        let db = self.db.lock().unwrap();
        let n = db.execute("DELETE FROM invitations WHERE org = ?1 AND id = ?2 AND accepted_at_ms = 0", (org, id))?;
        Ok(n == 1)
    }

    /// The invitation a link names, if it can still be accepted.
    pub(crate) fn invitation(&self, token_hash: &[u8]) -> anyhow::Result<Option<Invitation>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(&format!(
            "SELECT {INVITATION} FROM invitations WHERE token_hash = ?1 AND accepted_at_ms = 0 AND expires_at_ms > ?2"
        ))?;
        Ok(st.query_map((token_hash, now_ms()), invitation_row)?.next().transpose()?)
    }

    /// Marks an invitation accepted and returns it, if it could still be: each works once.
    pub(crate) fn take_invitation(&self, token_hash: &[u8]) -> anyhow::Result<Option<Invitation>> {
        let db = self.db.lock().unwrap();
        let now = now_ms();
        let mut st = db.prepare_cached(&format!(
            "UPDATE invitations SET accepted_at_ms = ?2
             WHERE token_hash = ?1 AND accepted_at_ms = 0 AND expires_at_ms > ?2 RETURNING {INVITATION}"
        ))?;
        Ok(st.query_map((token_hash, now), invitation_row)?.next().transpose()?)
    }

    /// Puts back an invitation taken for a join that then failed.
    pub(crate) fn untake_invitation(&self, token_hash: &[u8]) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        db.execute("UPDATE invitations SET accepted_at_ms = 0 WHERE token_hash = ?1", [token_hash])?;
        Ok(())
    }
}

/// An invitation's role as stored (read when unspecified).
pub(crate) fn invited_role(r: i32) -> Role {
    match Role::try_from(r).unwrap_or(Role::Unspecified) {
        Role::Unspecified => Role::Read,
        r => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logins_are_proposed_from_the_address_and_suffixed_when_taken() {
        assert_eq!(proposed_login("Kabir.Narang+x@example.com"), "kabir.narang-x");
        assert_eq!(proposed_login(".-@example.com"), "user");
        let r = Registry::open("memory").unwrap();
        r.ensure_account("mira", "").unwrap();
        let org = crate::directory::OrgRec {
            login: "mira-2".into(),
            display_name: String::new(),
            description: String::new(),
            created_at_ms: 0,
        };
        r.create_organization(&org).unwrap();
        assert_eq!(r.create_person("google", "1", "mira@a.example", &[], 1).unwrap(), "mira-3");
        assert_eq!(r.create_person("google", "1", "other@a.example", &[], 1).unwrap(), "mira-3", "same identity");
        assert_eq!(
            r.create_person("email", "root@a.example", "root@a.example", &["root".into()], 1).unwrap(),
            "root-2"
        );
        assert_eq!(r.identity_login("google", "1").unwrap().as_deref(), Some("mira-3"));
        assert_eq!(
            r.create_person("email", "a@x.example", "admin@x.example", &[], 1).unwrap(),
            "admin-2",
            "route-like names"
        );
        assert!(r.link_identity("mira", "email", "mira@b.example").unwrap());
        assert!(r.link_identity("mira", "email", "mira@b.example").unwrap(), "again: no change");
        assert!(!r.link_identity("mira-3", "email", "mira@b.example").unwrap(), "belongs to someone else");
    }

    #[test]
    fn sign_ins_in_progress_are_single_use_and_expire() {
        let r = Registry::open("memory").unwrap();
        let later = now_ms() + 60_000;
        let (n, v) = (super::super::random_hex().unwrap(), super::super::random_hex().unwrap());
        r.put_login_state(b"k1", "google", &n, &v, "/", later).unwrap();
        assert!(r.take_login_state(b"k1", "other").unwrap().is_none(), "for another provider");
        r.put_login_state(b"k2", "google", &n, &v, "/x", later).unwrap();
        assert_eq!(r.take_login_state(b"k2", "google").unwrap(), Some((n.clone(), v.clone(), "/x".into())));
        assert!(r.take_login_state(b"k2", "google").unwrap().is_none(), "used");
        r.put_login_state(b"k3", "google", &n, &v, "/", now_ms() - 1).unwrap();
        assert!(r.take_login_state(b"k3", "google").unwrap().is_none(), "expired");

        r.put_email_login(b"t1", "a@b.example", "/", later).unwrap();
        assert_eq!(r.take_email_login(b"t1").unwrap(), Some(("a@b.example".into(), "/".into())));
        assert!(r.take_email_login(b"t1").unwrap().is_none(), "used");
        r.put_email_login(b"t2", "a@b.example", "/", now_ms() - 1).unwrap();
        assert!(r.take_email_login(b"t2").unwrap().is_none(), "expired");
    }

    #[test]
    fn cli_sign_ins_and_invitations_expire_and_work_once() {
        let r = Registry::open("memory").unwrap();
        let later = now_ms() + 60_000;
        assert!(r.put_device_login(b"d1", "BCDF-GHJK", "ua", "laptop", now_ms() - 1).unwrap());
        assert_eq!(r.poll_device_login(b"d1", 4000).unwrap(), Poll::Expired);
        assert_eq!(r.poll_device_login(b"d1", 4000).unwrap(), Poll::Unknown, "gone once reported");
        assert!(r.put_device_login(b"d2", "BCDF-GHJL", "ua", "laptop", later).unwrap());
        assert!(!r.put_device_login(b"d3", "BCDF-GHJL", "ua", "laptop", later).unwrap(), "codes are unique");
        assert!(r.decide_device_login("BCDF-GHJL", Some("mira")).unwrap());
        assert!(!r.decide_device_login("BCDF-GHJL", None).unwrap(), "decided once");
        assert_eq!(
            r.poll_device_login(b"d2", 4000).unwrap(),
            Poll::Approved { login: "mira".into(), host: "laptop".into() }
        );

        let inv = |id: &str, expires_at_ms| Invitation {
            id: id.into(),
            org: "acme".into(),
            email: "a@b.example".into(),
            role: Role::Write as i32,
            invited_by: "mira".into(),
            created_at_ms: now_ms(),
            expires_at_ms,
        };
        r.create_invitation(&inv("old", now_ms() - 1), b"t-old").unwrap();
        assert!(r.invitation(b"t-old").unwrap().is_none(), "expired");
        assert!(r.take_invitation(b"t-old").unwrap().is_none(), "expired");
        assert_eq!(r.invitations("acme").unwrap().len(), 1, "listed, to show it expired");
        r.create_invitation(&inv("new", later), b"t-new").unwrap();
        assert_eq!(r.take_invitation(b"t-new").unwrap().unwrap().id, "new");
        assert!(r.take_invitation(b"t-new").unwrap().is_none(), "used");
        r.untake_invitation(b"t-new").unwrap();
        assert!(r.invitation(b"t-new").unwrap().is_some(), "put back after a failed join");
        assert!(r.revoke_invitation("acme", "new").unwrap());
        assert!(r.invitation(b"t-new").unwrap().is_none());
    }

    #[test]
    fn older_registries_gain_the_terms_column() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.db");
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE accounts (login TEXT PRIMARY KEY, display_name TEXT NOT NULL DEFAULT '',
                   kind INTEGER NOT NULL, owner TEXT NOT NULL DEFAULT '', created_at_ms INTEGER NOT NULL);
                 INSERT INTO accounts (login, kind, created_at_ms) VALUES ('old', 1, 0);",
            )
            .unwrap();
        let url = format!("sqlite://{}", path.display());
        let r = Registry::open(&url).unwrap();
        assert_eq!(r.accepted_terms_at("old").unwrap(), 0, "existing accounts are unaffected");
        assert_eq!(r.create_person("email", "ana@a.example", "ana@a.example", &[], 7).unwrap(), "ana");
        assert_eq!(r.accepted_terms_at("ana").unwrap(), 7);
        drop(r);
        Registry::open(&url).unwrap();
    }

    #[test]
    fn waitlist_matches_addresses_and_domains() {
        let r = Registry::open("memory").unwrap();
        r.set_signup_entry("mira@a.example", true).unwrap();
        r.set_signup_entry("@corp.example", true).unwrap();
        assert!(r.signup_allowed("mira@a.example").unwrap());
        assert!(r.signup_allowed("anyone@corp.example").unwrap());
        assert!(!r.signup_allowed("eve@a.example").unwrap());
        assert!(!r.signup_allowed("eve@sub.corp.example").unwrap());
        r.set_signup_entry("@corp.example", false).unwrap();
        assert_eq!(r.signup_entries().unwrap(), vec!["mira@a.example".to_owned()]);
    }
}
