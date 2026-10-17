//! Change requests, reviews, comments, checks, branch protections and activity, kept in the
//! registry database next to the repositories they belong to (ADR-024).

use pando_proto::{ChangeRequestState, Check, CheckState, Comment, EventKind, Protection, Verdict};

use crate::hosting::{Registry, RepoKey};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS change_requests (
  owner TEXT NOT NULL,
  name TEXT NOT NULL,
  number INTEGER NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL DEFAULT '',
  author TEXT NOT NULL,
  state INTEGER NOT NULL,
  source TEXT NOT NULL,
  target TEXT NOT NULL,
  source_tip BLOB NOT NULL,
  base BLOB NOT NULL,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  reviewers TEXT NOT NULL DEFAULT '',
  labels TEXT NOT NULL DEFAULT '',
  landed_as BLOB,
  from_protected_land INTEGER NOT NULL DEFAULT 0,
  files INTEGER NOT NULL DEFAULT 0,
  additions INTEGER NOT NULL DEFAULT 0,
  deletions INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (owner, name, number)
);
CREATE INDEX IF NOT EXISTS change_requests_open ON change_requests (state, owner, name);
CREATE TABLE IF NOT EXISTS reviews (
  owner TEXT NOT NULL,
  name TEXT NOT NULL,
  number INTEGER NOT NULL,
  reviewer TEXT NOT NULL,
  verdict INTEGER NOT NULL,
  at_ms INTEGER NOT NULL,
  reviewed_tip BLOB NOT NULL,
  PRIMARY KEY (owner, name, number, reviewer)
);
CREATE TABLE IF NOT EXISTS comments (
  id TEXT PRIMARY KEY,
  owner TEXT NOT NULL,
  name TEXT NOT NULL,
  number INTEGER NOT NULL,
  author TEXT NOT NULL,
  body TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  edited_at_ms INTEGER NOT NULL DEFAULT 0,
  path TEXT NOT NULL DEFAULT '',
  line INTEGER NOT NULL DEFAULT 0,
  end_line INTEGER NOT NULL DEFAULT 0,
  on_base INTEGER NOT NULL DEFAULT 0,
  at_tip BLOB NOT NULL,
  thread TEXT NOT NULL,
  resolved INTEGER NOT NULL DEFAULT 0,
  suggestion TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS comments_by_change_request ON comments (owner, name, number);
CREATE TABLE IF NOT EXISTS checks (
  owner TEXT NOT NULL,
  name TEXT NOT NULL,
  tip BLOB NOT NULL,
  check_name TEXT NOT NULL,
  state INTEGER NOT NULL,
  summary TEXT NOT NULL DEFAULT '',
  details_url TEXT NOT NULL DEFAULT '',
  started_at_ms INTEGER NOT NULL DEFAULT 0,
  finished_at_ms INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (owner, name, tip, check_name)
);
CREATE TABLE IF NOT EXISTS protections (
  owner TEXT NOT NULL,
  name TEXT NOT NULL,
  pattern TEXT NOT NULL,
  required_approvals INTEGER NOT NULL,
  required_checks TEXT NOT NULL DEFAULT '',
  dismiss_stale INTEGER NOT NULL,
  agents_land_directly INTEGER NOT NULL,
  bypass TEXT NOT NULL DEFAULT '',
  PRIMARY KEY (owner, name, pattern)
);
CREATE TABLE IF NOT EXISTS events (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  kind INTEGER NOT NULL,
  actor TEXT NOT NULL,
  owner TEXT NOT NULL,
  name TEXT NOT NULL,
  at_ms INTEGER NOT NULL,
  change_request INTEGER NOT NULL DEFAULT 0,
  changeset BLOB,
  summary TEXT NOT NULL DEFAULT ''
);
";

/// A change request as stored; reviews, checks and blockers are added when it is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrRec {
    pub key: RepoKey,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub author: String,
    pub state: ChangeRequestState,
    pub source: String,
    pub target: String,
    pub source_tip: Vec<u8>,
    pub base: Vec<u8>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub reviewers: Vec<String>,
    pub labels: Vec<String>,
    pub landed_as: Vec<u8>,
    pub from_protected_land: bool,
    pub files: u32,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRec {
    pub reviewer: String,
    pub verdict: Verdict,
    pub at_ms: i64,
    pub reviewed_tip: Vec<u8>,
}

fn list(s: String) -> Vec<String> {
    s.split(',').filter(|t| !t.is_empty()).map(str::to_owned).collect()
}

/// Whether a protection pattern (`main`, `release/*`, `*`) covers a branch.
pub fn pattern_matches(pattern: &str, branch: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => branch.starts_with(prefix),
        None => pattern == branch,
    }
}

