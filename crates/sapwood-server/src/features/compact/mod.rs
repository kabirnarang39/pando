//! Compact slice: delta-encode blob versions in storage (`sapwood compact`).
//!
//! Walks every changeset reachable from any ref, oldest generation first. For each file
//! changed relative to the first parent, the new blob is re-stored as a zstd delta
//! against the previous version of the same path; likewise every directory tree on the
//! path to a change (including the root) against the parent's tree at the same path.
//! Trees dominate storage in large monorepos, since each changeset writes a new root.
//!
//! Safety argument:
//! * Exactly one compaction runs at a time (lease = CAS on a reserved system ref), and live
//!   servers never convert objects, so conversions are serialised.
//! * Before `target` is stored as a delta on `base`, `base` is *pinned* (recorded in the
//!   derived-data store), and pinned objects are never converted. A base's chain therefore
//!   never changes after something depends on it: depth headers stay exact, chains stay
//!   within `MAX_DEPTH`, and no cycle can form.
//! * As a second line of defence a conversion is skipped if the target already appears in
//!   the base's chain.
//! * Every delta is decoded and compared with the target in memory *before* it replaces
//!   the stored object; `replace` is atomic per object, so readers see old or new, never torn.
//! * Readers re-verify the reconstructed content against its id on every read.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use quick_cache::Weighter;
use quick_cache::sync::Cache;
use sapwood_core::chunked::is_manifest;
use sapwood_core::delta::{self, MAX_DELTA_INPUT, MAX_DEPTH};
use sapwood_core::flags::{FeatureFlags, Flag, require};
use sapwood_core::object::{Kind, Mode, decode_object, payload_kind};
use sapwood_core::ports::ObjectStore;
use sapwood_core::{Error, Id, Repo, Result, SYSTEM_REF_PREFIX, tree};

pub const LEASE_REF: &str = "_sapwood/compaction-lease";
/// Derived-store namespace recording objects that are delta bases (never converted).
pub const PIN_KIND: &str = "delta-base-pin.v1";
const UNLOCKED: Id = Id([0; 32]);
/// Store a delta only if it is at most 80% of the full payload.
const MIN_GAIN_NUM: usize = 8;
const MIN_GAIN_DEN: usize = 10;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CompactStats {
    pub changesets: u64,
    pub pairs: u64,
    pub converted: u64,
    pub trees_converted: u64,
    pub already_delta: u64,
    pub skipped_depth: u64,
    pub skipped_cycle: u64,
    pub skipped_pinned: u64,
    pub skipped_small_gain: u64,
    pub skipped_large: u64,
    pub skipped_chunked: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

type Content = (Kind, Arc<Vec<u8>>);

#[derive(Clone)]
struct ContentWeight;
impl Weighter<Id, Content> for ContentWeight {
    fn weight(&self, _: &Id, v: &Content) -> u64 {
        v.1.len() as u64 + 64
    }
}

pub struct Compactor {
    repo: Repo,
    /// The durable object store itself, not the read-resolving decorator: compaction must
    /// see stored representations to read chain headers and to replace payloads.
    raw: Arc<dyn ObjectStore>,
    flags: Arc<dyn FeatureFlags>,
    /// Recently materialised contents: each version's content is the next version's base.
    contents: Cache<Id, Content, ContentWeight>,
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
            let root = self.repo.root_tree(id).await?;
            let mut dirs = std::collections::BTreeSet::from([String::new()]);
            for c in &cs.changes {
                let mut cut = c.path.len();
                while let Some(i) = c.path[..cut].rfind('/') {
                    dirs.insert(c.path[..i].to_owned());
                    cut = i;
                }
                let Some(target) = c.blob_id()? else { continue };
                if let Some((mode, base)) = tree::lookup(self.repo.objects.as_ref(), parent_root, &c.path).await?
                    && mode != Mode::Dir
                    && base != target
                {
                    stats.pairs += 1;
                    self.pair(base, target, &mut stats).await?;
                }
            }
            // Directory trees along every changed path, parents before children is not
            // required: each pair is independent and pinning keeps chains exact.
            for dir in dirs {
                let objects = self.repo.objects.as_ref();
                if let (Some((Mode::Dir, base)), Some((Mode::Dir, target))) =
                    (tree::lookup(objects, parent_root, &dir).await?, tree::lookup(objects, root, &dir).await?)
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

    async fn content(&self, id: Id, payload: bytes::Bytes) -> Result<Content> {
        if let Some(c) = self.contents.get(&id) {
            return Ok(c);
        }
        let (kind, data) = delta::content(self.raw.as_ref(), &id, payload).await?;
        let c = (kind, Arc::new(data));
        self.contents.insert(id, c.clone());
        Ok(c)
    }

    /// Does `needle` appear in the delta chain starting at `start` (stored as `payload`)?
    async fn chain_contains(&self, mut payload: bytes::Bytes, needle: Id) -> Result<bool> {
        for _ in 0..delta::READ_DEPTH_LIMIT {
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
        // Large files are already stored as deduplicated chunks; their manifests are tiny
        // and must stay readable as manifests. Never use one as a target or a base.
        if is_manifest(&tp) || is_manifest(&bp) {
            stats.skipped_chunked += 1;
            return Ok(());
        }
        if self.repo.derived.get(PIN_KIND, &target).await?.is_some() {
            // Something is already stored as a delta against `target`; converting it now
            // would silently lengthen that chain.
            stats.skipped_pinned += 1;
            return Ok(());
        }
        let kind = payload_kind(&tp)?;
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
        let (base_kind, base_content) = self.content(base, bp).await?;
        if base_kind != kind {
            return Ok(());
        }
        let (_, target_content) = decode_object(&target, &tp)?;
        let target_content = Arc::new(target_content);
        self.contents.insert(target, (kind, target_content.clone()));
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
        // Pin first: a crash between pin and replace leaves a harmless extra pin.
        self.repo.derived.put(PIN_KIND, base, bytes::Bytes::from_static(b"1")).await?;
        if self.raw.replace(target, encoded.clone().into()).await? {
            stats.converted += 1;
            stats.trees_converted += (kind == Kind::Tree) as u64;
            stats.bytes_before += tp.len() as u64;
            stats.bytes_after += encoded.len() as u64;
        }
        Ok(())
    }
}
