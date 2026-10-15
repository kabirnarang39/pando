//! Prefetch slice (node daemon): warm the daemon's caches with a revision's files before
//! jobs ask for them, and keep them warm as the branch moves. Objects already cached cost
//! nothing, so periodic re-prefetch only fetches what changed.

use std::sync::Arc;
use std::time::{Duration, Instant};

use sapwood_core::acl::ReadScope;
use sapwood_core::chunked::{is_manifest, parse_manifest};
use sapwood_core::flags::{FeatureFlags, Flag, require};
use sapwood_core::object::{Kind, Mode, decode_object, payload_kind};
use sapwood_core::ports::ObjectStore;
use sapwood_core::{Error, Id, Result, tree};
use sapwood_proto::changesets_client::ChangesetsClient;
use sapwood_proto::{BearerAuth, ResolveRequest};
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, Endpoint};

/// Objects requested per round trip.
const BATCH: usize = 256;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PrefetchStats {
    pub trees: u64,
    pub files: u64,
    pub objects: u64,
    pub bytes: u64,
}

/// Fetch every tree, file and large-file chunk of `root` under `paths` (all when empty)
/// through `store`, which fills the store's local tiers as a side effect.
pub async fn warm(store: &dyn ObjectStore, root: Id, paths: &[String]) -> Result<PrefetchStats> {
    let want = if paths.is_empty() { ReadScope::All } else { ReadScope::Prefixes(paths.to_vec()) };
    let mut stats = PrefetchStats::default();
    let mut level = vec![(String::new(), root)];
    let mut blobs = Vec::new();
    while !level.is_empty() {
        let mut next = Vec::new();
        for chunk in level.chunks(BATCH) {
            let ids: Vec<Id> = chunk.iter().map(|(_, id)| *id).collect();
            for ((prefix, id), payload) in chunk.iter().zip(store.get_many(&ids).await?) {
                let payload = payload.ok_or_else(|| Error::not_found(format!("tree {id}")))?;
                stats.trees += 1;
                stats.bytes += payload.len() as u64;
                let (_, content) = decode_object(id, &payload)?;
                for e in tree::parse_tree(id, &content)?.entries {
                    let path = if prefix.is_empty() { e.name } else { format!("{prefix}/{}", e.name) };
                    let eid = Id::from_slice(&e.id)?;
                    if Mode::parse(e.mode)? == Mode::Dir {
                        if want.traversable(&path) {
                            next.push((path, eid));
                        }
                    } else if want.allows(&path) {
                        blobs.push(eid);
                    }
                }
            }
        }
        level = next;
    }
    stats.files = blobs.len() as u64;
    let mut chunks = Vec::new();
    for batch in blobs.chunks(BATCH) {
        for payload in store.get_many(batch).await?.into_iter().flatten() {
            stats.objects += 1;
            stats.bytes += payload.len() as u64;
            if is_manifest(&payload) {
                for c in parse_manifest(&payload)?.chunks {
                    chunks.push(Id::from_slice(&c.id)?);
                }
            } else if payload_kind(&payload)? != Kind::Blob {
                return Err(Error::Corrupt("prefetch expected a blob".into()));
            }
        }
    }
    for batch in chunks.chunks(BATCH) {
        for payload in store.get_many(batch).await?.into_iter().flatten() {
            stats.objects += 1;
            stats.bytes += payload.len() as u64;
        }
    }
    stats.objects += stats.trees;
    Ok(stats)
}

/// Resolves revisions on the authoritative server and warms a daemon's store.
pub struct Prefetcher {
    server: ChangesetsClient<InterceptedService<Channel, BearerAuth>>,
    store: Arc<dyn ObjectStore>,
    flags: Arc<dyn FeatureFlags>,
}

impl Prefetcher {
    pub fn new(
        server_url: &str,
        token: Option<&str>,
        store: Arc<dyn ObjectStore>,
        flags: Arc<dyn FeatureFlags>,
    ) -> Result<Self> {
        let channel = Endpoint::from_shared(server_url.to_owned())
            .map_err(|e| Error::invalid(format!("server URL: {e}")))?
            .connect_lazy();
        let auth = BearerAuth::new(token).map_err(|s| Error::invalid(s.message().to_owned()))?;
        Ok(Self { server: ChangesetsClient::with_interceptor(channel, auth), store, flags })
    }

    /// Warm `rev` (a ref name or changeset id) under `paths`.
    pub async fn run_once(&self, rev: &str, paths: &[String]) -> Result<PrefetchStats> {
        require(self.flags.as_ref(), Flag::DaemonPrefetch)?;
        let info = self
            .server
            .clone()
            .get(ResolveRequest { rev: rev.to_owned() })
            .await
            .map_err(|s| Error::not_found(format!("resolving `{rev}`: {}", s.message())))?
            .into_inner();
        warm(self.store.as_ref(), Id::from_slice(&info.root_tree)?, paths).await
    }

    /// Warm now, then again every `every` (if given), logging each pass. Never returns
    /// when repeating; failures are logged and retried on the next tick.
    pub async fn run(self, rev: String, paths: Vec<String>, every: Option<Duration>) {
        loop {
            let started = Instant::now();
            match self.run_once(&rev, &paths).await {
                Ok(s) => {
                    metrics::counter!("sapwood_prefetch_objects_total").increment(s.objects);
                    tracing::info!(%rev, files = s.files, objects = s.objects, bytes = s.bytes,
                        secs = started.elapsed().as_secs_f64(), "prefetch complete");
                }
                Err(e) => tracing::warn!(%rev, error = %e, "prefetch failed"),
            }
            match every {
                Some(d) => tokio::time::sleep(d).await,
                None => return,
            }
        }
    }
}
