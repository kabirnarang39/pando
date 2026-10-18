//! Plans, usage and limits of organisations (ADR-035).
//!
//! Usage is recorded on every platform server: an agent becomes *active* in an organisation for
//! a calendar month (UTC) the first time it lands a change there, and repository sizes are
//! measured hourly. Limits (`org_limits`, or the server's `--default-org-limits`) are enforced
//! only with `--multi-tenant`: a suspended organisation can't write, one over its storage can't
//! write, and a new agent beyond the active-agent limit can't land while agents already active
//! this month keep landing. Seats are checked when members are added, private repositories when
//! they are created.

use std::collections::HashSet;
use std::sync::Mutex;

use anyhow::{Context as _, bail};
use pando_core::acl::Principal;
use pando_proto::{AccountKind, OrgLimits, Visibility};
use tonic::Status;
use tonic::codegen::{Bytes, http};

use crate::directory::kind_of;
use crate::hosting::{Hosting, Registry, RepoKey, now_ms};
use crate::status::internal;

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS org_limits (
  org TEXT PRIMARY KEY,
  seats INTEGER,
  active_agents INTEGER,
  storage_bytes INTEGER,
  private_repos INTEGER,
  suspended INTEGER NOT NULL DEFAULT 0,
  plan_label TEXT NOT NULL DEFAULT '',
  updated_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS usage_agents (
  org TEXT NOT NULL,
  month TEXT NOT NULL,
  login TEXT NOT NULL,
  first_at_ms INTEGER NOT NULL,
  PRIMARY KEY (org, month, login)
);
CREATE TABLE IF NOT EXISTS repo_sizes (
  owner TEXT NOT NULL,
  name TEXT NOT NULL,
  bytes INTEGER NOT NULL,
  measured_at_ms INTEGER NOT NULL,
  PRIMARY KEY (owner, name)
);
CREATE TABLE IF NOT EXISTS usage_storage (
  org TEXT NOT NULL,
  month TEXT NOT NULL,
  peak_bytes INTEGER NOT NULL,
  PRIMARY KEY (org, month)
);
";

/// What an organisation may use; `None` is unlimited.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Limits {
    pub seats: Option<u64>,
    pub active_agents: Option<u64>,
    pub storage_bytes: Option<u64>,
    pub private_repos: Option<u64>,
    pub suspended: bool,
    pub plan_label: String,
}

impl Limits {
    /// `seats=N,agents=N,storage=SIZE,private_repos=N,plan=LABEL`, each optional;
    /// `unlimited` for no limit.
    pub fn parse(spec: &str) -> anyhow::Result<Self> {
        let mut l = Limits::default();
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (k, v) = part.split_once('=').with_context(|| format!("`{part}` is not KEY=VALUE"))?;
            let n = |v: &str| -> anyhow::Result<Option<u64>> {
                if v == "unlimited" {
                    Ok(None)
                } else {
                    Ok(Some(v.parse().with_context(|| format!("`{v}` is not a number"))?))
                }
            };
            match k.trim() {
                "seats" => l.seats = n(v)?,
                "agents" | "active_agents" => l.active_agents = n(v)?,
                "storage" | "storage_bytes" => {
                    l.storage_bytes = if v == "unlimited" { None } else { Some(parse_bytes(v)?) }
                }
                "private_repos" => l.private_repos = n(v)?,
                "plan" => l.plan_label = v.to_owned(),
                other => bail!("unknown limit `{other}`: use seats, agents, storage, private_repos or plan"),
            }
        }
        Ok(l)
    }

    pub fn to_proto(&self) -> OrgLimits {
        OrgLimits {
            seats: self.seats,
            active_agents: self.active_agents,
            storage_bytes: self.storage_bytes,
            private_repos: self.private_repos,
            suspended: self.suspended,
            plan_label: self.plan_label.clone(),
        }
    }

    pub fn from_proto(p: &OrgLimits) -> Self {
        Self {
            seats: p.seats,
            active_agents: p.active_agents,
            storage_bytes: p.storage_bytes,
            private_repos: p.private_repos,
            suspended: p.suspended,
            plan_label: p.plan_label.clone(),
        }
    }
}

