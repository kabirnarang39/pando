//! Composition root: turns URLs into adapters. This is the only place that knows which
//! concrete infrastructure is in use.
//!
//! Metadata (`--metadata`): `memory`, `sqlite:///abs/path.db` or `sqlite://rel/path.db`,
//! `postgres://user:pass@host/db`.
//! Objects (`--objects`): `metadata` (store objects in the metadata database), or any
//! object-store URL: `file:///path`, `s3://bucket/prefix`, `gs://bucket`, `az://container`,
//! `memory:///`.

use std::sync::Arc;

use anyhow::{Context, bail};
use pando_adapters::cache::{BoundedCache, CachedDerived, CachedLog, Tiered};
use pando_adapters::cloud::CloudObjects;
use pando_adapters::delta::DeltaObjects;
use pando_adapters::grpc::GrpcObjects;
use pando_adapters::memory::MemoryStore;
use pando_adapters::postgres::PostgresStore;
use pando_adapters::sqlite::SqliteStore;
use pando_core::Repo;
use pando_core::ports::ObjectStore;

/// A composed repository plus direct handles needed by maintenance jobs.
pub struct Storage {
    pub repo: Repo,
    /// The durable stores without decorators or caches (for copying a repository).
    pub raw: Repo,
    /// The durable object store without read decorators or caches (for compaction).
    pub raw_objects: Arc<dyn ObjectStore>,
    /// Present when metadata lives in SQLite, so maintenance can VACUUM.
    pub sqlite: Option<Arc<SqliteStore>>,
}

pub async fn open_repo(metadata: &str, objects: &str) -> anyhow::Result<Repo> {
    Ok(open_storage(metadata, objects).await?.repo)
}

pub async fn open_storage(metadata: &str, objects: &str) -> anyhow::Result<Storage> {
    let mut sqlite = None;
    let mut repo = if metadata == "memory" {
        let m = Arc::new(MemoryStore::new());
        Repo { objects: m.clone(), log: m.clone(), refs: m.clone(), derived: m }
    } else if let Some(path) = metadata.strip_prefix("sqlite://") {
        if let Some(dir) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let s = Arc::new(SqliteStore::open(path)?);
        sqlite = Some(s.clone());
        Repo { objects: s.clone(), log: s.clone(), refs: s.clone(), derived: s }
    } else if metadata.starts_with("postgres://") || metadata.starts_with("postgresql://") {
        let s = Arc::new(PostgresStore::connect(metadata, 32).await?);
        Repo { objects: s.clone(), log: s.clone(), refs: s.clone(), derived: s }
    } else {
        bail!("unsupported metadata URL `{metadata}` (memory | sqlite://PATH | postgres://...)");
    };
    if objects != "metadata" {
        repo.objects = Arc::new(CloudObjects::from_url(objects)?);
    }
    let raw_objects = repo.objects.clone();
    let raw = repo.clone();
    // Reads always resolve delta-encoded objects (written by `pandod compact`) back to
    // ordinary verified payloads, so nothing above this layer ever sees a delta.
    repo.objects = Arc::new(DeltaObjects::new(raw_objects.clone()));
    // Everything below is immutable, so memory caches in front of any backend are always
    // coherent: hot objects (trees, recent blobs), changesets, derived data.
    repo.objects = Arc::new(Tiered::new(Arc::new(BoundedCache::new(256 << 20)), repo.objects));
    repo.log = Arc::new(CachedLog::new(repo.log, 128 << 20));
    repo.derived = Arc::new(CachedDerived::new(repo.derived, 64 << 20));
    Ok(Storage { repo, raw, raw_objects, sqlite })
}

/// Regional cache: memory only, read-through to `upstream`.
pub fn cache_store(upstream: &str, token: Option<&str>, capacity_bytes: u64) -> anyhow::Result<Arc<dyn ObjectStore>> {
    let up = Arc::new(GrpcObjects::connect(upstream, token)?);
    Ok(Arc::new(Tiered::new(Arc::new(BoundedCache::new(capacity_bytes)), up)))
}

/// Node daemon: memory -> local disk -> upstream. Disk survives restarts so a node
/// starts warm.
pub fn daemon_store(
    upstream: &str,
    token: Option<&str>,
    dir: &str,
    memory_bytes: u64,
) -> anyhow::Result<Arc<dyn ObjectStore>> {
    let up = Arc::new(GrpcObjects::connect(upstream, token)?);
    std::fs::create_dir_all(dir)?;
    let disk = Arc::new(CloudObjects::from_url(&pando_adapters::cloud::file_url(std::path::Path::new(dir))?)?);
    let disk_tier = Arc::new(Tiered::new(disk, up));
    Ok(Arc::new(Tiered::new(Arc::new(BoundedCache::new(memory_bytes)), disk_tier)))
}

/// Hide credentials in a URL before logging it: `postgres://u:pw@h/db` -> `postgres://***@h/db`.
pub fn redact(url: &str) -> String {
    match (url.find("://"), url.rfind('@')) {
        (Some(scheme), Some(at)) if at > scheme => format!("{}://***{}", &url[..scheme], &url[at..]),
        _ => url.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn redacts_credentials() {
        assert_eq!(super::redact("postgres://u:secret@db:5432/x"), "postgres://***@db:5432/x");
        assert_eq!(super::redact("postgres://u:p@ss@db/x"), "postgres://***@db/x");
        assert_eq!(super::redact("sqlite://./a.db"), "sqlite://./a.db");
        assert_eq!(super::redact("s3://bucket/prefix"), "s3://bucket/prefix");
    }
}
