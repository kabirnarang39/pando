//! Compact slice: delta-encode blob versions in storage (`sapwood compact`).
//!
//! Walks every changeset reachable from any ref, oldest generation first. For each file
//! changed relative to the first parent, the new blob is re-stored as a zstd delta
//! against the previous version of the same path.
//!
//! Safety argument:
//! * Exactly one compaction runs at a time (lease = CAS on a reserved system ref), and live
//!   servers never convert objects, so conversions are serialised.
//! * A conversion is skipped if the target already appears in the base's chain, so chains
//!   are acyclic by construction.
//! * Every delta is decoded and compared with the target in memory *before* it replaces
//!   the stored object; `replace` is atomic per object, so readers see old or new, never torn.
//! * Readers re-verify the reconstructed content against its id on every read.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use quick_cache::Weighter;
use quick_cache::sync::Cache;
use sapwood_core::delta::{self, MAX_DELTA_INPUT, MAX_DEPTH};
use sapwood_core::flags::{FeatureFlags, Flag, require};
use sapwood_core::object::{Kind, Mode, decode_object, payload_kind};
use sapwood_core::ports::ObjectStore;
use sapwood_core::{Error, Id, Repo, Result, SYSTEM_REF_PREFIX, tree};

pub const LEASE_REF: &str = "_sapwood/compaction-lease";
const UNLOCKED: Id = Id([0; 32]);
/// Store a delta only if it is at most 80% of the full payload.
const MIN_GAIN_NUM: usize = 8;
const MIN_GAIN_DEN: usize = 10;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CompactStats {
    pub changesets: u64,
    pub pairs: u64,
    pub converted: u64,
    pub already_delta: u64,
    pub skipped_depth: u64,
    pub skipped_cycle: u64,
    pub skipped_small_gain: u64,
    pub skipped_large: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

#[derive(Clone)]
struct ContentWeight;
impl Weighter<Id, Arc<Vec<u8>>> for ContentWeight {
    fn weight(&self, _: &Id, v: &Arc<Vec<u8>>) -> u64 {
        v.len() as u64 + 64
    }
}

pub struct Compactor {
    repo: Repo,
    /// The durable object store itself, not the read-resolving decorator: compaction must
    /// see stored representations to read chain headers and to replace payloads.
    raw: Arc<dyn ObjectStore>,
    flags: Arc<dyn FeatureFlags>,
    /// Recently materialised contents: each version's content is the next version's base.
    contents: Cache<Id, Arc<Vec<u8>>, ContentWeight>,
}

impl Compactor {
    pub fn new(repo: Repo, raw: Arc<dyn ObjectStore>, flags: Arc<dyn FeatureFlags>) -> Self {
        let budget = 512 << 20;
        Self { repo, raw, flags, contents: Cache::with_weighter(16_384, budget, ContentWeight) }
    }

    pub async fn run(&self, force_unlock: bool) -> Result<CompactStats> {
        require(self.flags.as_ref(), Flag::DeltaStorage)?;
        let lease = self.acquire(force_unlock).await?;
        let result = self.compact_all().await;
        if !self.repo.refs.compare_and_swap(LEASE_REF, Some(lease), UNLOCKED).await? {
            tracing::warn!("compaction lease was taken over before release");
        }
        result
    }

    async fn acquire(&self, force_unlock: bool) -> Result<Id> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let lease = Id::hash(0xEE, format!("{nonce}:{}", std::process::id()).as_bytes());
        let current = self.repo.refs.get(LEASE_REF).await?;
        let free = matches!(current, None | Some(UNLOCKED));
        if !free && !force_unlock {
            return Err(Error::Conflict(
                "another compaction holds the lease; pass --force-unlock only if it crashed".into(),
            ));
        }
        if !self.repo.refs.compare_and_swap(LEASE_REF, current, lease).await? {
            return Err(Error::Conflict("lost the race for the compaction lease".into()));
        }
        Ok(lease)
    }

    /// Every changeset reachable from any user ref, oldest generation first.
    async fn history(&self) -> Result<Vec<Id>> {
        let mut stack: Vec<Id> = self
            .repo
            .refs
            .list()
            .await?
            .into_iter()
            .filter(|(name, _)| !name.starts_with(SYSTEM_REF_PREFIX))
            .map(|(_, id)| id)
            .collect();
        let mut seen: HashSet<Id> = stack.iter().copied().collect();
        let mut out = Vec::new();
        while let Some(id) = stack.pop() {
            out.push((self.repo.graph(id).await?.generation, id));
            for p in self.repo.changeset(&id).await?.parent_ids()? {
                if seen.insert(p) {
                    stack.push(p);
                }
            }
        }
        out.sort();
        Ok(out.into_iter().map(|(_, id)| id).collect())
    }

    async fn compact_all(&self) -> Result<CompactStats> {
        let mut stats = CompactStats::default();
        for id in self.history().await? {
            stats.changesets += 1;
            let cs = self.repo.changeset(&id).await?;
            let Some(parent) = cs.first_parent()? else { continue };
            let parent_root = self.repo.root_tree(parent).await?;
            for c in &cs.changes {
                let Some(target) = c.blob_id()? else { continue };
                if let Some((mode, base)) = tree::lookup(self.repo.objects.as_ref(), parent_root, &c.path).await?
                    && mode != Mode::Dir
                    && base != target
                {
                    stats.pairs += 1;
                    self.pair(base, target, &mut stats).await?;
                }
            }
            if stats.changesets.is_multiple_of(5_000) {
                tracing::info!(?stats, "compaction progress");
            }
        }
        Ok(stats)
    }

    async fn content(&self, id: Id, payload: bytes::Bytes) -> Result<Arc<Vec<u8>>> {
        if let Some(c) = self.contents.get(&id) {
            return Ok(c);
        }
        let (_, data) = delta::content(self.raw.as_ref(), &id, payload).await?;
        let data = Arc::new(data);
        self.contents.insert(id, data.clone());
        Ok(data)
    }

    /// Does `needle` appear in the delta chain starting at `start` (stored as `payload`)?
    async fn chain_contains(&self, mut payload: bytes::Bytes, needle: Id) -> Result<bool> {
        for _ in 0..=MAX_DEPTH {
            if !delta::is_delta(&payload) {
                return Ok(false);
            }
            let (base, _, _) = delta::parse(&payload)?;
            if base == needle {
                return Ok(true);
            }
            payload = self.raw.get(&base).await?.ok_or_else(|| Error::Corrupt(format!("missing delta base {base}")))?;
        }
        Err(Error::Corrupt("delta chain longer than the maximum depth".into()))
    }

    async fn pair(&self, base: Id, target: Id, stats: &mut CompactStats) -> Result<()> {
        let (Some(tp), Some(bp)) = (self.raw.get(&target).await?, self.raw.get(&base).await?) else {
            return Ok(());
        };
        if delta::is_delta(&tp) {
            stats.already_delta += 1;
            return Ok(());
        }
        if payload_kind(&tp)? != Kind::Blob {
            return Ok(());
        }
        let depth = if delta::is_delta(&bp) { delta::parse(&bp)?.1 } else { 0 };
        if depth + 1 >= MAX_DEPTH {
            // Target stays whole and becomes the root of the next chain.
            stats.skipped_depth += 1;
            return Ok(());
        }
        if self.chain_contains(bp.clone(), target).await? {
            stats.skipped_cycle += 1;
            return Ok(());
        }
        let base_content = self.content(base, bp).await?;
        let (_, target_content) = decode_object(&target, &tp)?;
        let target_content = Arc::new(target_content);
        self.contents.insert(target, target_content.clone());
        if base_content.len() > MAX_DELTA_INPUT || target_content.len() > MAX_DELTA_INPUT {
            stats.skipped_large += 1;
            return Ok(());
        }
        let (bc, tc) = (base_content.clone(), target_content.clone());
        let encoded = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
            let encoded = delta::encode(base, depth + 1, &bc, &tc)?;
            // Verify before anything is replaced.
            if delta::decode_with_base(&encoded, &bc)? != *tc {
                return Err(Error::Corrupt(format!("delta for {target} failed verification")));
            }
            Ok(encoded)
        })
        .await
        .map_err(|e| Error::Storage(e.into()))??;
        if encoded.len() * MIN_GAIN_DEN >= tp.len() * MIN_GAIN_NUM {
            stats.skipped_small_gain += 1;
            return Ok(());
        }
        if self.raw.replace(target, encoded.clone().into()).await? {
            stats.converted += 1;
            stats.bytes_before += tp.len() as u64;
            stats.bytes_after += encoded.len() as u64;
        }
        Ok(())
    }
}