/// `2GiB`, `500MB`, `1048576`: binary (KiB…TiB) and decimal (KB…TB) units.
pub fn parse_bytes(s: &str) -> anyhow::Result<u64> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (n, unit) = s.split_at(split);
    let n: u64 = n.parse().with_context(|| format!("`{s}` is not a size such as 2GiB"))?;
    let mul: u64 = match unit.trim() {
        "" | "B" => 1,
        "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        "TB" => 1_000_000_000_000,
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        "TiB" => 1 << 40,
        u => bail!("unknown size unit `{u}`: use B, KB, MB, GB, TB, KiB, MiB, GiB or TiB"),
    };
    n.checked_mul(mul).with_context(|| format!("`{s}` is too large"))
}

/// `YYYY-MM` (UTC) of a time in milliseconds.
pub fn month_of(ms: i64) -> String {
    // Days to civil date (Howard Hinnant's algorithm).
    let z = ms.div_euclid(86_400_000) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}")
}

/// Whether `s` is a `YYYY-MM` month.
pub fn valid_month(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 7
        && b[4] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || c.is_ascii_digit())
        && matches!(&s[5..], "01" | "02" | "03" | "04" | "05" | "06" | "07" | "08" | "09" | "10" | "11" | "12")
}

/// The first day of the month after `month`, `YYYY-MM-01`.
pub fn next_month_start(month: &str) -> String {
    let y: i64 = month[..4].parse().unwrap_or(1970);
    let m: i64 = month[5..7].parse().unwrap_or(1);
    if m == 12 { format!("{:04}-01-01", y + 1) } else { format!("{y:04}-{:02}-01", m + 1) }
}

fn limit(v: Option<i64>) -> Option<u64> {
    v.map(|n| n.max(0) as u64)
}

fn sql_n(v: Option<u64>) -> Option<i64> {
    v.map(|n| n.min(i64::MAX as u64) as i64)
}

impl Registry {
    /// The organisation's own limits, if it has any.
    pub fn org_limits(&self, org: &str) -> anyhow::Result<Option<Limits>> {
        self.cached(format!("limits:{org}"), || {
            let db = self.db.lock().unwrap();
            let mut st = db.prepare_cached(
                "SELECT seats, active_agents, storage_bytes, private_repos, suspended, plan_label
                 FROM org_limits WHERE org = ?1",
            )?;
            let mut rows = st.query_map([org], |r| {
                Ok(Limits {
                    seats: limit(r.get(0)?),
                    active_agents: limit(r.get(1)?),
                    storage_bytes: limit(r.get(2)?),
                    private_repos: limit(r.get(3)?),
                    suspended: r.get::<_, i64>(4)? != 0,
                    plan_label: r.get(5)?,
                })
            })?;
            Ok(rows.next().transpose()?)
        })
    }

    /// The limits that apply: the organisation's own, else `defaults`, else none.
    pub fn limits_for(&self, org: &str, defaults: Option<&Limits>) -> anyhow::Result<Limits> {
        Ok(self.org_limits(org)?.or_else(|| defaults.cloned()).unwrap_or_default())
    }

