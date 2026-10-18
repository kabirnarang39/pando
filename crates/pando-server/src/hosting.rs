//! Hosting many repositories on one server.
//!
//! Each repository gets its own storage, from templates such as
//! `sqlite:///var/lib/pando/repos/{repo}.db`, and its own set of services (including its own
//! landing queues), opened the first time it is used. A request names its repository with the
//! `x-pando-repo: OWNER/NAME` header; a request without it reaches the server's default
//! repository, so single-repository clients and servers are unchanged. Which repositories exist
//! is kept in a small registry database.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, bail};
use pando_core::Repo;
use tokio::sync::OnceCell;
use tonic::Status;
use tonic::codegen::{BoxFuture, Bytes, StdError, http};
use tonic::service::Routes;
use tower::{Layer, Service, ServiceExt};

/// The request header that names the repository a call is about.
pub const REPO_HEADER: &str = "x-pando-repo";

/// `OWNER/NAME`, each 1–100 characters of letters, digits, `.`, `_` and `-`, not starting with
/// `.` (so names are safe in paths, URLs and database identifiers).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoKey {
    pub owner: String,
    pub name: String,
}

pub(crate) fn valid_part(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && !s.starts_with('.')
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

impl RepoKey {
    pub fn new(owner: &str, name: &str) -> Result<Self, Status> {
        if !valid_part(owner) || !valid_part(name) {
            return Err(Status::invalid_argument(format!(
                "`{owner}/{name}` is not a valid repository name: use letters, digits, `.`, `_` and `-` (up to 100 each)"
            )));
        }
        Ok(Self { owner: owner.to_owned(), name: name.to_owned() })
    }

    pub fn parse(s: &str) -> Result<Self, Status> {
        match s.split_once('/') {
            Some((owner, name)) if !name.contains('/') => Self::new(owner, name),
            _ => Err(Status::invalid_argument(format!("`{s}` is not OWNER/NAME"))),
        }
    }

    /// A form safe as a single path segment or identifier: `owner--name`.
    pub fn id(&self) -> String {
        format!("{}--{}", self.owner, self.name)
    }
}

impl std::fmt::Display for RepoKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// Storage URLs for each repository: `{repo}` becomes `owner/name`, `{repo_id}` becomes
/// `owner--name`.
#[derive(Debug, Clone)]
pub struct Templates {
    pub metadata: String,
    pub objects: String,
}

impl Templates {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.metadata.contains("{repo}") && !self.metadata.contains("{repo_id}") {
            bail!("--repos-metadata must contain {{repo}} or {{repo_id}}, so each repository gets its own database");
        }
        if self.objects != "metadata" && !self.objects.contains("{repo}") && !self.objects.contains("{repo_id}") {
            bail!("--repos-objects must contain {{repo}} or {{repo_id}}, or be `metadata`");
        }
        Ok(())
    }

    pub fn render(&self, key: &RepoKey) -> (String, String) {
        let r = |t: &str| t.replace("{repo}", &key.to_string()).replace("{repo_id}", &key.id());
        (r(&self.metadata), r(&self.objects))
    }
}

/// One repository as the registry records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRecord {
    pub key: RepoKey,
    pub description: String,
    pub visibility: i32,
    pub default_branch: String,
    pub created_at_ms: i64,
    pub archived: bool,
    pub topics: Vec<String>,
}

/// The registry's connection. A panic while it is held must not take every later request down
/// with it (M5): a poisoned lock is recovered and any transaction the panic left open is rolled
/// back. `lock()` keeps `Mutex`'s signature so callers read `db.lock().unwrap()` as before.
pub struct Db(Mutex<rusqlite::Connection>);

impl Db {
    pub fn lock(&self) -> Result<std::sync::MutexGuard<'_, rusqlite::Connection>, std::convert::Infallible> {
        Ok(self.0.lock().unwrap_or_else(|poisoned| {
            self.0.clear_poison();
            let conn = poisoned.into_inner();
            if !conn.is_autocommit() {
                let _ = conn.execute_batch("ROLLBACK");
            }
            tracing::warn!("recovered the registry connection after a panic");
            conn
        }))
    }
}

