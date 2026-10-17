//! Webhooks (ADR-025): a repository calls URLs when something happens in it, so CI, chat and
//! trackers can react. Events go into a delivery queue in the registry and a worker posts them,
//! signed like GitHub's (`X-Pando-Signature-256: sha256=HMAC`), retrying with backoff.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use hmac::{Hmac, KeyInit, Mac};
use pando_core::{Id, Result as CoreResult};
use pando_proto::{Webhook, WebhookDelivery};
use serde_json::{Value, json};
use sha2::Sha256;

use crate::hosting::{Registry, RepoKey, now_ms};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS webhooks (
  id TEXT PRIMARY KEY,
  owner TEXT NOT NULL,
  name TEXT NOT NULL,
  url TEXT NOT NULL,
  secret TEXT NOT NULL,
  events TEXT NOT NULL DEFAULT '',
  active INTEGER NOT NULL DEFAULT 1,
  created_at_ms INTEGER NOT NULL,
  created_by TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS webhooks_by_repository ON webhooks (owner, name);
CREATE TABLE IF NOT EXISTS deliveries (
  id TEXT PRIMARY KEY,
  hook TEXT NOT NULL,
  event TEXT NOT NULL,
  payload TEXT NOT NULL,
  status INTEGER NOT NULL DEFAULT 0,
  error TEXT NOT NULL DEFAULT '',
  attempts INTEGER NOT NULL DEFAULT 0,
  delivered INTEGER NOT NULL DEFAULT 0,
  next_at_ms INTEGER NOT NULL,
  created_at_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS deliveries_due ON deliveries (delivered, next_at_ms);
CREATE INDEX IF NOT EXISTS deliveries_by_hook ON deliveries (hook, created_at_ms);
";

/// The events a webhook can ask for.
pub const EVENTS: &[&str] = &["push", "change_request", "review", "comment", "check", "issue"];

/// Attempts before a delivery is given up; the gaps grow from 10 s to about 20 minutes.
const MAX_ATTEMPTS: i64 = 8;

/// Deliveries kept per webhook, for the log.
const KEEP: i64 = 200;

fn random_hex(n: usize) -> anyhow::Result<String> {
    let mut raw = vec![0u8; n];
    getrandom::fill(&mut raw).context("reading random bytes")?;
    Ok(hex::encode(raw))
}

/// `sha256=` and the hex HMAC-SHA256 of `body` under `secret`, as GitHub sends it.
pub fn signature(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// A URL a webhook may call: http(s), with a host.
pub fn check_url(url: &str) -> Result<reqwest::Url, String> {
    let u = reqwest::Url::parse(url).map_err(|e| format!("`{url}` is not a URL: {e}"))?;
    if !matches!(u.scheme(), "https" | "http") || u.host_str().is_none_or(str::is_empty) {
        return Err("a webhook calls an http:// or https:// URL".into());
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err("put credentials in the secret, not the URL".into());
    }
    Ok(u)
}

/// Addresses a webhook never calls: link-local (cloud metadata services live there),
/// unspecified and multicast. Private networks are allowed: self-hosted CI usually lives there.
fn forbidden(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local() || v4.is_unspecified() || v4.is_multicast() || v4.is_broadcast(),
        IpAddr::V6(v6) => {
            (v6.segments()[0] & 0xffc0) == 0xfe80
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6.to_ipv4_mapped().is_some_and(|v4| forbidden(IpAddr::V4(v4)))
        }
    }
}

fn hook_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<(Webhook, String)> {
    let events: String = r.get(4)?;
    Ok((
        Webhook {
            id: r.get(0)?,
            repository: Some(pando_proto::RepositoryRef { owner: r.get(1)?, name: r.get(2)? }),
            url: r.get(3)?,
            events: events.split(',').filter(|e| !e.is_empty()).map(str::to_owned).collect(),
            active: r.get::<_, i64>(5)? != 0,
            created_at_ms: r.get(6)?,
            created_by: r.get(7)?,
            last_status: r.get::<_, Option<i32>>(9)?.unwrap_or(0),
            last_delivery_at_ms: r.get::<_, Option<i64>>(10)?.unwrap_or(0),
        },
        r.get(8)?,
    ))
}

const HOOK_SELECT: &str = "SELECT h.id, h.owner, h.name, h.url, h.events, h.active, h.created_at_ms, h.created_by, h.secret,
       (SELECT status FROM deliveries d WHERE d.hook = h.id AND d.attempts > 0 ORDER BY d.created_at_ms DESC LIMIT 1),
       (SELECT created_at_ms FROM deliveries d WHERE d.hook = h.id AND d.attempts > 0 ORDER BY d.created_at_ms DESC LIMIT 1)
     FROM webhooks h";

impl Registry {
    /// Adds a webhook; returns it and its secret.
    pub fn create_webhook(
        &self,
        key: &RepoKey,
        url: &str,
        events: &[String],
        secret: &str,
        by: &str,
    ) -> anyhow::Result<(Webhook, String)> {
        let id = random_hex(8)?;
        let secret = if secret.is_empty() { random_hex(32)? } else { secret.to_owned() };
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO webhooks (id, owner, name, url, secret, events, created_at_ms, created_by) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![id, key.owner, key.name, url, secret, events.join(","), now_ms(), by],
        )?;
        self.invalidate();
        drop(db);
        Ok((self.webhook(&id)?.expect("just added").0, secret))
    }

    pub fn webhooks(&self, key: &RepoKey) -> anyhow::Result<Vec<Webhook>> {
        let db = self.db.lock().unwrap();
        let mut st =
            db.prepare_cached(&format!("{HOOK_SELECT} WHERE h.owner = ?1 AND h.name = ?2 ORDER BY h.created_at_ms"))?;
        let rows = st.query_map((&key.owner, &key.name), hook_row)?;
        Ok(rows.map(|r| r.map(|(h, _)| h)).collect::<Result<_, _>>()?)
    }

    /// A webhook and its secret.
    pub fn webhook(&self, id: &str) -> anyhow::Result<Option<(Webhook, String)>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(&format!("{HOOK_SELECT} WHERE h.id = ?1"))?;
        let mut rows = st.query_map([id], hook_row)?;
        Ok(rows.next().transpose()?)
    }

    pub fn delete_webhook(&self, id: &str) -> anyhow::Result<bool> {
        let db = self.db.lock().unwrap();
        db.execute("DELETE FROM deliveries WHERE hook = ?1", [id])?;
        let n = db.execute("DELETE FROM webhooks WHERE id = ?1", [id])?;
        self.invalidate();
        Ok(n == 1)
    }

    pub fn deliveries(&self, hook: &str, limit: usize) -> anyhow::Result<Vec<WebhookDelivery>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(
            "SELECT id, event, status, error, attempts, created_at_ms, delivered, payload FROM deliveries
             WHERE hook = ?1 ORDER BY created_at_ms DESC LIMIT ?2",
        )?;
        let rows = st.query_map(rusqlite::params![hook, limit as i64], |r| {
            Ok(WebhookDelivery {
                id: r.get(0)?,
                event: r.get(1)?,
                status: r.get(2)?,
                error: r.get(3)?,
                attempts: r.get(4)?,
                created_at_ms: r.get(5)?,
                delivered: r.get::<_, i64>(6)? != 0,
                payload: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Queues `event` for every active webhook of the repository that wants it; returns the
    /// deliveries' ids.
    pub fn enqueue(
        &self,
        key: &RepoKey,
        event: &str,
        payload: &Value,
        only: Option<&str>,
    ) -> anyhow::Result<Vec<String>> {
        let hooks: Vec<Webhook> = match only {
            Some(id) => self.webhook(id)?.map(|(h, _)| h).into_iter().collect(),
            // Every push asks; most repositories have no webhooks, so the answer is cached.
            None => self
                .cached(format!("hooks:{key}"), || self.webhooks(key))?
                .into_iter()
                .filter(|h| h.active && (h.events.is_empty() || h.events.iter().any(|e| e == event)))
                .collect(),
        };
        if hooks.is_empty() {
            return Ok(vec![]);
        }
        let mut ids = Vec::new();
        let db = self.db.lock().unwrap();
        for h in hooks {
            let id = random_hex(8)?;
            let mut body = payload.clone();
            body["delivery"] = json!(id);
            // A delivery to one named hook (a ping) is sent by the caller right away; the worker
            // only picks it up if that never happened, so it is never sent twice.
            let now = now_ms();
            let first = if only.is_some() { now + 60_000 } else { now };
            db.execute(
                "INSERT INTO deliveries (id, hook, event, payload, next_at_ms, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![id, h.id, event, body.to_string(), first, now],
            )?;
            // Keep the log short.
            db.execute(
                "DELETE FROM deliveries WHERE hook = ?1 AND delivered = 1 AND id NOT IN
                   (SELECT id FROM deliveries WHERE hook = ?1 ORDER BY created_at_ms DESC LIMIT ?2)",
                rusqlite::params![h.id, KEEP],
            )?;
            ids.push(id);
        }
        Ok(ids)
    }

    /// Claims deliveries due now. Claiming moves
    /// their next attempt a minute ahead in the same statement, so workers on several replicas
    /// never post the same delivery twice; a worker that dies leaves it to be retried.
    fn claim_due(&self, limit: usize) -> anyhow::Result<Vec<Due>> {
        let now = now_ms();
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(
            "UPDATE deliveries SET next_at_ms = ?2 + 60000 WHERE id IN (
               SELECT id FROM deliveries WHERE delivered = 0 AND attempts < ?1 AND next_at_ms <= ?2 ORDER BY next_at_ms LIMIT ?3)
             RETURNING id, hook, event, payload, attempts",
        )?;
        let claimed: Vec<(String, String, String, String, i64)> = st
            .query_map(rusqlite::params![MAX_ATTEMPTS, now, limit as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<Result<_, _>>()?;
        let mut hook = db.prepare_cached("SELECT url, secret FROM webhooks WHERE id = ?1")?;
        let mut out = Vec::new();
        for (id, h, event, payload, attempts) in claimed {
            if let Ok((url, secret)) = hook.query_row([&h], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
                out.push(Due { id, url, secret, event, payload, attempts });
            }
        }
        Ok(out)
    }

    fn record_attempt(&self, id: &str, attempts: i64, status: i32, error: &str, delivered: bool) -> anyhow::Result<()> {
        let next = now_ms() + 10_000 * (1i64 << attempts.min(7));
        let db = self.db.lock().unwrap();
        db.execute(
            "UPDATE deliveries SET attempts = ?2, status = ?3, error = ?4, delivered = ?5, next_at_ms = ?6 WHERE id = ?1",
            rusqlite::params![id, attempts, status, error, delivered as i64, next],
        )?;
        Ok(())
    }

    /// Whether any active webhook of the repository wants `event` (cached).
    pub fn wants(&self, key: &RepoKey, event: &str) -> bool {
        self.cached(format!("hooks:{key}"), || self.webhooks(key))
            .map(|hooks| hooks.iter().any(|h| h.active && (h.events.is_empty() || h.events.iter().any(|e| e == event))))
            .unwrap_or(true)
    }

    pub fn delivery(&self, id: &str) -> anyhow::Result<Option<WebhookDelivery>> {
        let db = self.db.lock().unwrap();
        let hook: Option<String> = db.query_row("SELECT hook FROM deliveries WHERE id = ?1", [id], |r| r.get(0)).ok();
        drop(db);
        let Some(hook) = hook else { return Ok(None) };
        Ok(self.deliveries(&hook, KEEP as usize)?.into_iter().find(|d| d.id == id))
    }
}

/// A delivery claimed for sending.
struct Due {
    id: String,
    url: String,
    secret: String,
    event: String,
    payload: String,
    attempts: i64,
}

/// The envelope every event shares.
pub fn envelope(event: &str, action: &str, key: &RepoKey, sender: &str) -> Value {
    json!({
        "event": event,
        "action": action,
        "repository": { "owner": key.owner, "name": key.name, "full_name": key.to_string() },
        "sender": sender,
        "at_ms": now_ms(),
    })
}

/// Queues an event; a failure to queue is logged, never fails the action that caused it.
pub fn emit(reg: &Registry, key: &RepoKey, event: &str, payload: Value) {
    if let Err(e) = reg.enqueue(key, event, &payload, None) {
        tracing::warn!(error = %e, event, repo = %key, "queueing a webhook");
    }
}

/// Posts one delivery. Returns the HTTP status (or -1) and an error message.
async fn post(client: &reqwest::Client, url: &str, secret: &str, event: &str, id: &str, body: String) -> (i32, String) {
    let Ok(u) = check_url(url) else { return (-1, "not a valid URL".into()) };
    let host = u.host_str().unwrap_or_default().trim_matches(['[', ']']).to_owned();
    let port = u.port_or_known_default().unwrap_or(443);
    // ponytail: checked at resolve time, not at connect time (a DNS answer could change in
    // between); pin the resolved address in the client if that ever matters.
    match tokio::net::lookup_host((host.as_str(), port)).await {
        Ok(addrs) => {
            let addrs: Vec<_> = addrs.collect();
            if addrs.is_empty() || addrs.iter().any(|a| forbidden(a.ip())) {
                return (-1, format!("{host} resolves to an address webhooks may not call"));
            }
        }
        Err(e) => return (-1, format!("resolving {host}: {e}")),
    }
    let sent = client
        .post(u)
        .header("content-type", "application/json")
        .header("user-agent", concat!("pando-webhooks/", env!("CARGO_PKG_VERSION")))
        .header("x-pando-event", event)
        .header("x-pando-delivery", id)
        .header("x-pando-signature-256", signature(secret, body.as_bytes()))
        .body(body)
        .send()
        .await;
    match sent {
        Ok(r) if r.status().is_success() => (r.status().as_u16() as i32, String::new()),
        Ok(r) => (r.status().as_u16() as i32, format!("answered {}", r.status())),
        Err(e) => (-1, e.to_string()),
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("an HTTP client")
}

/// Delivers one queued delivery now (used by ping); returns how it went.
pub async fn deliver_now(reg: &Registry, id: &str) -> anyhow::Result<Option<WebhookDelivery>> {
    let due = {
        let db = reg.db.lock().unwrap();
        db.query_row(
            "SELECT h.url, h.secret, d.event, d.payload, d.attempts FROM deliveries d JOIN webhooks h ON h.id = d.hook WHERE d.id = ?1",
            [id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?)),
        )
        .ok()
    };
    if let Some((url, secret, event, payload, attempts)) = due {
        let (status, error) = post(&client(), &url, &secret, &event, id, payload).await;
        reg.record_attempt(id, attempts + 1, status, &error, (200..300).contains(&status))?;
    }
    reg.delivery(id)
}

/// Posts due deliveries until the server stops.
pub async fn run(reg: Arc<Registry>) {
    let client = client();
    loop {
        let due = match reg.claim_due(32) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "reading webhook deliveries");
                vec![]
            }
        };
        if due.is_empty() {
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }
        let posts = due.into_iter().map(|Due { id, url, secret, event, payload, attempts }| {
            let (reg, client) = (reg.clone(), client.clone());
            async move {
                let (status, error) = post(&client, &url, &secret, &event, &id, payload).await;
                let ok = (200..300).contains(&status);
                if !ok {
                    tracing::info!(delivery = %id, status, error, attempt = attempts + 1, "webhook delivery failed");
                }
                if let Err(e) = reg.record_attempt(&id, attempts + 1, status, &error, ok) {
                    tracing::warn!(error = %e, "recording a webhook delivery");
                }
            }
        });
        futures::future::join_all(posts).await;
    }
}

/// A repository's branches, reporting every move as a `push` event.
pub struct NotifyingRefs {
    pub inner: Arc<dyn pando_core::ports::RefStore>,
    pub log: Arc<dyn pando_core::ports::ChangesetLog>,
    pub registry: Arc<Registry>,
    pub key: RepoKey,
}

#[tonic::async_trait]
impl pando_core::ports::RefStore for NotifyingRefs {
    async fn get(&self, name: &str) -> CoreResult<Option<Id>> {
        self.inner.get(name).await
    }

    async fn compare_and_swap(&self, name: &str, expected: Option<Id>, new: Id) -> CoreResult<bool> {
        let moved = self.inner.compare_and_swap(name, expected, new).await?;
        // Branch moves are on the landing path: do nothing more unless a webhook wants pushes
        // (a cached answer), and then queue the event in the background.
        if moved && !name.starts_with('_') && self.registry.wants(&self.key, "push") {
            let (registry, log, key, name) =
                (self.registry.clone(), self.log.clone(), self.key.clone(), name.to_owned());
            tokio::spawn(async move {
                let head = match log.get(&new).await {
                    Ok(Some(bytes)) => <pando_core::object::Changeset as prost::Message>::decode(bytes.as_ref()).ok(),
                    _ => None,
                };
                let (author, message) = head.map(|c| (c.author, c.message)).unwrap_or_default();
                let mut p = envelope("push", if expected.is_none() { "created" } else { "updated" }, &key, &author);
                p["ref"] = json!(name);
                p["before"] = json!(expected.map(|e| e.to_hex()));
                p["after"] = json!(new.to_hex());
                p["head"] = json!({ "id": new.to_hex(), "author": author, "message": message });
                emit(&registry, &key, "push", p);
            });
        }
        Ok(moved)
    }

    async fn list(&self) -> CoreResult<Vec<(String, Id)>> {
        self.inner.list().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_match_githubs() {
        // The example from GitHub's webhook documentation.
        assert_eq!(
            signature("It's a Secret to Everybody", b"Hello, World!"),
            "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"
        );
    }

    #[test]
    fn urls_and_addresses() {
        assert!(check_url("https://ci.example.com/hooks/pando").is_ok());
        assert!(check_url("http://10.0.0.5:8080/hook").is_ok(), "private networks are allowed");
        for bad in ["ftp://x", "https://", "not a url", "https://user:pw@example.com/"] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
        for ip in ["169.254.169.254", "0.0.0.0", "224.0.0.1", "fe80::1", "::ffff:169.254.169.254"] {
            assert!(forbidden(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["10.0.0.5", "127.0.0.1", "93.184.216.34", "::1"] {
            assert!(!forbidden(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn events_queue_only_for_hooks_that_want_them() {
        let reg = Registry::open("memory").unwrap();
        let key = RepoKey::parse("lumen/fleet").unwrap();
        let (all, secret) = reg.create_webhook(&key, "https://a.example/", &[], "", "kabir").unwrap();
        assert_eq!(secret.len(), 64);
        reg.create_webhook(&key, "https://b.example/", &["review".into()], "s", "kabir").unwrap();
        let ids = reg.enqueue(&key, "push", &envelope("push", "updated", &key, "dev"), None).unwrap();
        assert_eq!(ids.len(), 1, "only the hook that wants every event");
        assert_eq!(reg.enqueue(&key, "review", &json!({}), None).unwrap().len(), 2);
        let d = reg.deliveries(&all.id, 10).unwrap();
        assert_eq!(d.len(), 2);
        assert!(d.iter().all(|x| !x.delivered && x.attempts == 0));
        let payload: Value = serde_json::from_str(&d[1].payload).unwrap();
        assert_eq!(payload["delivery"], json!(d[1].id));
        assert!(reg.delete_webhook(&all.id).unwrap());
        assert!(reg.deliveries(&all.id, 10).unwrap().is_empty());
    }
}