    /// Replaces the organisation's limits, keeping whether it is suspended.
    pub fn set_org_limits(&self, org: &str, l: &Limits) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO org_limits (org, seats, active_agents, storage_bytes, private_repos, suspended, plan_label, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7)
             ON CONFLICT (org) DO UPDATE SET seats = excluded.seats, active_agents = excluded.active_agents,
               storage_bytes = excluded.storage_bytes, private_repos = excluded.private_repos,
               plan_label = excluded.plan_label, updated_at_ms = excluded.updated_at_ms",
            (
                org,
                sql_n(l.seats),
                sql_n(l.active_agents),
                sql_n(l.storage_bytes),
                sql_n(l.private_repos),
                &l.plan_label,
                now_ms(),
            ),
        )?;
        drop(db);
        self.invalidate();
        Ok(())
    }

    /// Suspends or restores an organisation. One without limits of its own takes `defaults`.
    pub fn set_suspended(&self, org: &str, suspended: bool, defaults: &Limits) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO org_limits (org, seats, active_agents, storage_bytes, private_repos, suspended, plan_label, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (org) DO UPDATE SET suspended = excluded.suspended, updated_at_ms = excluded.updated_at_ms",
            (
                org,
                sql_n(defaults.seats),
                sql_n(defaults.active_agents),
                sql_n(defaults.storage_bytes),
                sql_n(defaults.private_repos),
                suspended as i64,
                &defaults.plan_label,
                now_ms(),
            ),
        )?;
        drop(db);
        self.invalidate();
        Ok(())
    }

    /// Records `login` as active in `org` for `month`; true the first time.
    pub fn record_active_agent(&self, org: &str, month: &str, login: &str, at_ms: i64) -> anyhow::Result<bool> {
        let db = self.db.lock().unwrap();
        Ok(db.execute(
            "INSERT INTO usage_agents (org, month, login, first_at_ms) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (org, month, login) DO NOTHING",
            (org, month, login, at_ms),
        )? == 1)
    }

    pub fn is_active_agent(&self, org: &str, month: &str, login: &str) -> anyhow::Result<bool> {
        let db = self.db.lock().unwrap();
        Ok(db.query_row(
            "SELECT EXISTS (SELECT 1 FROM usage_agents WHERE org = ?1 AND month = ?2 AND login = ?3)",
            (org, month, login),
            |r| r.get(0),
        )?)
    }

    /// Agents active in `org` in `month`, with when each first landed, oldest first.
    pub fn active_agents(&self, org: &str, month: &str) -> anyhow::Result<Vec<(String, i64)>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(
            "SELECT login, first_at_ms FROM usage_agents WHERE org = ?1 AND month = ?2 ORDER BY first_at_ms, login",
        )?;
        let rows = st.query_map((org, month), |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn count_active_agents(&self, org: &str, month: &str) -> anyhow::Result<u64> {
        let db = self.db.lock().unwrap();
        let n: i64 =
            db.query_row("SELECT COUNT(*) FROM usage_agents WHERE org = ?1 AND month = ?2", (org, month), |r| {
                r.get(0)
            })?;
        Ok(n as u64)
    }

    /// Records a repository's measured size and raises its organisation's peak for the month.
    pub fn record_repo_size(&self, key: &RepoKey, bytes: u64, at_ms: i64) -> anyhow::Result<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO repo_sizes (owner, name, bytes, measured_at_ms) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (owner, name) DO UPDATE SET bytes = excluded.bytes, measured_at_ms = excluded.measured_at_ms",
            (&key.owner, &key.name, sql_n(Some(bytes)), at_ms),
        )?;
        let total: i64 =
            tx.query_row("SELECT COALESCE(SUM(bytes), 0) FROM repo_sizes WHERE owner = ?1", [&key.owner], |r| {
                r.get(0)
            })?;
        tx.execute(
            "INSERT INTO usage_storage (org, month, peak_bytes) VALUES (?1, ?2, ?3)
             ON CONFLICT (org, month) DO UPDATE SET peak_bytes = excluded.peak_bytes
             WHERE excluded.peak_bytes > usage_storage.peak_bytes",
            (&key.owner, month_of(at_ms), total),
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The organisation's storage at its last measurement, and when that was.
    pub fn org_storage(&self, org: &str) -> anyhow::Result<(u64, i64)> {
        self.cached(format!("storage:{org}"), || {
            let db = self.db.lock().unwrap();
            let (b, at): (i64, i64) = db.query_row(
                "SELECT COALESCE(SUM(bytes), 0), COALESCE(MAX(measured_at_ms), 0) FROM repo_sizes WHERE owner = ?1",
                [org],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            Ok((b.max(0) as u64, at))
        })
    }

    pub fn peak_storage(&self, org: &str, month: &str) -> anyhow::Result<u64> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached("SELECT peak_bytes FROM usage_storage WHERE org = ?1 AND month = ?2")?;
        let peak: Option<i64> = st.query_map((org, month), |r| r.get(0))?.next().transpose()?;
        Ok(peak.unwrap_or(0).max(0) as u64)
    }

    pub fn private_repo_count(&self, org: &str) -> anyhow::Result<u64> {
        let db = self.db.lock().unwrap();
        let n: i64 = db.query_row(
            "SELECT COUNT(*) FROM repositories WHERE owner = ?1 AND visibility = ?2",
            (org, Visibility::Private as i32),
            |r| r.get(0),
        )?;
        Ok(n as u64)
    }

    /// People (not agents) who are members of `org`.
    pub fn seat_count(&self, org: &str) -> anyhow::Result<u64> {
        Ok(self.members(org)?.iter().filter(|(l, _)| kind_of(l) == AccountKind::Person).count() as u64)
    }
}

