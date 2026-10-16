//! Land slice: server-side rebase + lock-free ref advance.
//!
//! There is no merge queue and no global lock. Each attempt reads the ref head, rebases
//! the changeset onto it if needed (path-level conflict check, cost proportional to the
//! changes since the changeset's base), and compare-and-swaps the ref. A lost race just
//! retries against the new head.
//!
//! Within one server process, lands to the same ref are batched. Without that, N
//! concurrent landers each rebase, N-1 lose the swap and rebase again: quadratic work that
//! collapsed to 10 lands/s with errors at 200 agents. Instead each land joins its ref's
//! queue; whoever takes the turn rebases everything queued as a chain in memory, writes
//! the chain at once and advances the ref with one compare-and-swap, which still
//! arbitrates between server replicas. A change that conflicts fails alone; the rest of
//! its batch lands.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use futures::StreamExt;
use pando_core::acl::{AccessPolicy, Principal, require_ref};
use pando_core::flags::{FeatureFlags, Flag, require};
use pando_core::object::{Changeset, FileChange, Mode};
use pando_core::ports::{ChangesetLog, DerivedStore, LandCheck, LandProposal, ObjectStore};
use pando_core::{Error, Id, Repo, Result, tree, validate_ref_name};
use pando_proto::land_server::{self, LandServer};
use pando_proto::{LandRequest, LandResponse};
use tokio::sync::oneshot;
use tonic::{Request, Response};

use crate::auth::principal;
use crate::status::{RpcResult, id_from, to_status};

pub const MAX_ATTEMPTS: u32 = 64;

#[derive(Clone)]
pub struct Land {
    repo: Repo,
    flags: Arc<dyn FeatureFlags>,
    policy: Arc<dyn AccessPolicy>,
    checks: Arc<dyn LandCheck>,
    // ponytail: one entry per ref ever landed to, never pruned; prune idle entries if ref
    // counts reach millions.
    queues: Arc<Mutex<HashMap<String, Arc<RefQueue>>>>,
}

#[derive(Default)]
struct RefQueue {
    pending: Mutex<Vec<Pending>>,
    turn: Arc<tokio::sync::Mutex<()>>,
}

struct Pending {
    id: Id,
    cs: Changeset,
    base: Option<Id>,
    done: oneshot::Sender<Result<Landed>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landed {
    pub id: Id,
    pub rebased: bool,
    pub attempts: u32,
}

impl Land {
    pub fn new(
        repo: Repo,
        flags: Arc<dyn FeatureFlags>,
        policy: Arc<dyn AccessPolicy>,
        checks: Arc<dyn LandCheck>,
    ) -> Self {
        Self { repo, flags, policy, checks, queues: Default::default() }
    }

    pub async fn land(&self, who: &Principal, ref_name: &str, cs_id: Id) -> Result<Landed> {
        require(self.flags.as_ref(), Flag::Land)?;
        validate_ref_name(ref_name)?;
        if self.flags.enabled(Flag::Permissions) {
            require_ref(self.policy.as_ref(), who, ref_name)?;
        }
        let cs = self.repo.changeset(&cs_id).await?;
        let base = cs.first_parent()?;
        if self.flags.enabled(Flag::LandChecks) {
            let proposal = LandProposal {
                principal: who.0.clone(),
                ref_name: ref_name.to_owned(),
                changeset: cs_id,
                parent: base,
                author: cs.author.clone(),
                message: cs.message.clone(),
                paths: cs.changes.iter().map(|c| c.path.clone()).collect(),
            };
            self.checks.check(&proposal).await.inspect_err(|_| {
                metrics::counter!("pando_land_rejected_total").increment(1);
            })?;
        }
        let queue = self.queues.lock().expect("land queues").entry(ref_name.to_owned()).or_default().clone();
        let (done, mut result) = oneshot::channel();
        queue.pending.lock().expect("land queue").push(Pending { id: cs_id, cs, base, done });
        loop {
            tokio::select! {
                biased; // a finished result wins; no hidden randomness, so simulations replay
                r = &mut result => return r.map_err(|_| Error::Storage(anyhow::anyhow!("land batch stopped")))?,
                turn = queue.turn.clone().lock_owned() => {
                    let batch = std::mem::take(&mut *queue.pending.lock().expect("land queue"));
                    if !batch.is_empty() {
                        // Spawned so a caller that disconnects cannot abandon the others.
                        let (this, ref_name) = (self.clone(), ref_name.to_owned());
                        let _ = tokio::spawn(async move {
                            this.land_batch(&ref_name, batch).await;
                            drop(turn);
                        })
                        .await;
                    }
                }
            }
        }
    }