/// Ordered registry schema changes, applied once each and recorded in `schema_version`
/// (L13). Append only: never edit, reorder or remove an entry.
const MIGRATIONS: &[&str] = &[
    // 1: listing figures, so listing repositories never opens their storage (M6).
    "CREATE TABLE IF NOT EXISTS repository_activity (
       owner TEXT NOT NULL,
       name TEXT NOT NULL,
       tip TEXT NOT NULL,
       tip_at_ms INTEGER NOT NULL,
       lands_24h INTEGER NOT NULL,
       computed_at_ms INTEGER NOT NULL,
       PRIMARY KEY (owner, name)
     );",
    // 2: repositories taken down by the operator (`pandod repo disable`).
    "CREATE TABLE IF NOT EXISTS disabled_repositories (
       owner TEXT NOT NULL,
       name TEXT NOT NULL,
       reason TEXT NOT NULL,
       disabled_by TEXT NOT NULL,
       at_ms INTEGER NOT NULL,
       PRIMARY KEY (owner, name)
     );",
    // 3: plans, usage and limits (ADR-035).
    crate::plans::SCHEMA,
];

/// Tables holding rows about one repository (columns `owner`, `name`), cleared when it is
/// deleted.
const REPOSITORY_TABLES: &[&str] = &[
    "grants",
    "change_requests",
    "reviews",
    "comments",
    "checks",
    "protections",
    "events",
    "issues",
    "repository_activity",
    "disabled_repositories",
    "repo_sizes",
];

/// Which repositories exist. SQLite, one small table; writes are rare (creating a repository).
pub struct Registry {
    pub(crate) db: Db,
    /// Answers to the reads every request makes (a repository, a role, branch protection),
    /// dropped by any write that could change them (ADR-029).
    cache: quick_cache::sync::Cache<String, (Instant, Arc<dyn std::any::Any + Send + Sync>)>,
    /// Bumped by every invalidation, so an answer read before a write is never cached after it.
    generation: std::sync::atomic::AtomicU64,
    /// How organisations' plans are enforced (ADR-035), set by [`Hosting::new`].
    pub(crate) plan_policy: std::sync::OnceLock<crate::plans::Policy>,
}

/// How long a cached answer may be used: writes in this process clear the cache at once; this
/// bounds how long another replica's write can go unseen.
const CACHE_TTL: Duration = Duration::from_secs(5);