/// Agents already recorded as active this month, so only an agent's first land in a month
/// writes to the registry.
#[derive(Default)]
pub struct Meter {
    // ponytail: one process's view; another replica records the same agent once more, which
    // the table's primary key absorbs.
    seen: Mutex<(String, HashSet<(String, String)>)>,
}

impl Meter {
    fn seen(&self, month: &str, org: &str, login: &str) -> bool {
        let s = self.seen.lock().unwrap();
        s.0 == month && s.1.contains(&(org.to_owned(), login.to_owned()))
    }

    fn remember(&self, month: &str, org: &str, login: &str) {
        let mut s = self.seen.lock().unwrap();
        if s.0 != month {
            *s = (month.to_owned(), HashSet::new());
        }
        s.1.insert((org.to_owned(), login.to_owned()));
    }

    /// Whether `login` is already active in `org` this month.
    fn active(&self, reg: &Registry, org: &str, month: &str, login: &str) -> anyhow::Result<bool> {
        if self.seen(month, org, login) {
            return Ok(true);
        }
        let active = reg.is_active_agent(org, month, login)?;
        if active {
            self.remember(month, org, login);
        }
        Ok(active)
    }
}

/// Calls that land a change: they make an agent active.
pub fn is_land(path: &str) -> bool {
    path == "/pando.v1.Land/Land" || path == "/pando.v1.Refs/CompareAndSwap"
}


/// "upgrade at URL", or who to ask when the server has no billing page.
fn upgrade(hosting: &Hosting) -> String {
    upgrade_to(&hosting.access.billing_url)
}

fn upgrade_to(billing_url: &str) -> String {
    match billing_url {
        "" => "ask the server's administrators to raise it".to_owned(),
        u => format!("upgrade at {u}"),
    }
}

/// What the registry needs to check seats wherever members are added (the Platform API and
/// invitations): set once, when hosting starts.
#[derive(Debug, Default)]
pub struct Policy {
    /// Limits are enforced (`--multi-tenant`).
    pub enforce: bool,
    pub defaults: Option<Limits>,
    pub billing_url: String,
}

/// Refuses a write to `key` that its organisation's plan doesn't allow (`--multi-tenant` only).
/// `path` is the call; reads are never refused here.
pub fn check_write(hosting: &Hosting, key: &RepoKey, who: &Principal, path: &str) -> Result<(), Status> {
    let landers: &[&str] = if is_land(path) { &[who.0.as_str()] } else { &[] };
    check_landing(hosting, key, landers)
}

/// Refuses a write to `key` that its organisation's plan doesn't allow, where `landers` are the
/// accounts the write would make active (agents among them count; `--multi-tenant` only).
pub fn check_landing(hosting: &Hosting, key: &RepoKey, landers: &[&str]) -> Result<(), Status> {
    let access = &hosting.access;
    if !access.multi_tenant {
        return Ok(());
    }
    let reg = &hosting.registry;
    let org = &key.owner;
    let l = reg.limits_for(org, access.default_org_limits.as_ref()).map_err(internal)?;
    if l.suspended {
        return Err(Status::permission_denied(format!(
            "organisation {org} is suspended; contact support. Its repositories can still be read"
        )));
    }
    if let Some(max) = l.storage_bytes {
        let (used, _) = reg.org_storage(org).map_err(internal)?;
        if used > max {
            return Err(Status::resource_exhausted(format!(
                "{org} uses {} of its {} of storage, so it can't take more changes; {}, or delete repositories it no longer needs. Reads still work",
                human_bytes(used),
                human_bytes(max),
                upgrade(hosting)
            )));
        }
    }
    let Some(max) = l.active_agents else { return Ok(()) };
    let month = month_of(now_ms());
    let mut new = 0;
    let agents: std::collections::BTreeSet<&str> =
        landers.iter().copied().filter(|l| kind_of(l) == AccountKind::Agent).collect();
    for agent in agents {
        // ponytail: checked before the land and recorded after it, so new agents landing at the
        // same moment can pass the limit by a few; a reservation would count refused lands.
        if hosting.meter.active(reg, org, &month, agent).map_err(internal)? {
            continue;
        }
        new += 1;
        let n = reg.count_active_agents(org, &month).map_err(internal)?;
        if n + new > max {
            return Err(Status::resource_exhausted(format!(
                "{org} has {n} of {max} active agents this month, so `{agent}` can't start landing; {}, or wait until {} \
                 (agents that already landed this month keep landing)",
                upgrade(hosting),
                next_month_start(&month)
            )));
        }
    }
    Ok(())
}

