//! Compact slice: delta-encode blob versions in storage (`pandod compact`).
//!
//! Walks every changeset reachable from any ref that an earlier run has not finished,
//! oldest generation first. For each file
//! changed relative to the first parent, the new blob is re-stored as a zstd delta
//! against the previous version of the same path; likewise every directory tree on the
//! path to a change (including the root) against the parent's tree at the same path.
//! Trees dominate storage in large monorepos, since each changeset writes a new root.
//!
//! Runs are incremental: each finished changeset is recorded, and the history walk stops at
//! recorded ones. Since changesets finish in generation order and every ancestor has a
//! lower generation, a recorded changeset's ancestors are always recorded too, even if a
//! run is interrupted. Repeated runs therefore cost what changed since the last run.
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

use pando_core::chunked::is_manifest;
use pando_core::delta::{self, MAX_DELTA_INPUT, MAX_DEPTH};
use pando_core::flags::{FeatureFlags, Flag, require};
use pando_core::object::{Kind, Mode, decode_object, payload_kind};
use pando_core::ports::ObjectStore;
use pando_core::{Error, Id, Repo, Result, SYSTEM_REF_PREFIX, tree};
use quick_cache::Weighter;
use quick_cache::sync::Cache;

pub const LEASE_REF: &str = "_pando/compaction-lease";
/// Derived-store namespace recording objects that are delta bases (never converted).
pub const PIN_KIND: &str = "delta-base-pin.v1";
/// Derived-store namespace recording changesets whose objects have been compacted.
pub const DONE_KIND: &str = "compacted.v1";
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
/// A parsed tree: entries sorted by name (validated on parse).
type Entries = Arc<Vec<(String, Mode, Id)>>;

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
    /// Parsed trees. Consecutive changesets share almost all of their trees, so path
    /// lookups are served from memory instead of re-reading and re-parsing each level.
    trees: Cache<Id, Entries>,
    /// Delta chain below each object (its base, the base's base, ...). Chains never change
    /// once something depends on them (pinning), so each is read from storage once per run.
    chains: Cache<Id, Arc<Vec<Id>>>,
}

impl Compactor {
    pub fn new(repo: Repo, raw: Arc<dyn ObjectStore>, flags: Arc<dyn FeatureFlags>) -> Self {
        let budget = 512 << 20;
        Self {
            repo,
            raw,
            flags,
            contents: Cache::with_weighter(16_384, budget, ContentWeight),
            trees: Cache::new(65_536),
            chains: Cache::new(262_144),
        }
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

    async fn done(&self, id: &Id) -> Result<bool> {
        Ok(self.repo.derived.get(DONE_KIND, id).await?.is_some())
    }

    /// Every changeset reachable from any user ref and not yet compacted, oldest
    /// generation first.
    async fn history(&self) -> Result<Vec<Id>> {
        let mut stack = Vec::new();
        let mut seen = HashSet::new();
        for (name, id) in self.repo.refs.list().await? {
            if !name.starts_with(SYSTEM_REF_PREFIX) && seen.insert(id) && !self.done(&id).await? {
                stack.push(id);
            }
        }
        let mut out = Vec::new();
        while let Some(id) = stack.pop() {
            out.push((self.repo.graph(id).await?.generation, id));
            for p in self.repo.changeset(&id).await?.parent_ids()? {
                if seen.insert(p) && !self.done(&p).await? {
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
            self.compact_changeset(id, &mut stats).await?;
            self.repo.derived.put(DONE_KIND, id, bytes::Bytes::from_static(b"1")).await?;
            if stats.changesets.is_multiple_of(5_000) {
                tracing::info!(?stats, "compaction progress");
            }
        }
        Ok(stats)
    }

    async fn compact_changeset(&self, id: Id, stats: &mut CompactStats) -> Result<()> {
        let cs = self.repo.changeset(&id).await?;
        let Some(parent) = cs.first_parent()? else { return Ok(()) };
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
            if let Some((mode, base)) = self.lookup(parent_root, &c.path).await?
                && mode != Mode::Dir
                && base != target
            {
                stats.pairs += 1;
                self.pair(base, target, stats).await?;
            }
        }
        // Directory trees along every changed path, parents before children is not
        // required: each pair is independent and pinning keeps chains exact.
        for dir in dirs {
            if let (Some((Mode::Dir, base)), Some((Mode::Dir, target))) =
                (self.lookup(parent_root, &dir).await?, self.lookup(root, &dir).await?)
                && base != target
            {
                stats.pairs += 1;
                self.pair(base, target, stats).await?;
            }
        }
        Ok(())
    }

    async fn entries(&self, id: Id) -> Result<Entries> {
        if let Some(e) = self.trees.get(&id) {
            return Ok(e);
        }
        let payload = self.repo.objects.get(&id).await?.ok_or_else(|| Error::not_found(format!("tree {id}")))?;
        let (_, content) = decode_object(&id, &payload)?;
        let parsed = tree::parse_tree(&id, &content)?
            .entries
            .into_iter()
            .map(|e| Ok((e.name, Mode::parse(e.mode)?, Id::from_slice(&e.id)?)))
            .collect::<Result<Vec<_>>>()?;
        let e = Arc::new(parsed);
        self.trees.insert(id, e.clone());
        Ok(e)
    }

    /// `tree::lookup`, served from the parsed-tree cache.
    async fn lookup(&self, root: Id, path: &str) -> Result<Option<(Mode, Id)>> {
        let mut cur = (Mode::Dir, root);
        for part in path.split('/').filter(|p| !p.is_empty()) {
            if cur.0 != Mode::Dir {
                return Ok(None);
            }
            let entries = self.entries(cur.1).await?;
            match entries.binary_search_by(|(name, _, _)| name.as_str().cmp(part)) {
                Ok(i) => cur = (entries[i].1, entries[i].2),
                Err(_) => return Ok(None),
            }
        }
        Ok(Some(cur))
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

    /// The objects below `id` (stored as `payload`) in its delta chain, nearest first.
    async fn chain(&self, id: Id, mut payload: bytes::Bytes) -> Result<Arc<Vec<Id>>> {
        if let Some(c) = self.chains.get(&id) {
            return Ok(c);
        }
        let mut below = Vec::new();
        let mut tail: Arc<Vec<Id>> = Arc::default();
        while delta::is_delta(&payload) {
            if below.len() >= delta::READ_DEPTH_LIMIT {
                return Err(Error::Corrupt("delta chain longer than the maximum depth".into()));
            }
            let (base, _, _) = delta::parse(&payload)?;
            below.push(base);
            if let Some(c) = self.chains.get(&base) {
                tail = c;
                break;
            }
            payload = self.raw.get(&base).await?.ok_or_else(|| Error::Corrupt(format!("missing delta base {base}")))?;
        }
        below.extend(tail.iter().copied());
        let chain = Arc::new(below);
        self.chains.insert(id, chain.clone());
        Ok(chain)
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
        let base_chain = self.chain(base, bp.clone()).await?;
        if base == target || base_chain.contains(&target) {
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
            let mut chain = vec![base];
            chain.extend(base_chain.iter().copied());
            self.chains.insert(target, Arc::new(chain));
            stats.converted += 1;
            stats.trees_converted += (kind == Kind::Tree) as u64;
            stats.bytes_before += tp.len() as u64;
            stats.bytes_after += encoded.len() as u64;
        }
        Ok(())
    }
}