const CR_COLUMNS: &str = "owner, name, number, title, body, author, state, source, target, source_tip, base, created_at_ms, \
     updated_at_ms, reviewers, labels, landed_as, from_protected_land, files, additions, deletions";

fn cr_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<CrRec> {
    Ok(CrRec {
        key: RepoKey { owner: r.get(0)?, name: r.get(1)? },
        number: r.get::<_, i64>(2)? as u64,
        title: r.get(3)?,
        body: r.get(4)?,
        author: r.get(5)?,
        state: ChangeRequestState::try_from(r.get::<_, i32>(6)?).unwrap_or(ChangeRequestState::Open),
        source: r.get(7)?,
        target: r.get(8)?,
        source_tip: r.get(9)?,
        base: r.get(10)?,
        created_at_ms: r.get(11)?,
        updated_at_ms: r.get(12)?,
        reviewers: list(r.get(13)?),
        labels: list(r.get(14)?),
        landed_as: r.get::<_, Option<Vec<u8>>>(15)?.unwrap_or_default(),
        from_protected_land: r.get::<_, i64>(16)? != 0,
        files: r.get(17)?,
        additions: r.get(18)?,
        deletions: r.get(19)?,
    })
}

impl Registry {
    /// Records a new change request and returns its number (the next in its repository).
    pub fn create_change_request(&self, cr: &CrRec) -> anyhow::Result<u64> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let next = Self::next_number(&tx, &cr.key)?;
        tx.execute(
            &format!("INSERT INTO change_requests ({CR_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, NULL, ?16, ?17, ?18, ?19)"),
            rusqlite::params![
                cr.key.owner,
                cr.key.name,
                next,
                cr.title,
                cr.body,
                cr.author,
                cr.state as i32,
                cr.source,
                cr.target,
                cr.source_tip,
                cr.base,
                cr.created_at_ms,
                cr.updated_at_ms,
                cr.reviewers.join(","),
                cr.labels.join(","),
                cr.from_protected_land as i64,
                cr.files,
                cr.additions,
                cr.deletions,
            ],
        )?;
        tx.commit()?;
        Ok(next as u64)
    }

    /// Writes back everything but the repository and number.
    pub fn update_change_request(&self, cr: &CrRec) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "UPDATE change_requests SET title = ?4, body = ?5, state = ?6, source_tip = ?7, base = ?8, updated_at_ms = ?9,
               reviewers = ?10, labels = ?11, landed_as = ?12, files = ?13, additions = ?14, deletions = ?15
             WHERE owner = ?1 AND name = ?2 AND number = ?3",
            rusqlite::params![
                cr.key.owner,
                cr.key.name,
                cr.number as i64,
                cr.title,
                cr.body,
                cr.state as i32,
                cr.source_tip,
                cr.base,
                cr.updated_at_ms,
                cr.reviewers.join(","),
                cr.labels.join(","),
                (!cr.landed_as.is_empty()).then_some(&cr.landed_as),
                cr.files,
                cr.additions,
                cr.deletions,
            ],
        )?;
        Ok(())
    }

    pub fn change_request(&self, key: &RepoKey, number: u64) -> anyhow::Result<Option<CrRec>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(&format!(
            "SELECT {CR_COLUMNS} FROM change_requests WHERE owner = ?1 AND name = ?2 AND number = ?3"
        ))?;
        let mut rows = st.query_map((&key.owner, &key.name, number as i64), cr_row)?;
        Ok(rows.next().transpose()?)
    }

    /// Change requests of a repository (or of every repository), newest first.
    pub fn change_requests(
        &self,
        key: Option<&RepoKey>,
        states: &[ChangeRequestState],
        limit: usize,
    ) -> anyhow::Result<Vec<CrRec>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(&format!(
            "SELECT {CR_COLUMNS} FROM change_requests WHERE (?1 IS NULL OR (owner = ?1 AND name = ?2))
             ORDER BY updated_at_ms DESC"
        ))?;
        let rows = st.query_map((key.map(|k| &k.owner), key.map(|k| &k.name)), cr_row)?;
        let mut out = Vec::new();
        for r in rows {
            let r = r?;
            if states.is_empty() || states.contains(&r.state) {
                out.push(r);
                if out.len() == limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Open (and draft) change requests in a repository.
    pub fn open_change_requests(&self, key: &RepoKey) -> anyhow::Result<u32> {
        let db = self.db.lock().unwrap();
        Ok(db.query_row(
            "SELECT COUNT(*) FROM change_requests WHERE owner = ?1 AND name = ?2 AND state IN (?3, ?4)",
            rusqlite::params![key.owner, key.name, ChangeRequestState::Open as i32, ChangeRequestState::Draft as i32],
            |r| r.get(0),
        )?)
    }

    pub fn reviews(&self, key: &RepoKey, number: u64) -> anyhow::Result<Vec<ReviewRec>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(
            "SELECT reviewer, verdict, at_ms, reviewed_tip FROM reviews WHERE owner = ?1 AND name = ?2 AND number = ?3 ORDER BY at_ms",
        )?;
        let rows = st.query_map((&key.owner, &key.name, number as i64), |r| {
            Ok(ReviewRec {
                reviewer: r.get(0)?,
                verdict: Verdict::try_from(r.get::<_, i32>(1)?).unwrap_or(Verdict::Commented),
                at_ms: r.get(2)?,
                reviewed_tip: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// A reviewer's latest verdict replaces their earlier one.
    pub fn set_review(&self, key: &RepoKey, number: u64, r: &ReviewRec) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO reviews (owner, name, number, reviewer, verdict, at_ms, reviewed_tip) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (owner, name, number, reviewer) DO UPDATE SET verdict = excluded.verdict, at_ms = excluded.at_ms, reviewed_tip = excluded.reviewed_tip",
            rusqlite::params![key.owner, key.name, number as i64, r.reviewer, r.verdict as i32, r.at_ms, r.reviewed_tip],
        )?;
        Ok(())
    }

    pub fn comments(&self, key: &RepoKey, number: u64) -> anyhow::Result<Vec<Comment>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(
            "SELECT id, author, body, created_at_ms, edited_at_ms, path, line, on_base, at_tip, thread, resolved, suggestion, end_line
             FROM comments WHERE owner = ?1 AND name = ?2 AND number = ?3 ORDER BY created_at_ms, id",
        )?;
        let rows = st.query_map((&key.owner, &key.name, number as i64), |r| {
            Ok(Comment {
                id: r.get(0)?,
                author: r.get(1)?,
                body: r.get(2)?,
                created_at_ms: r.get(3)?,
                edited_at_ms: r.get(4)?,
                path: r.get(5)?,
                line: r.get(6)?,
                on_base: r.get::<_, i64>(7)? != 0,
                at_tip: r.get(8)?,
                thread: r.get(9)?,
                resolved: r.get::<_, i64>(10)? != 0,
                suggestion: r.get(11)?,
                end_line: r.get(12)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn add_comment(&self, key: &RepoKey, number: u64, c: &Comment) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO comments (id, owner, name, number, author, body, created_at_ms, path, line, end_line, on_base, at_tip, thread, suggestion)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            rusqlite::params![
                c.id,
                key.owner,
                key.name,
                number as i64,
                c.author,
                c.body,
                c.created_at_ms,
                c.path,
                c.line,
                c.end_line,
                c.on_base as i64,
                c.at_tip,
                c.thread,
                c.suggestion
            ],
        )?;
        Ok(())
    }

    /// Marks every comment of a thread resolved or open; false if there is no such thread.
    pub fn resolve_thread(&self, key: &RepoKey, number: u64, thread: &str, resolved: bool) -> anyhow::Result<bool> {
        let db = self.db.lock().unwrap();
        let n = db.execute(
            "UPDATE comments SET resolved = ?5 WHERE owner = ?1 AND name = ?2 AND number = ?3 AND thread = ?4",
            rusqlite::params![key.owner, key.name, number as i64, thread, resolved as i64],
        )?;
        Ok(n > 0)
    }

    /// Checks reported for one version (tip) of a change.
    pub fn checks(&self, key: &RepoKey, tip: &[u8]) -> anyhow::Result<Vec<Check>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(
            "SELECT check_name, state, summary, details_url, started_at_ms, finished_at_ms FROM checks
             WHERE owner = ?1 AND name = ?2 AND tip = ?3 ORDER BY check_name",
        )?;
        let rows = st.query_map(rusqlite::params![key.owner, key.name, tip], |r| {
            Ok(Check {
                name: r.get(0)?,
                state: r.get(1)?,
                summary: r.get(2)?,
                details_url: r.get(3)?,
                started_at_ms: r.get(4)?,
                finished_at_ms: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn set_check(&self, key: &RepoKey, tip: &[u8], c: &Check) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO checks (owner, name, tip, check_name, state, summary, details_url, started_at_ms, finished_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT (owner, name, tip, check_name) DO UPDATE SET state = excluded.state, summary = excluded.summary,
               details_url = excluded.details_url, started_at_ms = excluded.started_at_ms, finished_at_ms = excluded.finished_at_ms",
            rusqlite::params![key.owner, key.name, tip, c.name, c.state, c.summary, c.details_url, c.started_at_ms, c.finished_at_ms],
        )?;
        Ok(())
    }

    pub fn protections(&self, key: &RepoKey) -> anyhow::Result<Vec<Protection>> {
        self.cached(format!("protections:{key}"), || self.protections_uncached(key))
    }

    fn protections_uncached(&self, key: &RepoKey) -> anyhow::Result<Vec<Protection>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(
            "SELECT pattern, required_approvals, required_checks, dismiss_stale, agents_land_directly, bypass
             FROM protections WHERE owner = ?1 AND name = ?2 ORDER BY pattern",
        )?;
        let rows = st.query_map((&key.owner, &key.name), |r| {
            Ok(Protection {
                repository: Some(pando_proto::RepositoryRef { owner: key.owner.clone(), name: key.name.clone() }),
                pattern: r.get(0)?,
                required_approvals: r.get(1)?,
                required_checks: list(r.get(2)?),
                dismiss_stale: r.get::<_, i64>(3)? != 0,
                agents_land_directly: r.get::<_, i64>(4)? != 0,
                bypass: list(r.get(5)?),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The protection covering `branch`: an exact pattern wins over a wildcard, a longer
    /// wildcard over a shorter one.
    pub fn protection_for(&self, key: &RepoKey, branch: &str) -> anyhow::Result<Option<Protection>> {
        let mut matching: Vec<Protection> =
            self.protections(key)?.into_iter().filter(|p| pattern_matches(&p.pattern, branch)).collect();
        matching.sort_by_key(|p| (!p.pattern.ends_with('*'), p.pattern.len()));
        Ok(matching.pop())
    }

    pub fn set_protection(&self, key: &RepoKey, p: &Protection) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO protections (owner, name, pattern, required_approvals, required_checks, dismiss_stale, agents_land_directly, bypass)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (owner, name, pattern) DO UPDATE SET required_approvals = excluded.required_approvals,
               required_checks = excluded.required_checks, dismiss_stale = excluded.dismiss_stale,
               agents_land_directly = excluded.agents_land_directly, bypass = excluded.bypass",
            rusqlite::params![
                key.owner,
                key.name,
                p.pattern,
                p.required_approvals,
                p.required_checks.join(","),
                p.dismiss_stale as i64,
                p.agents_land_directly as i64,
                p.bypass.join(","),
            ],
        )?;
        self.invalidate();
        Ok(())
    }

    pub fn delete_protection(&self, key: &RepoKey, pattern: &str) -> anyhow::Result<bool> {
        let db = self.db.lock().unwrap();
        let n = db.execute(
            "DELETE FROM protections WHERE owner = ?1 AND name = ?2 AND pattern = ?3",
            (&key.owner, &key.name, pattern),
        )?;
        self.invalidate();
        Ok(n == 1)
    }

    pub fn record_event(
        &self,
        kind: EventKind,
        actor: &str,
        key: &RepoKey,
        change_request: u64,
        changeset: &[u8],
        summary: &str,
    ) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO events (kind, actor, owner, name, at_ms, change_request, changeset, summary) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                kind as i32,
                actor,
                key.owner,
                key.name,
                crate::hosting::now_ms(),
                change_request as i64,
                (!changeset.is_empty()).then_some(changeset),
                summary
            ],
        )?;
        Ok(())
    }

    /// Events newest first, before sequence number `before` (0: from the newest).
    pub fn events(
        &self,
        key: Option<&RepoKey>,
        actor: &str,
        before: i64,
        limit: usize,
    ) -> anyhow::Result<Vec<(i64, pando_proto::Event)>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(
            "SELECT seq, kind, actor, owner, name, at_ms, change_request, changeset, summary FROM events
             WHERE (?1 IS NULL OR (owner = ?1 AND name = ?2)) AND (?3 = '' OR actor = ?3) AND (?4 = 0 OR seq < ?4)
             ORDER BY seq DESC LIMIT ?5",
        )?;
        let rows = st.query_map(
            rusqlite::params![key.map(|k| &k.owner), key.map(|k| &k.name), actor, before, limit as i64],
            |r| {
                let seq: i64 = r.get(0)?;
                Ok((
                    seq,
                    pando_proto::Event {
                        id: seq.to_string(),
                        kind: r.get(1)?,
                        actor: r.get(2)?,
                        repository: Some(pando_proto::RepositoryRef { owner: r.get(3)?, name: r.get(4)? }),
                        at_ms: r.get(5)?,
                        change_request: r.get::<_, i64>(6)? as u64,
                        changeset: r.get::<_, Option<Vec<u8>>>(7)?.unwrap_or_default(),
                        summary: r.get(8)?,
                    },
                ))
            },
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

pub fn check_state(c: &Check) -> CheckState {
    CheckState::try_from(c.state).unwrap_or(CheckState::Pending)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protection_patterns() {
        assert!(pattern_matches("main", "main"));
        assert!(!pattern_matches("main", "main2"));
        assert!(pattern_matches("release/*", "release/2.0"));
        assert!(!pattern_matches("release/*", "releases"));
        assert!(pattern_matches("*", "anything"));
        let reg = Registry::open("memory").unwrap();
        let key = RepoKey::parse("lumen/fleet").unwrap();
        let p = |pattern: &str, n| Protection { pattern: pattern.into(), required_approvals: n, ..Default::default() };
        reg.set_protection(&key, &p("*", 1)).unwrap();
        reg.set_protection(&key, &p("release/*", 2)).unwrap();
        reg.set_protection(&key, &p("release/1.0", 3)).unwrap();
        assert_eq!(reg.protection_for(&key, "release/1.0").unwrap().unwrap().required_approvals, 3, "exact wins");
        assert_eq!(
            reg.protection_for(&key, "release/2.0").unwrap().unwrap().required_approvals,
            2,
            "longer wildcard wins"
        );
        assert_eq!(reg.protection_for(&key, "main").unwrap().unwrap().required_approvals, 1);
        assert!(reg.delete_protection(&key, "*").unwrap());
        assert!(reg.protection_for(&key, "main").unwrap().is_none());
    }

    #[test]
    fn numbers_count_up_per_repository() {
        let reg = Registry::open("memory").unwrap();
        let cr = |repo: &str| CrRec {
            key: RepoKey::parse(repo).unwrap(),
            number: 0,
            title: "t".into(),
            body: String::new(),
            author: "kabir".into(),
            state: ChangeRequestState::Open,
            source: "feature".into(),
            target: "main".into(),
            source_tip: vec![1],
            base: vec![0],
            created_at_ms: 1,
            updated_at_ms: 1,
            reviewers: vec!["mira".into()],
            labels: vec![],
            landed_as: vec![],
            from_protected_land: false,
            files: 1,
            additions: 2,
            deletions: 0,
        };
        assert_eq!(reg.create_change_request(&cr("lumen/fleet")).unwrap(), 1);
        assert_eq!(reg.create_change_request(&cr("lumen/fleet")).unwrap(), 2);
        assert_eq!(reg.create_change_request(&cr("lumen/web")).unwrap(), 1);
        let got = reg.change_request(&RepoKey::parse("lumen/fleet").unwrap(), 2).unwrap().unwrap();
        assert_eq!(got.reviewers, vec!["mira".to_string()]);
        assert_eq!(reg.change_requests(None, &[ChangeRequestState::Open], 10).unwrap().len(), 3);
    }
}