/// Records `login`, if it is an agent, as active in `org` this month: after a change it made or
/// authored has landed.
pub fn record_landed(hosting: &Hosting, org: &str, login: &str) {
    if kind_of(login) != AccountKind::Agent {
        return;
    }
    let month = month_of(now_ms());
    if hosting.meter.seen(&month, org, login) {
        return;
    }
    match hosting.registry.record_active_agent(org, &month, login, now_ms()) {
        Ok(_) => hosting.meter.remember(&month, org, login),
        Err(e) => tracing::warn!(error = %e, agent = login, "recording an active agent"),
    }
}

/// Whether a Land or CompareAndSwap response is a landed change: status OK, and the change
/// landed (Land) or the ref moved (CompareAndSwap). A refused land is a gRPC error; a lost
/// compare-and-swap is status OK with `swapped: false`.
fn landed(path: &str, headers: &http::HeaderMap, trailers: Option<&http::HeaderMap>, body: &[u8]) -> bool {
    use prost::Message;
    let status = headers.get("grpc-status").or_else(|| trailers.and_then(|t| t.get("grpc-status")));
    if status.is_none_or(|s| s != "0") || body.len() < 5 || body[0] != 0 {
        return false;
    }
    let msg = &body[5..];
    if path == "/pando.v1.Land/Land" {
        pando_proto::LandResponse::decode(msg).is_ok_and(|r| !r.landed.is_empty())
    } else {
        pando_proto::CasResponse::decode(msg).is_ok_and(|r| r.swapped)
    }
}

/// Records the agent as active once its land has succeeded, passing the response on unchanged.
pub async fn observe_land(
    hosting: &Hosting,
    key: &RepoKey,
    who: &Principal,
    path: &str,
    resp: http::Response<tonic::body::Body>,
) -> http::Response<tonic::body::Body> {
    use http_body_util::BodyExt;
    if !is_land(path) || kind_of(&who.0) != AccountKind::Agent {
        return resp;
    }
    let (parts, body) = resp.into_parts();
    // Land and CompareAndSwap answer with one small message: buffering it costs nothing.
    let collected = match body.collect().await {
        Ok(c) => c,
        Err(status) => return status.into_http(),
    };
    let trailers = collected.trailers().cloned();
    let data: Bytes = collected.to_bytes();
    if landed(path, &parts.headers, trailers.as_ref(), &data) {
        record_landed(hosting, &key.owner, &who.0);
    }
    let mut frames = vec![Ok::<_, Status>(http_body::Frame::data(data))];
    frames.extend(trailers.map(|t| Ok(http_body::Frame::trailers(t))));
    let body = http_body_util::StreamBody::new(futures::stream::iter(frames));
    http::Response::from_parts(parts, tonic::body::Body::new(body))
}

