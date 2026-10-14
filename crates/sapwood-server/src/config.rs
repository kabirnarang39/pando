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
use sapwood_adapters::cache::{BoundedCache, CachedDerived, CachedLog, Tiered};
use sapwood_adapters::cloud::CloudObjects;
use sapwood_adapters::grpc::GrpcObjects;
use sapwood_adapters::memory::MemoryStore;
use sapwood_adapters::postgres::PostgresStore;
use sapwood_adapters::sqlite::SqliteStore;
use sapwood_core::Repo;
use sapwood_core::ports::ObjectStore;

pub async fn open_repo(metadata: &str, objects: &str) -> anyhow::Result<Repo> {
    let mut repo = if metadata == "memory" {
        let m = Arc::new(MemoryStore::new());
        Repo { objects: m.clone(), log: m.clone(), refs: m.clone(), derived: m }
    } else if let Some(path) = metadata.strip_prefix("sqlite://") {
        if let Some(dir) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let s = Arc::new(SqliteStore::open(path)?);
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
    // Everything below is immutable, so memory caches in front of any backend are always
    // coherent: hot objects (trees, recent blobs), changesets, derived data.
    repo.objects = Arc::new(Tiered::new(Arc::new(BoundedCache::new(256 << 20)), repo.objects));
    repo.log = Arc::new(CachedLog::new(repo.log, 128 << 20));
    repo.derived = Arc::new(CachedDerived::new(repo.derived, 64 << 20));
    Ok(repo)
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
    let disk = Arc::new(CloudObjects::from_url(&sapwood_adapters::cloud::file_url(std::path::Path::new(dir))?)?);
    let disk_tier = Arc::new(Tiered::new(disk, up));
    Ok(Arc::new(Tiered::new(Arc::new(BoundedCache::new(memory_bytes)), disk_tier)))
}