    /// Land `batch` in order as one chain on top of the ref head, then advance the ref once.
    async fn land_batch(&self, ref_name: &str, batch: Vec<Pending>) {
        let fail = |batch: Vec<Pending>, e: &Error| {
            for p in batch {
                let _ = p.done.send(Err(copy_err(e)));
            }
        };
        // A storage error (a timeout, a lost reply) retries the whole attempt from the current
        // head instead of failing every change in the batch. That is safe even when the failed
        // write was applied: changes that already landed are found in the head's history and
        // reported as landed, never applied twice.
        let mut storage_retries = 0;
        let mut retry_storage = async |e: &Error| -> bool {
            if !matches!(e, Error::Storage(_)) || storage_retries == STORAGE_RETRIES {
                return false;
            }
            tokio::time::sleep(STORAGE_BACKOFF * 2u32.pow(storage_retries)).await;
            storage_retries += 1;
            true
        };
        for attempt in 1..=MAX_ATTEMPTS {
            let head = match self.repo.refs.get(ref_name).await {
                Ok(h) => h,
                Err(e) if retry_storage(&e).await => continue,
                Err(e) => return fail(batch, &e),
            };
            let staged = Staged::over(&self.repo);
            let repo = staged.repo();
            // Everything that depends only on the batch's starting head runs concurrently;
            // only replaying onto the growing chain is sequential, and that reads staged data.
            let prepares: Vec<_> = batch.iter().map(|p| prepare(&repo, &staged, ref_name, p, head)).collect();
            let prepared: Vec<Result<Prepared>> =
                futures::stream::iter(prepares).buffered(PREPARE_CONCURRENCY).collect().await;
            let mut tip = head;
            let mut outcomes = Vec::with_capacity(batch.len());
            for (p, prep) in batch.iter().zip(prepared) {
                let r = match prep {
                    Ok(prep) => replay(&repo, p, prep, tip).await,
                    Err(e) => Err(e),
                };
                if let Ok((id, _, true)) = r {
                    tip = Some(id);
                }
                outcomes.push(r);
            }
            if let Some(Err(e)) = outcomes.iter().find(|r| matches!(r, Err(Error::Storage(_))))
                && retry_storage(e).await
            {
                continue;
            }
            if tip != head {
                if let Err(e) = staged.flush(&self.repo).await {
                    if retry_storage(&e).await {
                        continue;
                    }
                    return fail(batch, &e);
                }
                match self.repo.refs.compare_and_swap(ref_name, head, tip.expect("moved")).await {
                    Ok(true) => {}
                    Ok(false) => continue, // another replica moved the ref: redo on its head
                    Err(e) if retry_storage(&e).await => continue,
                    Err(e) => return fail(batch, &e),
                }
            }
            metrics::histogram!("pando_land_batch_size").record(batch.len() as f64);
            for (p, r) in batch.into_iter().zip(outcomes) {
                let r = r.map(|(id, rebased, _)| {
                    metrics::counter!("pando_land_total", "rebased" => rebased.to_string()).increment(1);
                    metrics::histogram!("pando_land_attempts").record(attempt as f64);
                    Landed { id, rebased, attempts: attempt }
                });
                let _ = p.done.send(r);
            }
            return;
        }
        fail(batch, &Error::Conflict(format!("ref `{ref_name}` too contended after {MAX_ATTEMPTS} attempts")));
    }
}

/// Storage errors a batch rides out before failing, and the first backoff (doubling).
const STORAGE_RETRIES: u32 = 4;
const STORAGE_BACKOFF: std::time::Duration = std::time::Duration::from_millis(25);

/// Concurrent reads per batch while preparing.
const PREPARE_CONCURRENCY: usize = 64;

/// What a pending land needs from the batch's starting head.
enum Prepared {
    /// Already in the ref's history: lands as is, the ref does not move.
    Present,
    /// The ref did not exist when the batch started.
    OntoEmpty,
    /// The entries along each changed path in the changeset's base.
    Replay { base: Option<Id>, trails: Vec<Vec<Option<(Mode, Id)>>> },
}

async fn prepare(repo: &Repo, staged: &Staged, ref_name: &str, p: &Pending, head: Option<Id>) -> Result<Prepared> {
    let Some(h) = head else { return Ok(Prepared::OntoEmpty) };
    if h == p.id || repo.is_ancestor(p.id, h).await? {
        return Ok(Prepared::Present);
    }
    let Some(base) = p.base else {
        return Err(Error::Conflict(format!("root changeset cannot land on existing ref `{ref_name}`")));
    };
    if base != h && !repo.is_ancestor(base, h).await? {
        return Err(Error::Conflict("changeset base is not in the ref's history; sync and retry".into()));
    }
    let base_root = repo.root_tree(base).await?;
    let objects = repo.objects.as_ref();
    let trails =
        futures::future::try_join_all(p.cs.changes.iter().map(|c| tree::trail(objects, base_root, &c.path))).await?;
    // Blob existence is checked again when the changeset is replayed; answer it from here.
    let blobs: Vec<Id> = p.cs.changes.iter().filter_map(|c| c.blob_id().transpose()).collect::<Result<_>>()?;
    staged.known_present(&blobs, &repo.objects.missing(&blobs).await?);
    Ok(Prepared::Replay { base: Some(base), trails })
}

/// Land `p` on top of `tip`: (resulting id, rebased, whether the ref moves). Conflicts are
/// path overlaps with changes made between the changeset's base and `tip`, unless both
/// sides made the same change.
async fn replay(repo: &Repo, p: &Pending, prep: Prepared, tip: Option<Id>) -> Result<(Id, bool, bool)> {
    let (base, trails) = match prep {
        Prepared::Present => return Ok((p.id, false, false)),
        // The first change in the batch creates the ref; the rest were prepared without a
        // head to check against, so they sync and retry like a lost race.
        Prepared::OntoEmpty if tip.is_none() => return Ok((p.id, false, true)),
        Prepared::OntoEmpty if tip == Some(p.id) => return Ok((p.id, false, false)),
        Prepared::OntoEmpty => {
            return Err(Error::Conflict("ref was created while landing; sync and retry".into()));
        }
        Prepared::Replay { base, trails } => (base, trails),
    };
    let (Some(tip), Some(base)) = (tip, base) else { unreachable!("prepared against an existing head") };
    if tip == p.id {
        return Ok((p.id, false, false)); // the same changeset twice in one batch
    }
    // Already landed under a rebased id (a client retrying after a lost reply, or the same
    // change twice in one batch): report that landing instead of applying the change again.
    if let Some(v) = repo.derived.get(LANDED_AS, &p.id).await? {
        let earlier = Id::from_slice(&v)?;
        if earlier == tip || repo.is_ancestor(earlier, tip).await? {
            return Ok((earlier, true, false));
        }
    }
    if tip == base {
        return Ok((p.id, false, true));
    }
    // Compare base and tip only along our own paths: cost is our changes times tree depth,
    // not the size of everything that landed since our base.
    let tip_root = repo.root_tree(tip).await?;
    let mut conflicts = Vec::new();
    for (c, b) in p.cs.changes.iter().zip(&trails) {
        let t = tree::trail(repo.objects.as_ref(), tip_root, &c.path).await?;
        if path_conflicts(c, b, &t)? {
            conflicts.push(c.path.clone());
        }
    }
    if !conflicts.is_empty() {
        let shown: Vec<_> = conflicts.iter().take(20).map(String::as_str).collect();
        return Err(Error::Conflict(format!("{} conflicting path(s): {}", conflicts.len(), shown.join(", "))));
    }
    let mut parents = vec![tip.0.to_vec()];
    parents.extend(p.cs.parents.iter().skip(1).cloned());
    let rebased = repo.create_changeset(Changeset { parents, ..p.cs.clone() }).await?;
    // Staged with the batch, so it is durable before the ref can move.
    repo.derived.put(LANDED_AS, p.id, Bytes::copy_from_slice(&rebased.0)).await?;
    Ok((rebased, true, true))
}

/// Derived record: the id a changeset was rebased to when it landed.
const LANDED_AS: &str = "landed-as.v1";

/// The same error for every member of a batch.
fn copy_err(e: &Error) -> Error {
    match e {
        Error::NotFound(m) => Error::NotFound(m.clone()),
        Error::Conflict(m) => Error::Conflict(m.clone()),
        Error::Invalid(m) => Error::Invalid(m.clone()),
        Error::Corrupt(m) => Error::Corrupt(m.clone()),
        Error::PermissionDenied(m) => Error::PermissionDenied(m.clone()),
        Error::Rejected(m) => Error::Rejected(m.clone()),
        Error::Disabled(f) => Error::Disabled(f),
        Error::Storage(e) => Error::Storage(anyhow::anyhow!("{e:#}")),
    }
}

/// Writes of one batch, held in memory until the chain is complete. Reads see staged
/// writes first, then the repository's stores.
#[derive(Default)]
struct Staged {
    objects: Mutex<HashMap<Id, Bytes>>,
    log: Mutex<Vec<(Id, Bytes)>>,
    log_index: Mutex<HashMap<Id, Bytes>>,
    derived: Mutex<Vec<(String, Id, Bytes)>>,
    derived_index: Mutex<HashMap<(String, Id), Bytes>>,
    /// Objects the repository is known to hold (checked while preparing the batch).
    present: Mutex<std::collections::HashSet<Id>>,
    inner: Option<Repo>,
}

impl Staged {
    fn over(repo: &Repo) -> Arc<Self> {
        Arc::new(Self { inner: Some(repo.clone()), ..Default::default() })
    }