impl Registry {
    /// `sqlite:///abs/path.db`, `sqlite://rel/path.db` or `memory`.
    pub fn open(url: &str) -> anyhow::Result<Self> {
        let conn = if url == "memory" {
            rusqlite::Connection::open_in_memory()?
        } else {
            let path = url
                .strip_prefix("sqlite://")
                .with_context(|| format!("unsupported --platform `{url}` (sqlite://PATH | memory)"))?;
            if let Some(dir) = Path::new(path).parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)?;
            }
            rusqlite::Connection::open(path)?
        };
        // FULL: an acknowledged write (a token, a review, an approval) survives power loss (M4).
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS repositories (
               owner TEXT NOT NULL,
               name TEXT NOT NULL,
               description TEXT NOT NULL DEFAULT '',
               visibility INTEGER NOT NULL DEFAULT 1,
               default_branch TEXT NOT NULL DEFAULT 'main',
               created_at_ms INTEGER NOT NULL,
               archived INTEGER NOT NULL DEFAULT 0,
               topics TEXT NOT NULL DEFAULT '',
               PRIMARY KEY (owner, name)
             );",
        )?;
        let reg = Self {
            db: Db(Mutex::new(conn)),
            cache: quick_cache::sync::Cache::new(100_000),
            generation: Default::default(),
            plan_policy: Default::default(),
        };
        reg.migrate()?;
        Ok(reg)
    }

    /// `load()`, or its answer from the last few seconds.
    pub(crate) fn cached<T: Clone + Send + Sync + 'static>(
        &self,
        key: String,
        load: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        if let Some((at, v)) = self.cache.get(&key)
            && at.elapsed() < CACHE_TTL
            && let Some(v) = v.downcast_ref::<T>()
        {
            return Ok(v.clone());
        }
        use std::sync::atomic::Ordering::SeqCst;
        let generation = self.generation.load(SeqCst);
        let v = load()?;
        self.cache.insert(key.clone(), (Instant::now(), Arc::new(v.clone())));
        // A write committed while we read: what we read may predate it, so don't keep it.
        if self.generation.load(SeqCst) != generation {
            self.cache.remove(&key);
        }
        Ok(v)
    }

    /// Forgets every cached answer; called after (never before) every write that changes who
    /// may do what, so a concurrent read can't re-cache the old answer.
    pub(crate) fn invalidate(&self) {
        self.generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.cache.clear();
    }

    /// Brings the schema up to date: accounts, organisations, teams, grants and tokens.
    pub(crate) fn migrate(&self) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        db.execute_batch(crate::directory::SCHEMA)?;
        // Columns added since a table was first made (portable: no IF NOT EXISTS on columns).
        if db.prepare("SELECT accepted_terms_at_ms FROM accounts LIMIT 0").is_err() {
            db.execute_batch("ALTER TABLE accounts ADD COLUMN accepted_terms_at_ms INTEGER NOT NULL DEFAULT 0")?;
        }
        db.execute_batch(crate::features::reviews::store::SCHEMA)?;
        db.execute_batch(crate::webhooks::SCHEMA)?;
        db.execute_batch(crate::features::issues::SCHEMA)?;
        db.execute_batch(crate::login::SCHEMA)?;
        drop(db);
        self.apply_migrations()
    }

    fn apply_migrations(&self) -> anyhow::Result<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);")?;
        let current: i64 = tx.query_row("SELECT COALESCE(MAX(version), 0) FROM schema_version", [], |r| r.get(0))?;
        for (i, m) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            tx.execute_batch(m).with_context(|| format!("registry migration {}", i + 1))?;
            tx.execute("INSERT INTO schema_version (version) VALUES (?1)", [i as i64 + 1])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Why the operator disabled the repository, if they did (cached like every access answer,
    /// so a `pandod repo disable` from another process takes effect within seconds).
    pub fn disabled(&self, key: &RepoKey) -> anyhow::Result<Option<String>> {
        self.cached(format!("disabled:{key}"), || {
            let db = self.db.lock().unwrap();
            let mut st =
                db.prepare_cached("SELECT reason FROM disabled_repositories WHERE owner = ?1 AND name = ?2")?;
            let mut rows = st.query_map((&key.owner, &key.name), |r| r.get::<_, String>(0))?;
            Ok(rows.next().transpose()?)
        })
    }

    /// Disables the repository (`Some(reason)`) or enables it again (`None`); false if nothing
    /// changed.
    pub fn set_disabled(&self, key: &RepoKey, reason: Option<&str>, by: &str) -> anyhow::Result<bool> {
        let db = self.db.lock().unwrap();
        let n = match reason {
            Some(reason) => db.execute(
                "INSERT INTO disabled_repositories (owner, name, reason, disabled_by, at_ms) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (owner, name) DO UPDATE SET reason = excluded.reason, disabled_by = excluded.disabled_by, at_ms = excluded.at_ms",
                rusqlite::params![key.owner, key.name, reason, by, now_ms()],
            )?,
            None => db.execute("DELETE FROM disabled_repositories WHERE owner = ?1 AND name = ?2", (&key.owner, &key.name))?,
        };
        self.invalidate();
        Ok(n > 0)
    }

    /// Removes the repository and every row about it (reviews, issues, webhooks, grants...);
    /// false if it did not exist. Its storage is the caller's to remove.
    pub fn delete_repository(&self, key: &RepoKey) -> anyhow::Result<bool> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute(
            "DELETE FROM deliveries WHERE hook IN (SELECT id FROM webhooks WHERE owner = ?1 AND name = ?2)",
            (&key.owner, &key.name),
        )?;
        for t in REPOSITORY_TABLES.iter().chain(&["webhooks"]) {
            // Table names come from the constant list above, never from input.
            tx.execute(&format!("DELETE FROM {t} WHERE owner = ?1 AND name = ?2"), (&key.owner, &key.name))?;
        }
        let n = tx.execute("DELETE FROM repositories WHERE owner = ?1 AND name = ?2", (&key.owner, &key.name))?;
        tx.commit()?;
        self.invalidate();
        Ok(n == 1)
    }

    /// Listing figures last computed for the repository.
    fn activity(&self, key: &RepoKey) -> anyhow::Result<Option<RepoStats>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(
            "SELECT tip, tip_at_ms, lands_24h FROM repository_activity WHERE owner = ?1 AND name = ?2",
        )?;
        let mut rows = st.query_map((&key.owner, &key.name), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })?;
        Ok(rows.next().transpose()?.map(|(tip, tip_at_ms, lands)| RepoStats {
            tip: tip.parse().ok(),
            tip_at_ms,
            // Not recomputed while the repository is closed: a day-old count is no count.
            lands_24h: if now_ms() - tip_at_ms < 86_400_000 { lands as u32 } else { 0 },
        }))
    }

    fn record_activity(&self, key: &RepoKey, s: &RepoStats) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO repository_activity (owner, name, tip, tip_at_ms, lands_24h, computed_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (owner, name) DO UPDATE SET tip = excluded.tip, tip_at_ms = excluded.tip_at_ms,
               lands_24h = excluded.lands_24h, computed_at_ms = excluded.computed_at_ms",
            rusqlite::params![
                key.owner,
                key.name,
                s.tip.map(|t| t.to_hex()).unwrap_or_default(),
                s.tip_at_ms,
                s.lands_24h as i64,
                now_ms()
            ],
        )?;
        Ok(())
    }

    /// Readiness: the registry answers and its write lock can be taken.
    pub fn ready(&self) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        db.execute_batch("BEGIN IMMEDIATE; ROLLBACK;")?;
        let _: i64 = db.query_row("SELECT COUNT(*) FROM repositories", [], |r| r.get(0))?;
        Ok(())
    }

    fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RepoRecord> {
        let topics: String = r.get(7)?;
        Ok(RepoRecord {
            key: RepoKey { owner: r.get(0)?, name: r.get(1)? },
            description: r.get(2)?,
            visibility: r.get(3)?,
            default_branch: r.get(4)?,
            created_at_ms: r.get(5)?,
            archived: r.get::<_, i64>(6)? != 0,
            topics: topics.split(',').filter(|t| !t.is_empty()).map(str::to_owned).collect(),
        })
    }

    pub fn get(&self, key: &RepoKey) -> anyhow::Result<Option<RepoRecord>> {
        self.cached(format!("repo:{key}"), || self.get_uncached(key))
    }

    fn get_uncached(&self, key: &RepoKey) -> anyhow::Result<Option<RepoRecord>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(
            "SELECT owner, name, description, visibility, default_branch, created_at_ms, archived, topics
             FROM repositories WHERE owner = ?1 AND name = ?2",
        )?;
        let mut rows = st.query_map((&key.owner, &key.name), Self::row)?;
        Ok(rows.next().transpose()?)
    }

    /// Every repository, or those of one owner, ordered by owner then name.
    pub fn list(&self, owner: Option<&str>) -> anyhow::Result<Vec<RepoRecord>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(
            "SELECT owner, name, description, visibility, default_branch, created_at_ms, archived, topics
             FROM repositories WHERE ?1 IS NULL OR owner = ?1 ORDER BY owner, name",
        )?;
        let rows = st.query_map([owner], Self::row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Replaces a repository's settings (everything but its name and creation time).
    pub fn update(&self, rec: &RepoRecord) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "UPDATE repositories SET description = ?3, visibility = ?4, default_branch = ?5, archived = ?6, topics = ?7
             WHERE owner = ?1 AND name = ?2",
            (
                &rec.key.owner,
                &rec.key.name,
                &rec.description,
                rec.visibility,
                &rec.default_branch,
                rec.archived as i64,
                rec.topics.join(","),
            ),
        )?;
        self.invalidate();
        Ok(())
    }

    /// Adds a repository; false if it already exists.
    pub fn create(&self, rec: &RepoRecord) -> anyhow::Result<bool> {
        let db = self.db.lock().unwrap();
        let n = db.execute(
            "INSERT OR IGNORE INTO repositories (owner, name, description, visibility, default_branch, created_at_ms, archived, topics)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            (
                &rec.key.owner,
                &rec.key.name,
                &rec.description,
                rec.visibility,
                &rec.default_branch,
                rec.created_at_ms,
                rec.archived as i64,
                rec.topics.join(","),
            ),
        )?;
        self.invalidate();
        Ok(n == 1)
    }
}