/// Refuses a new member beyond the organisation's seats (`--multi-tenant` only). Agents don't
/// take seats, and changing an existing member's role never does.
pub fn check_seat(reg: &Registry, org: &str, login: &str) -> Result<(), Status> {
    let Some(policy) = reg.plan_policy.get().filter(|p| p.enforce) else { return Ok(()) };
    if kind_of(login) != AccountKind::Person {
        return Ok(());
    }
    let l = reg.limits_for(org, policy.defaults.as_ref()).map_err(internal)?;
    let Some(max) = l.seats else { return Ok(()) };
    if reg.members(org).map_err(internal)?.iter().any(|(l, _)| l == login) {
        return Ok(());
    }
    let n = reg.seat_count(org).map_err(internal)?;
    if n >= max {
        return Err(Status::resource_exhausted(format!(
            "{org} has {n} of {max} seats taken; {}, or remove someone first",
            upgrade_to(&policy.billing_url)
        )));
    }
    Ok(())
}

/// Refuses a new repository in a suspended organisation, and a new private one (also one made
/// private) beyond the organisation's limit (`--multi-tenant` only).
pub fn check_new_repo(hosting: &Hosting, org: &str, private: bool) -> Result<(), Status> {
    let reg = &hosting.registry;
    if !hosting.access.multi_tenant {
        return Ok(());
    }
    let l = reg.limits_for(org, hosting.access.default_org_limits.as_ref()).map_err(internal)?;
    if l.suspended {
        return Err(Status::permission_denied(format!("organisation {org} is suspended; contact support")));
    }
    let Some(max) = l.private_repos.filter(|_| private) else { return Ok(()) };
    let n = reg.private_repo_count(org).map_err(internal)?;
    if n >= max {
        return Err(Status::resource_exhausted(format!(
            "{org} has {n} of {max} private repositories; {}, or make one public",
            upgrade(hosting)
        )));
    }
    Ok(())
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else { format!("{v:.1} {}", UNITS[i]) }
}

/// Bytes a repository takes: its SQLite database and write-ahead log, and its objects when
/// they live elsewhere. Postgres metadata isn't counted.
async fn repo_bytes(metadata: &str, objects: &str) -> anyhow::Result<u64> {
    let mut n = 0;
    if let Some(path) = metadata.strip_prefix("sqlite://") {
        for p in [path.to_owned(), format!("{path}-wal")] {
            n += std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        }
    }
    if objects != "metadata" {
        n += pando_adapters::cloud::CloudObjects::from_url(objects)?.size_bytes().await?;
    }
    Ok(n)
}

/// Measures every hosted repository once.
pub async fn measure_storage(hosting: &Hosting) -> anyhow::Result<()> {
    for rec in hosting.registry.list(None)? {
        let (metadata, objects) = hosting.templates.render(&rec.key);
        match repo_bytes(&metadata, &objects).await {
            Ok(bytes) => hosting.registry.record_repo_size(&rec.key, bytes, now_ms())?,
            Err(e) => tracing::warn!(repo = %rec.key, error = %e, "measuring a repository's size"),
        }
    }
    // Storage answers are cached with everything else: show the new sizes at once.
    hosting.registry.invalidate();
    Ok(())
}