    fn inner(&self) -> &Repo {
        self.inner.as_ref().expect("staged over a repo")
    }

    fn known_present(&self, ids: &[Id], missing: &[Id]) {
        let mut present = self.present.lock().expect("staged");
        present.extend(ids.iter().filter(|i| !missing.contains(i)));
    }

    fn repo(self: &Arc<Self>) -> Repo {
        Repo { objects: self.clone(), log: self.clone(), refs: self.inner().refs.clone(), derived: self.clone() }
    }

    /// Write everything staged in dependency order: objects, then the changesets that name
    /// them, then derived records that name both (a changeset's tree, the id it landed as).
    /// A flush that fails halfway then never leaves a record pointing at something missing,
    /// which a later land recreating the same changeset would trust (both orderings were
    /// found by the landing simulation). Nothing is visible until the ref moves.
    async fn flush(&self, to: &Repo) -> Result<()> {
        let mut objects: Vec<_> = self.objects.lock().expect("staged").drain().collect();
        objects.sort_unstable_by_key(|(id, _)| *id); // a fixed write order keeps simulations replayable
        let log = std::mem::take(&mut *self.log.lock().expect("staged"));
        let derived = std::mem::take(&mut *self.derived.lock().expect("staged"));
        to.objects.put_many(objects).await?;
        futures::future::try_join_all(log.into_iter().map(|(id, b)| to.log.append(id, b))).await?;
        futures::future::try_join_all(
            derived.into_iter().map(|(k, id, v)| async move { to.derived.put(&k, id, v).await }),
        )
        .await?;
        Ok(())
    }
}

#[tonic::async_trait]
impl ObjectStore for Staged {
    async fn get(&self, id: &Id) -> Result<Option<Bytes>> {
        if let Some(b) = self.objects.lock().expect("staged").get(id) {
            return Ok(Some(b.clone()));
        }
        self.inner().objects.get(id).await
    }
    async fn put(&self, id: Id, payload: Bytes) -> Result<()> {
        self.objects.lock().expect("staged").insert(id, payload);
        Ok(())
    }
    async fn has(&self, id: &Id) -> Result<bool> {
        Ok(self.objects.lock().expect("staged").contains_key(id) || self.inner().objects.has(id).await?)
    }
    async fn missing(&self, ids: &[Id]) -> Result<Vec<Id>> {
        let unstaged: Vec<Id> = {
            let (staged, present) = (self.objects.lock().expect("staged"), self.present.lock().expect("staged"));
            ids.iter().filter(|i| !staged.contains_key(i) && !present.contains(i)).copied().collect()
        };
        if unstaged.is_empty() {
            return Ok(unstaged);
        }
        self.inner().objects.missing(&unstaged).await
    }
}

#[tonic::async_trait]
impl ChangesetLog for Staged {
    async fn append(&self, id: Id, encoded: Bytes) -> Result<()> {
        if self.log_index.lock().expect("staged").insert(id, encoded.clone()).is_none() {
            self.log.lock().expect("staged").push((id, encoded));
        }
        Ok(())
    }
    async fn get(&self, id: &Id) -> Result<Option<Bytes>> {
        if let Some(b) = self.log_index.lock().expect("staged").get(id) {
            return Ok(Some(b.clone()));
        }
        self.inner().log.get(id).await
    }
}

#[tonic::async_trait]
impl DerivedStore for Staged {
    async fn get(&self, kind: &str, id: &Id) -> Result<Option<Bytes>> {
        if let Some(b) = self.derived_index.lock().expect("staged").get(&(kind.to_owned(), *id)) {
            return Ok(Some(b.clone()));
        }
        self.inner().derived.get(kind, id).await
    }
    async fn put(&self, kind: &str, id: Id, value: Bytes) -> Result<()> {
        self.derived_index.lock().expect("staged").insert((kind.to_owned(), id), value.clone());
        self.derived.lock().expect("staged").push((kind.to_owned(), id, value));
        Ok(())
    }
}

/// Whether `ours` collides with what changed between base and head along its path, given
/// the entries at each prefix of the path in both (see [`tree::trail`]). Rules: an ancestor they turned into a file, anything they changed at or
/// under our path, except the identical change to the same file.
pub fn path_conflicts(ours: &FileChange, base: &[Option<(Mode, Id)>], head: &[Option<(Mode, Id)>]) -> Result<bool> {
    let n = base.len();
    for k in 0..n.saturating_sub(1) {
        let is_file = |e: Option<(Mode, Id)>| e.is_some_and(|(m, _)| m != Mode::Dir);
        // They wrote a file there, or deleted the file that was there. A file they replaced
        // by a directory shows up only as the entries under it.
        if base[k] != head[k] && (is_file(head[k]) || (is_file(base[k]) && head[k].is_none())) {
            return Ok(true);
        }
    }
    let (b, h) = (base[n - 1], head[n - 1]);
    if b == h {
        return Ok(false);
    }
    // A directory they removed shows up as deletes under our path; a directory they
    // replaced with our exact file shows up as that file alone.
    let base_was_dir = b.is_some_and(|(m, _)| m == Mode::Dir);
    let same = match ours.blob_id()? {
        None => h.is_none() && !base_was_dir,
        Some(id) => h == Some((Mode::parse(ours.mode)?, id)),
    };
    Ok(!same)
}

pub fn service(s: Land) -> LandServer<Land> {
    LandServer::new(s)
}

#[tonic::async_trait]
impl land_server::Land for Land {
    async fn land(&self, req: Request<LandRequest>) -> RpcResult<LandResponse> {
        let who = principal(&req);
        let r = req.into_inner();
        let landed = Land::land(self, &who, &r.r#ref, id_from(&r.changeset)?).await.map_err(to_status)?;
        Ok(Response::new(LandResponse {
            landed: landed.id.0.to_vec(),
            rebased: landed.rebased,
            attempts: landed.attempts,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pando_adapters::memory::MemoryStore;

    use super::*;

    /// The full-diff rule the path-scoped check must agree with.
    /// Paths in `ours` that collide with `theirs`: same path with a different change, or a
    /// file/directory nesting in either direction.
    fn conflicting_paths(ours: &[FileChange], theirs: &BTreeMap<String, FileChange>) -> Vec<String> {
        let mut out = Vec::new();
        for c in ours {
            let p = &c.path;
            let same_path_conflict = theirs.get(p).is_some_and(|t| t != c);
            // Any of their paths strictly under ours: range ["p/", "p0") since '0' = '/' + 1.
            let under = theirs.range(format!("{p}/")..format!("{p}0")).next().is_some();
            // Any ancestor directory of ours that they changed as a file.
            let above = p.match_indices('/').any(|(i, _)| theirs.contains_key(&p[..i]));
            if same_path_conflict || under || above {
                out.push(p.clone());
            }
        }
        out
    }

    fn up(p: &str, n: u8) -> FileChange {
        FileChange::upsert(p, Mode::File, Id([n; 32]))
    }

    #[test]
    fn conflict_detection() {
        let theirs: BTreeMap<_, _> = [up("a/x", 1), up("dir/f", 2), up("file", 3), FileChange::delete("gone")]
            .into_iter()
            .map(|c| (c.path.clone(), c))
            .collect();
        let ours = vec![
            up("a/x", 1),      // identical change: fine
            up("a/y", 1),      // sibling: fine
            up("a/x2", 1),     // shares prefix only: fine
            up("dir", 9),      // file over their changed dir: conflict
            up("file/sub", 9), // dir under their changed file: conflict
            up("gone", 9),     // they deleted, we modified: conflict
            up("dir-x", 9),    // '-' sorts before '/': fine
        ];
        assert_eq!(conflicting_paths(&ours, &theirs), vec!["dir", "file/sub", "gone"]);
        assert_eq!(conflicting_paths(&[up("a/x", 2)], &theirs), vec!["a/x"]);
    }

    /// The path-scoped check agrees with the full-diff rule on random trees, including
    /// files replaced by directories and back.
    #[tokio::test]
    async fn path_scoped_check_matches_full_diff() {
        const PATHS: [&str; 7] = ["a", "a/x", "a/y", "a/b/z", "a/b", "c", "c/d"];
        let store = MemoryStore::new();
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut rnd = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let change = |rnd: &mut dyn FnMut(u64) -> u64| {
            let p = PATHS[rnd(PATHS.len() as u64) as usize];
            if rnd(4) == 0 { FileChange::delete(p) } else { up(p, 1 + rnd(2) as u8) }
        };
        let (mut checked, mut conflicts) = (0, 0);
        for _ in 0..4000 {
            let base_changes: Vec<_> = (0..rnd(5)).map(|_| change(&mut rnd)).filter(|c| !c.is_delete()).collect();
            let Ok(base) = tree::apply_changes(&store, None, &base_changes).await else { continue };
            let theirs_changes: Vec<_> = (0..rnd(4)).map(|_| change(&mut rnd)).collect();
            let Ok(head) = tree::apply_changes(&store, Some(base), &theirs_changes).await else { continue };
            let ours = change(&mut rnd);
            let theirs: BTreeMap<String, FileChange> = tree::diff_trees(&store, Some(base), Some(head))
                .await
                .unwrap()
                .into_iter()
                .map(|c| (c.path.clone(), c))
                .collect();
            let expected = !conflicting_paths(std::slice::from_ref(&ours), &theirs).is_empty();
            let (b, h) = (
                tree::trail(&store, base, &ours.path).await.unwrap(),
                tree::trail(&store, head, &ours.path).await.unwrap(),
            );
            assert_eq!(path_conflicts(&ours, &b, &h).unwrap(), expected, "ours {ours:?} theirs {theirs:?}");
            checked += 1;
            conflicts += expected as u32;
        }
        assert!(checked > 2000 && conflicts > 200, "too few cases: {checked} checked, {conflicts} conflicts");
    }

    fn cs(parent: Option<Id>, path: &str, blob: Id) -> Changeset {
        Changeset {
            parents: parent.map(|p| vec![p.0.to_vec()]).unwrap_or_default(),
            changes: vec![FileChange::upsert(path, Mode::File, blob)],
            author: "t".into(),
            message: path.into(),
            timestamp_ms: 0,
        }
    }

    /// Lands queued together form one chain: every non-conflicting change lands, and a
    /// conflicting one fails without affecting the rest of its batch.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_lands_batch_and_conflicts_fail_alone() {
        let store = Arc::new(MemoryStore::new());
        let repo = Repo { objects: store.clone(), log: store.clone(), refs: store.clone(), derived: store.clone() };
        let land = Land::new(
            repo.clone(),
            Arc::new(pando_adapters::flags::SpecFlags::all_on()),
            Arc::new(pando_core::acl::AllowAllPolicy),
            Arc::new(pando_core::ports::NoChecks),
        );
        let who = Principal("t".into());
        let objects = repo.objects.as_ref();
        let root = repo.create_changeset(cs(None, "seed", tree::put_blob(objects, b"0").await.unwrap())).await.unwrap();
        land.land(&who, "main", root).await.unwrap();

        // 100 changes on the same base: 80 write their own file, 20 fight in pairs.
        let mut ids = Vec::new();
        for i in 0..100u32 {
            let path = if i < 80 { format!("own/{i}") } else { format!("shared/{}", i % 10) };
            let blob = tree::put_blob(objects, i.to_string().as_bytes()).await.unwrap();
            ids.push(repo.create_changeset(cs(Some(root), &path, blob)).await.unwrap());
        }
        let results = futures::future::join_all(ids.iter().map(|id| land.land(&who, "main", *id))).await;
        let landed = results.iter().filter(|r| r.is_ok()).count();
        let conflicts = results.iter().filter(|r| matches!(r, Err(Error::Conflict(_)))).count();
        assert_eq!((landed, conflicts), (90, 10));

        let head = repo.refs.get("main").await.unwrap().unwrap();
        let head_root = repo.root_tree(head).await.unwrap();
        for path in (0..80).map(|i| format!("own/{i}")).chain((0..10).map(|k| format!("shared/{k}"))) {
            assert!(tree::lookup(objects, head_root, &path).await.unwrap().is_some(), "{path} missing");
        }
        assert_eq!(repo.graph(head).await.unwrap().depth, repo.graph(root).await.unwrap().depth + 90);

        // Two unrelated roots onto a new ref: one creates it, the other is refused.
        let a = repo.create_changeset(cs(None, "a", tree::put_blob(objects, b"a").await.unwrap())).await.unwrap();
        let b = repo.create_changeset(cs(None, "b", tree::put_blob(objects, b"b").await.unwrap())).await.unwrap();
        let (ra, rb) = futures::join!(land.land(&who, "fresh", a), land.land(&who, "fresh", b));
        assert_eq!([ra.is_ok(), rb.is_ok()].iter().filter(|ok| **ok).count(), 1);
        assert!(matches!(ra.err().or(rb.err()), Some(Error::Conflict(_))));
    }
}