/// One repository's services, as [`BuildRoutes`] makes them.
pub struct RepoServices {
    /// Everything clients call on the repository.
    pub routes: Routes,
    /// Landing for change requests: their review is the gate, so it skips branch protection.
    pub review_land: Option<crate::features::land::Land>,
}

/// Builds one repository's services from its name and storage.
pub type BuildRoutes = Arc<dyn Fn(&RepoKey, Repo) -> RepoServices + Send + Sync>;

struct Hosted {
    repo: Repo,
    services: RepoServices,
    /// References to `repo.refs` held by the repository's own services: more means a call (a
    /// land, a review, a streaming read) is still using it.
    baseline: usize,
}

type Cell = Arc<OnceCell<Hosted>>;

/// Opened repositories and when each was last asked for.
struct OpenRepos {
    map: HashMap<RepoKey, (Cell, Instant)>,
    swept: Instant,
}

/// How many repositories stay open at once, and how long an unused one stays open (M7).
#[derive(Debug, Clone, Copy)]
pub struct OpenLimits {
    pub max_open: usize,
    pub idle: Duration,
}

impl Default for OpenLimits {
    fn default() -> Self {
        Self { max_open: 256, idle: Duration::from_secs(600) }
    }
}

fn in_use(cell: &Cell) -> bool {
    Arc::strong_count(cell) > 1 || cell.get().is_some_and(|h| Arc::strong_count(&h.repo.refs) > h.baseline)
}