/// Measures storage every hour, for as long as the server runs.
pub async fn run(hosting: std::sync::Arc<Hosting>) {
    let mut every = tokio::time::interval(std::time::Duration::from_secs(3600));
    loop {
        every.tick().await;
        if let Err(e) = measure_storage(&hosting).await {
            tracing::warn!(error = %e, "measuring storage");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_and_sizes_parse() {
        let l = Limits::parse("seats=4, agents=7,storage=3GiB,private_repos=unlimited,plan=Trial").unwrap();
        assert_eq!(
            l,
            Limits {
                seats: Some(4),
                active_agents: Some(7),
                storage_bytes: Some(3 << 30),
                private_repos: None,
                suspended: false,
                plan_label: "Trial".into()
            }
        );
        assert_eq!(parse_bytes("500MB").unwrap(), 500_000_000);
        assert_eq!(parse_bytes("12").unwrap(), 12);
        assert!(parse_bytes("2 parsecs").is_err());
        assert!(Limits::parse("seats=x").is_err());
        assert!(Limits::parse("people=3").is_err());
    }

    #[test]
    fn months_are_utc_calendar_months() {
        assert_eq!(month_of(0), "1970-01");
        assert_eq!(month_of(1_709_251_199_000), "2024-02"); // 2024-02-29T23:59:59Z
        assert_eq!(month_of(1_709_251_200_000), "2024-03");
        assert_eq!(next_month_start("2026-12"), "2027-01-01");
        assert_eq!(next_month_start("2026-10"), "2026-11-01");
        assert!(valid_month("2026-10") && !valid_month("2026-13") && !valid_month("26-10") && !valid_month("2026/10"));
    }

    #[test]
    fn usage_is_recorded_once_and_storage_keeps_its_peak() {
        let r = Registry::open("memory").unwrap();
        assert!(r.record_active_agent("lumen", "2026-10", "agent/a", 5).unwrap());
        assert!(!r.record_active_agent("lumen", "2026-10", "agent/a", 9).unwrap());
        r.record_active_agent("lumen", "2026-10", "agent/b", 7).unwrap();
        r.record_active_agent("lumen", "2026-11", "agent/c", 7).unwrap();
        assert_eq!(r.active_agents("lumen", "2026-10").unwrap(), vec![("agent/a".into(), 5), ("agent/b".into(), 7)]);
        assert_eq!(r.count_active_agents("lumen", "2026-11").unwrap(), 1);

        let key = RepoKey::parse("lumen/fleet").unwrap();
        let at = 1_790_000_000_000; // 2026-09
        r.record_repo_size(&key, 900, at).unwrap();
        r.record_repo_size(&RepoKey::parse("lumen/web").unwrap(), 100, at).unwrap();
        r.record_repo_size(&key, 400, at + 1).unwrap();
        assert_eq!(r.org_storage("lumen").unwrap().0, 500);
        assert_eq!(r.peak_storage("lumen", &month_of(at)).unwrap(), 1000, "the month's highest total stays");
    }

    #[test]
    fn limits_fall_back_to_defaults_and_suspension_keeps_them() {
        let r = Registry::open("memory").unwrap();
        let defaults = Limits { seats: Some(2), plan_label: "Default".into(), ..Default::default() };
        assert_eq!(r.limits_for("lumen", None).unwrap(), Limits::default(), "no defaults: unlimited");
        assert_eq!(r.limits_for("lumen", Some(&defaults)).unwrap(), defaults);
        r.set_suspended("lumen", true, &defaults).unwrap();
        let l = r.limits_for("lumen", None).unwrap();
        assert!(l.suspended && l.seats == Some(2), "{l:?}");
        r.set_org_limits("lumen", &Limits { active_agents: Some(50), ..Default::default() }).unwrap();
        let l = r.limits_for("lumen", Some(&defaults)).unwrap();
        assert!(
            l.suspended && l.seats.is_none() && l.active_agents == Some(50),
            "limits replaced, suspension kept: {l:?}"
        );
        r.set_suspended("lumen", false, &defaults).unwrap();
        assert!(!r.limits_for("lumen", None).unwrap().suspended);
    }

    #[test]
    fn only_landed_changes_count() {
        use prost::Message;
        let frame = |m: &[u8]| [&[0u8, 0, 0, 0, m.len() as u8][..], m].concat();
        let ok = {
            let mut t = http::HeaderMap::new();
            t.insert("grpc-status", "0".parse().unwrap());
            t
        };
        let none = http::HeaderMap::new();
        let land = pando_proto::LandResponse { landed: vec![1; 32], ..Default::default() }.encode_to_vec();
        assert!(landed("/pando.v1.Land/Land", &none, Some(&ok), &frame(&land)));
        let mut refused = http::HeaderMap::new();
        refused.insert("grpc-status", "9".parse().unwrap());
        assert!(!landed("/pando.v1.Land/Land", &refused, None, &[]), "a refused land is an error status");
        let swapped = pando_proto::CasResponse { swapped: true }.encode_to_vec();
        let lost = pando_proto::CasResponse { swapped: false }.encode_to_vec();
        assert!(landed("/pando.v1.Refs/CompareAndSwap", &none, Some(&ok), &frame(&swapped)));
        assert!(!landed("/pando.v1.Refs/CompareAndSwap", &none, Some(&ok), &frame(&lost)), "a lost swap moved nothing");
    }
}