impl OpenRepos {
    /// Closes repositories nobody is using that have been idle too long, then the least
    /// recently used unused ones beyond the limit. A repository in use is never closed, so a
    /// repository is never open twice. ponytail: O(open) scan, at most every few seconds or
    /// when over the limit; fine for hundreds open.
    fn sweep(&mut self, limits: OpenLimits) {
        if self.map.len() <= limits.max_open && self.swept.elapsed() < Duration::from_secs(5) {
            return;
        }
        self.swept = Instant::now();
        self.map.retain(|key, (cell, at)| {
            let keep = in_use(cell) || at.elapsed() < limits.idle;
            if !keep {
                tracing::info!(repo = %key, "closed idle repository");
            }
            keep
        });
        if self.map.len() > limits.max_open {
            let mut idle: Vec<(Instant, RepoKey)> =
                self.map.iter().filter(|(_, (c, _))| !in_use(c)).map(|(k, (_, at))| (*at, k.clone())).collect();
            idle.sort();
            for (_, key) in idle.into_iter().take(self.map.len() - limits.max_open) {
                self.map.remove(&key);
                tracing::info!(repo = %key, "closed least recently used repository");
            }
        }
    }
}

/// Activity figures for listings, cached briefly: they walk history.
#[derive(Debug, Clone, Copy, Default)]
pub struct RepoStats {
    pub tip: Option<pando_core::Id>,
    pub tip_at_ms: i64,
    pub lands_24h: u32,
}

/// The repositories this server hosts, opened on first use.
pub struct Hosting {
    pub(crate) templates: Templates,
    pub registry: Arc<Registry>,
    build: BuildRoutes,
    open: Mutex<OpenRepos>,
    limits: OpenLimits,
    stats: Mutex<HashMap<RepoKey, (Instant, RepoStats)>>,
    /// Who may do what.
    pub access: Arc<crate::permissions::Access>,
    /// The sign-in pages, when served (`--login-listen`): invitations link to them and use their mail.
    pub login: std::sync::OnceLock<Arc<crate::login::Login>>,
    /// Agents already counted as active this month (ADR-035).
    pub(crate) meter: crate::plans::Meter,
}

impl Hosting {
    pub fn new(
        templates: Templates,
        registry: Arc<Registry>,
        build: BuildRoutes,
        access: Arc<crate::permissions::Access>,
    ) -> anyhow::Result<Self> {
        templates.validate()?;
        let _ = registry.plan_policy.set(crate::plans::Policy {
            enforce: access.multi_tenant,
            defaults: access.default_org_limits.clone(),
            billing_url: access.billing_url.clone(),
        });
        Ok(Self {
            templates,
            registry,
            build,
            open: Mutex::new(OpenRepos { map: HashMap::new(), swept: Instant::now() }),
            limits: OpenLimits::default(),
            stats: Mutex::new(HashMap::new()),
            access,
            login: std::sync::OnceLock::new(),
            meter: Default::default(),
        })
    }

    /// Sets how many repositories stay open and for how long.
    pub fn with_open_limits(mut self, limits: OpenLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Repositories open now.
    pub fn open_count(&self) -> usize {
        self.open.lock().unwrap_or_else(std::sync::PoisonError::into_inner).map.len()
    }

    /// The repository's record, if `who` (limited by `scope`) has at least `need` on it.
    pub fn authorize(
        &self,
        key: &RepoKey,
        who: &pando_core::acl::Principal,
        scope: Option<&crate::auth::TokenScope>,
        need: pando_proto::Role,
    ) -> Result<RepoRecord, Status> {
        let rec = self
            .registry
            .get(key)
            .map_err(crate::status::internal)?
            .ok_or_else(|| Status::not_found(format!("no repository `{key}` on this server")))?;
        self.access.check(&self.registry, who, scope, &rec, need)?;
        Ok(rec)
    }

    async fn hosted(&self, key: &RepoKey) -> Result<Cell, Status> {
        let exists = self.registry.get(key).map_err(crate::status::internal)?.is_some();
        if !exists {
            return Err(Status::not_found(format!("no repository `{key}` on this server")));
        }
        let cell = {
            let mut open = self.open.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = open.map.entry(key.clone()).or_insert_with(|| (Default::default(), Instant::now()));
            entry.1 = Instant::now();
            let cell = entry.0.clone();
            open.sweep(self.limits);
            cell
        };
        cell.get_or_try_init(|| async {
            let (metadata, objects) = self.templates.render(key);
            if let Some(path) = metadata.strip_prefix("sqlite://")
                && let Some(dir) = Path::new(path).parent()
            {
                std::fs::create_dir_all(dir)
                    .map_err(|e| crate::status::internal(format!("creating storage for `{key}`: {e}")))?;
            }
            let repo = crate::config::open_repo(&metadata, &objects).await.map_err(|e| {
                tracing::error!(repo = %key, error = %format!("{e:#}"), "opening storage");
                Status::unavailable(format!("storage for `{key}` is unavailable; try again shortly"))
            })?;
            // Every branch move becomes a `push` webhook event.
            let refs = Arc::new(crate::webhooks::NotifyingRefs {
                inner: repo.refs.clone(),
                log: repo.log.clone(),
                registry: self.registry.clone(),
                key: key.clone(),
            });
            let repo = Repo { refs, ..repo };
            tracing::info!(repo = %key, "opened repository");
            let services = (self.build)(key, repo.clone());
            Ok::<_, Status>(Hosted { baseline: Arc::strong_count(&repo.refs), services, repo })
        })
        .await?;
        Ok(cell)
    }

    /// The services for `key`, opening its storage the first time, and a handle that keeps the
    /// repository open while held.
    async fn routes(&self, key: &RepoKey) -> Result<(Routes, Cell), Status> {
        let cell = self.hosted(key).await?;
        Ok((cell.get().expect("initialised above").services.routes.clone(), cell))
    }

    /// The repository's storage and its change-request landing, opening it the first time.
    pub async fn open_repo(&self, key: &RepoKey) -> Result<(Repo, Option<crate::features::land::Land>), Status> {
        let cell = self.hosted(key).await?;
        let h = cell.get().expect("initialised above");
        Ok((h.repo.clone(), h.services.review_land.clone()))
    }

    /// Creates the repository's storage and records it. Idempotent for an existing repository
    /// with the same name: returns false.
    pub async fn create(&self, rec: RepoRecord) -> Result<bool, Status> {
        let created = self.registry.create(&rec).map_err(crate::status::internal)?;
        // Open now so a storage problem surfaces at creation, not on first use.
        self.hosted(&rec.key).await?;
        Ok(created)
    }

    /// Tip, its time, and changes landed on the default branch in the last day. Cached per tip
    /// for up to 30 s: a landing always shows at once, and the day-long walk runs at most twice a
    /// minute per repository. A repository that is not open is not opened for this (a listing
    /// would open every repository): its figures come from the registry, as last computed.
    pub async fn stats(&self, rec: &RepoRecord) -> RepoStats {
        let open = self
            .open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .map
            .get(&rec.key)
            .filter(|(c, _)| c.initialized())
            .map(|(c, _)| c.clone());
        let Some(cell) = open else {
            return self.registry.activity(&rec.key).ok().flatten().unwrap_or_default();
        };
        let repo = &cell.get().expect("initialised").repo;
        let tip = repo.refs.get(&rec.default_branch).await.ok().flatten();
        if let Some((at, s)) = self.stats.lock().unwrap().get(&rec.key)
            && s.tip == tip
            && at.elapsed() < Duration::from_secs(30)
        {
            return *s;
        }
        let s = compute_stats(repo, &rec.default_branch).await.unwrap_or_default();
        self.stats.lock().unwrap().insert(rec.key.clone(), (Instant::now(), s));
        if let Err(e) = self.registry.record_activity(&rec.key, &s) {
            tracing::warn!(repo = %rec.key, error = %e, "recording listing figures");
        }
        s
    }
}

async fn compute_stats(repo: &Repo, branch: &str) -> anyhow::Result<RepoStats> {
    use prost::Message;
    let Some(tip) = repo.refs.get(branch).await? else { return Ok(RepoStats::default()) };
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;
    let mut s = RepoStats { tip: Some(tip), ..Default::default() };
    let mut cur = Some(tip);
    // A day of history, walked along first parents; bounded so a listing stays cheap.
    for _ in 0..20_000 {
        let Some(id) = cur else { break };
        let Some(bytes) = repo.log.get(&id).await? else { break };
        let cs = pando_core::object::Changeset::decode(bytes.as_ref())?;
        if s.tip_at_ms == 0 {
            s.tip_at_ms = cs.timestamp_ms;
        }
        if cs.timestamp_ms < now - 86_400_000 {
            break;
        }
        s.lands_24h += 1;
        cur = cs.first_parent()?;
    }
    Ok(s)
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Routes calls that name a repository to that repository's services, once the caller may make
/// them. With hosting, a repository call that names none is refused; without hosting the header
/// is ignored (one-repository servers).
#[derive(Clone)]
pub struct HostingLayer(pub Option<Arc<Hosting>>);

impl<S> Layer<S> for HostingLayer {
    type Service = HostingService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        HostingService { inner, hosting: self.0.clone() }
    }
}

#[derive(Clone)]
pub struct HostingService<S> {
    inner: S,
    hosting: Option<Arc<Hosting>>,
}

impl<S, B> Service<http::Request<B>> for HostingService<S>
where
    S: Service<http::Request<B>, Response = http::Response<tonic::body::Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: tonic::codegen::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<StdError>,
{
    type Response = http::Response<tonic::body::Body>;
    type Error = S::Error;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        // Platform and review calls name their repository in the request itself.
        let path = req.uri().path();
        let central =
            ["/pando.v1.Platform/", "/pando.v1.Reviews/", "/pando.v1.Issues/"].iter().any(|p| path.starts_with(p));
        let Some(hosting) = self.hosting.clone().filter(|_| !central) else {
            return Box::pin(self.inner.call(req));
        };
        let named = req.headers().get(REPO_HEADER).map(|v| v.to_str().map(str::to_owned));
        let name = match named {
            Some(Ok(name)) => name,
            Some(Err(_)) => {
                return Box::pin(async {
                    Ok(Status::invalid_argument("`x-pando-repo` is not OWNER/NAME").into_http())
                });
            }
            // Calls that touch no repository; every other call on a server that hosts many must
            // name one, so the server's own repository is never reached unchecked.
            None if path.starts_with("/grpc.") || path == "/pando.v1.Access/WhoAmI" => {
                return Box::pin(self.inner.call(req));
            }
            None => {
                return Box::pin(async {
                    Ok(Status::not_found("this server hosts many repositories: name one (a URL ending in /OWNER/NAME)")
                        .into_http())
                });
            }
        };
        Box::pin(async move {
            let who = req
                .extensions()
                .get::<pando_core::acl::Principal>()
                .cloned()
                .unwrap_or_else(pando_core::acl::Principal::anonymous);
            let scope = req.extensions().get::<crate::auth::TokenScope>().cloned();
            let need = crate::permissions::required_role(req.uri().path());
            let path = req.uri().path().to_owned();
            let routes = match RepoKey::parse(&name).and_then(|key| {
                hosting.authorize(&key, &who, scope.as_ref(), need)?;
                // Plans and limits (ADR-035): writes only.
                if need != pando_proto::Role::Read {
                    crate::plans::check_write(&hosting, &key, &who, &path)?;
                }
                Ok(key)
            }) {
                Ok(key) => hosting.routes(&key).await.map(|r| (key, r)),
                Err(e) => Err(e),
            };
            match routes {
                // The response body (a streamed checkout, say) keeps the repository open until it
                // is sent.
                Ok((key, (routes, open))) => match routes.oneshot(req).await {
                    Ok(resp) => Ok(crate::plans::observe_land(&hosting, &key, &who, &path, resp).await.map(|body| {
                        use http_body_util::BodyExt;
                        tonic::body::Body::new(body.map_frame(move |f| {
                            let _ = &open;
                            f
                        }))
                    })),
                    Err(never) => match never {},
                },
                Err(status) => Ok(status.into_http()),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_in_paths_and_identifiers() {
        assert!(RepoKey::parse("lumen/fleet").is_ok());
        assert!(RepoKey::parse("a.b_c-d/x.y").is_ok());
        for bad in [
            "lumen",
            "lumen/",
            "/fleet",
            "a/b/c",
            "../etc",
            "a/..x",
            "a/.hidden",
            "a b/c",
            "a/b;drop",
            &format!("a/{}", "x".repeat(101)),
        ] {
            assert!(RepoKey::parse(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn templates_render_and_must_separate_repositories() {
        let t = Templates { metadata: "sqlite:///data/{repo}.db".into(), objects: "s3://bucket/{repo_id}".into() };
        let (m, o) = t.render(&RepoKey::parse("lumen/fleet").unwrap());
        assert_eq!(m, "sqlite:///data/lumen/fleet.db");
        assert_eq!(o, "s3://bucket/lumen--fleet");
        assert!(Templates { metadata: "sqlite:///data/one.db".into(), objects: "metadata".into() }.validate().is_err());
    }

    #[test]
    fn registry_creates_once_and_lists_by_owner() {
        let r = Registry::open("memory").unwrap();
        let rec = |o: &str, n: &str| RepoRecord {
            key: RepoKey::parse(&format!("{o}/{n}")).unwrap(),
            description: String::new(),
            visibility: 1,
            default_branch: "main".into(),
            created_at_ms: 1,
            archived: false,
            topics: vec!["rust".into()],
        };
        assert!(r.create(&rec("lumen", "fleet")).unwrap());
        assert!(!r.create(&rec("lumen", "fleet")).unwrap());
        assert!(r.create(&rec("lumen", "web")).unwrap());
        assert!(r.create(&rec("acme", "app")).unwrap());
        assert_eq!(r.list(Some("lumen")).unwrap().len(), 2);
        assert_eq!(r.list(None).unwrap().len(), 3);
        assert_eq!(r.get(&RepoKey::parse("lumen/fleet").unwrap()).unwrap().unwrap().topics, vec!["rust".to_string()]);
    }

    fn record(key: &str) -> RepoRecord {
        RepoRecord {
            key: RepoKey::parse(key).unwrap(),
            description: String::new(),
            visibility: 1,
            default_branch: "main".into(),
            created_at_ms: 1,
            archived: false,
            topics: vec![],
        }
    }

    #[test]
    fn schema_versions_durability_and_poison_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}/platform.db", dir.path().display());
        let version = |r: &Registry| -> i64 {
            r.db.lock().unwrap().query_row("SELECT MAX(version) FROM schema_version", [], |x| x.get(0)).unwrap()
        };
        let r = Registry::open(&url).unwrap();
        assert_eq!(version(&r), MIGRATIONS.len() as i64);
        let sync: i64 = r.db.lock().unwrap().query_row("PRAGMA synchronous", [], |x| x.get(0)).unwrap();
        assert_eq!(sync, 2, "FULL");
        drop(r);
        let r = Arc::new(Registry::open(&url).unwrap());
        assert_eq!(version(&r), MIGRATIONS.len() as i64, "each migration runs once");

        // A panic while holding the registry, mid-transaction, doesn't take the platform down.
        let r2 = r.clone();
        let _ = std::thread::spawn(move || {
            let db = r2.db.lock().unwrap();
            db.execute_batch("BEGIN; INSERT INTO repositories (owner, name, created_at_ms) VALUES ('a', 'b', 1);")
                .unwrap();
            panic!("boom");
        })
        .join();
        assert!(r.create(&record("lumen/fleet")).unwrap());
        assert!(r.get(&RepoKey::parse("a/b").unwrap()).unwrap().is_none(), "the unfinished write was rolled back");
    }

    #[test]
    fn deleting_a_repository_removes_its_rows() {
        let r = Registry::open("memory").unwrap();
        let rec = record("lumen/fleet");
        r.create(&rec).unwrap();
        r.set_grant(&rec.key, "kabir", pando_proto::Role::Write).unwrap();
        r.create_webhook(&rec.key, "https://ci.example/", &[], "", "kabir").unwrap();
        r.set_disabled(&rec.key, Some("x"), "ops").unwrap();
        assert!(r.delete_repository(&rec.key).unwrap());
        assert!(r.get(&rec.key).unwrap().is_none());
        assert!(r.webhooks(&rec.key).unwrap().is_empty());
        assert_eq!(r.held_role("kabir", &rec.key).unwrap(), pando_proto::Role::Unspecified);
        assert!(r.disabled(&rec.key).unwrap().is_none());
        assert!(!r.delete_repository(&rec.key).unwrap());
    }

    #[tokio::test]
    async fn idle_repositories_close_but_not_while_in_use_and_listings_open_none() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(Registry::open("memory").unwrap());
        let build: BuildRoutes = Arc::new(|_, repo| RepoServices {
            routes: crate::authority_routes(
                repo,
                Arc::new(pando_adapters::flags::SpecFlags::all_on()),
                Arc::new(pando_core::acl::AllowAllPolicy),
                Arc::new(pando_core::ports::NoChecks),
            ),
            review_land: None,
        });
        let templates = Templates {
            metadata: format!("sqlite://{}/{{repo}}.db", dir.path().display()),
            objects: "metadata".into(),
        };
        let h = Hosting::new(templates, registry.clone(), build, Default::default())
            .unwrap()
            .with_open_limits(OpenLimits { max_open: 1, idle: Duration::from_secs(600) });
        for k in ["lumen/a", "lumen/b"] {
            registry.create(&record(k)).unwrap();
        }
        let (a, b) = (RepoKey::parse("lumen/a").unwrap(), RepoKey::parse("lumen/b").unwrap());
        // `a` in use (a call holds its storage): opening `b` can't close it.
        let (held, _) = h.open_repo(&a).await.unwrap();
        h.open_repo(&b).await.unwrap();
        assert_eq!(h.open_count(), 2);
        drop(held);
        // Over the limit and `a` is idle now: the least recently used goes.
        h.open_repo(&b).await.unwrap();
        assert_eq!(h.open_count(), 1);

        // Listing figures never open a closed repository; they come from the registry.
        let s = h.stats(&record("lumen/a")).await;
        assert!(s.tip.is_none());
        assert_eq!(h.open_count(), 1);
    }
}
