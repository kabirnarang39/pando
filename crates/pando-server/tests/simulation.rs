//! Deterministic simulation of the landing protocol.
//!
//! Several server replicas, each with its own land queue, share one store through a
//! fault-injecting wrapper: operations fail, slow down, or apply and then report failure
//! (a lost reply). Replicas crash mid-batch and restart. Clients land changes on random
//! replicas; some changes deliberately conflict. Everything runs on a single-threaded
//! runtime with a paused clock and draws from one seeded generator, so a failing seed
//! replays exactly:
//!
//!     PANDO_SIM_SEED=1234 cargo test -p pando-server --test simulation
//!
//! `PANDO_SIM_SEEDS` sets how many seeds to run (default 200) and `PANDO_SIM_SEED_BASE`
//! where they start, so scheduled runs explore new schedules.
//!
//! After every run these must hold:
//! 0. no change is in the branch's history twice;
//! 1. every land the server reported as successful is in the branch's history;
//! 2. the branch's tree equals replaying its history's changes in order;
//! 3. two changes to the same path are never both landed unless the later one was based
//!    on a history that already contained the earlier one.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use pando_adapters::flags::SpecFlags;
use pando_adapters::memory::MemoryStore;
use pando_core::acl::{AllowAllPolicy, Principal};
use pando_core::object::{Changeset, FileChange, Mode};
use pando_core::ports::{ChangesetLog, DerivedStore, NoChecks, ObjectStore, RefStore};
use pando_core::{Error, Id, Repo, Result, tree};
use pando_server::features::land::Land;

const REF: &str = "main";

/// xorshift64*: small, seedable, good enough to drive a schedule.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// The shared store as one replica sees it: faults are injected per operation.
struct Faulty {
    inner: Arc<MemoryStore>,
    rng: Arc<Mutex<Rng>>,
    crashed: AtomicBool,
}

enum Fault {
    None,
    Before,
    LostReply,
}

impl Faulty {
    async fn fault(&self, write: bool) -> Result<Fault> {
        if self.crashed.load(Ordering::Relaxed) {
            return Err(Error::Storage(anyhow::anyhow!("replica crashed")));
        }
        let (delay, fault) = {
            let mut r = self.rng.lock().unwrap();
            let delay = r.chance(30).then(|| r.below(20));
            let fault = match r.below(100) {
                0 => Fault::Before,
                1 if write => Fault::LostReply,
                _ => Fault::None,
            };
            (delay, fault)
        };
        if let Some(ms) = delay {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        match fault {
            Fault::Before => Err(Error::Storage(anyhow::anyhow!("injected failure"))),
            f => Ok(f),
        }
    }

    fn after(f: Fault) -> Result<()> {
        match f {
            Fault::LostReply => Err(Error::Storage(anyhow::anyhow!("injected lost reply"))),
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl ObjectStore for Faulty {
    async fn get(&self, id: &Id) -> Result<Option<Bytes>> {
        self.fault(false).await?;
        ObjectStore::get(self.inner.as_ref(), id).await
    }
    async fn put(&self, id: Id, payload: Bytes) -> Result<()> {
        let f = self.fault(true).await?;
        ObjectStore::put(self.inner.as_ref(), id, payload).await?;
        Self::after(f)
    }
    async fn has(&self, id: &Id) -> Result<bool> {
        self.fault(false).await?;
        self.inner.has(id).await
    }
}

#[async_trait]
impl ChangesetLog for Faulty {
    async fn append(&self, id: Id, encoded: Bytes) -> Result<()> {
        let f = self.fault(true).await?;
        self.inner.append(id, encoded).await?;
        Self::after(f)
    }
    async fn get(&self, id: &Id) -> Result<Option<Bytes>> {
        self.fault(false).await?;
        ChangesetLog::get(self.inner.as_ref(), id).await
    }
}

#[async_trait]
impl RefStore for Faulty {
    async fn get(&self, name: &str) -> Result<Option<Id>> {
        self.fault(false).await?;
        RefStore::get(self.inner.as_ref(), name).await
    }
    async fn compare_and_swap(&self, name: &str, expected: Option<Id>, new: Id) -> Result<bool> {
        let f = self.fault(true).await?;
        let swapped = self.inner.compare_and_swap(name, expected, new).await?;
        Self::after(f)?;
        Ok(swapped)
    }
    async fn list(&self) -> Result<Vec<(String, Id)>> {
        self.fault(false).await?;
        RefStore::list(self.inner.as_ref()).await
    }
}

#[async_trait]
impl DerivedStore for Faulty {
    async fn get(&self, kind: &str, id: &Id) -> Result<Option<Bytes>> {
        self.fault(false).await?;
        DerivedStore::get(self.inner.as_ref(), kind, id).await
    }
    async fn put(&self, kind: &str, id: Id, value: Bytes) -> Result<()> {
        let f = self.fault(true).await?;
        DerivedStore::put(self.inner.as_ref(), kind, id, value).await?;
        Self::after(f)
    }
}

fn repo_over<S: ObjectStore + ChangesetLog + RefStore + DerivedStore + 'static>(s: Arc<S>) -> Repo {
    Repo { objects: s.clone(), log: s.clone(), refs: s.clone(), derived: s }
}

struct Replica {
    land: Land,
    store: Arc<Faulty>,
}

fn start_replica(inner: &Arc<MemoryStore>, rng: &Arc<Mutex<Rng>>) -> Replica {
    let store = Arc::new(Faulty { inner: inner.clone(), rng: rng.clone(), crashed: AtomicBool::new(false) });
    let land = Land::new(
        repo_over(store.clone()),
        Arc::new(SpecFlags::all_on()),
        Arc::new(AllowAllPolicy),
        Arc::new(NoChecks),
    );
    Replica { land, store }
}

/// A change a client asked to land, and what the server answered.
struct Attempt {
    msg: String,
    base: Id,
    path: String,
    blob: Id,
    landed: Option<Id>,
    outcome: Outcome,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Outcome {
    Landed,
    Conflict,
    Failed,
}

async fn simulate(seed: u64) -> HashMap<Outcome, usize> {
    let rng = Arc::new(Mutex::new(Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)));
    let inner = Arc::new(MemoryStore::new());
    let truth = repo_over(inner.clone()); // fault-free view, for clients and checks

    let seed_blob = tree::put_blob(truth.objects.as_ref(), b"seed").await.unwrap();
    let root = truth
        .create_changeset(Changeset {
            parents: vec![],
            changes: vec![FileChange::upsert("seed", Mode::File, seed_blob)],
            author: "sim".into(),
            message: "seed".into(),
            timestamp_ms: 0,
        })
        .await
        .unwrap();
    truth.refs.compare_and_swap(REF, None, root).await.unwrap();

    let replicas: Arc<Vec<Mutex<Arc<Replica>>>> =
        Arc::new((0..3).map(|_| Mutex::new(Arc::new(start_replica(&inner, &rng)))).collect());
    let attempts = Arc::new(Mutex::new(Vec::<Attempt>::new()));

    // Crashes and restarts while clients run.
    let chaos = {
        let (replicas, rng, inner) = (replicas.clone(), rng.clone(), inner.clone());
        tokio::spawn(async move {
            for _ in 0..6 {
                let (wait, which) = {
                    let mut r = rng.lock().unwrap();
                    (r.below(400), r.below(3) as usize)
                };
                tokio::time::sleep(Duration::from_millis(wait)).await;
                let old = replicas[which].lock().unwrap().clone();
                old.store.crashed.store(true, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(50)).await;
                *replicas[which].lock().unwrap() = Arc::new(start_replica(&inner, &rng));
            }
        })
    };

    let mut clients = Vec::new();
    for c in 0..12u32 {
        let (replicas, rng, attempts, truth) = (replicas.clone(), rng.clone(), attempts.clone(), truth.clone());
        clients.push(tokio::spawn(async move {
            for j in 0..8u32 {
                let (path, which, think) = {
                    let mut r = rng.lock().unwrap();
                    // A third of the changes fight over a handful of shared paths.
                    let path = if r.chance(33) { format!("shared/{}", r.below(4)) } else { format!("own/c{c}/f{j}") };
                    (path, r.below(3) as usize, r.below(30))
                };
                tokio::time::sleep(Duration::from_millis(think)).await;
                let base = truth.refs.get(REF).await.unwrap().unwrap();
                let blob = tree::put_blob(truth.objects.as_ref(), format!("{c}/{j}").as_bytes()).await.unwrap();
                let id = truth
                    .create_changeset(Changeset {
                        parents: vec![base.0.to_vec()],
                        changes: vec![FileChange::upsert(path.clone(), Mode::File, blob)],
                        author: format!("client-{c}"),
                        message: format!("{c}/{j}"),
                        timestamp_ms: 0,
                    })
                    .await
                    .unwrap();
                // A failed land is retried, sometimes on another replica while the first may
                // still be working on it: a real client does not know whether it landed.
                let mut which = which;
                let (mut landed, mut outcome) = (None, Outcome::Failed);
                for _ in 0..3 {
                    let replica = replicas[which].lock().unwrap().clone();
                    let land = replica.land.clone();
                    let attempt = async move { land.land(&Principal("sim".into()), REF, id).await };
                    let patience = Duration::from_millis(20 + rng.lock().unwrap().below(1000));
                    // Give up waiting (the reply is lost) but let the land carry on.
                    let r = match tokio::time::timeout(patience, tokio::spawn(attempt)).await {
                        Ok(joined) => joined.unwrap(),
                        Err(_) => Err(Error::Storage(anyhow::anyhow!("client timed out"))),
                    };
                    (landed, outcome) = match r {
                        Ok(l) => (Some(l.id), Outcome::Landed),
                        Err(Error::Conflict(_)) => (None, Outcome::Conflict),
                        Err(Error::Storage(_)) => (None, Outcome::Failed),
                        Err(e) => panic!("seed {seed}: unexpected error {e}"),
                    };
                    if outcome != Outcome::Failed {
                        break;
                    }
                    which = rng.lock().unwrap().below(3) as usize;
                }
                attempts.lock().unwrap().push(Attempt { msg: format!("{c}/{j}"), base, path, blob, landed, outcome });
            }
        }));
    }
    for c in clients {
        c.await.unwrap();
    }
    chaos.abort();

    let attempts = std::mem::take(&mut *attempts.lock().unwrap());
    check(seed, &truth, &attempts).await;
    let mut outcomes = HashMap::new();
    for a in attempts.iter() {
        *outcomes.entry(a.outcome).or_default() += 1;
    }
    outcomes
}

async fn check(seed: u64, truth: &Repo, attempts: &[Attempt]) {
    let head = truth.refs.get(REF).await.unwrap().unwrap();

    // History, oldest first, along first parents.
    let mut history = Vec::new();
    let mut cur = Some(head);
    while let Some(id) = cur {
        let cs = truth.changeset(&id).await.unwrap_or_else(|e| panic!("seed {seed}: history broken at {id}: {e}"));
        history.push((id, cs.clone()));
        cur = cs.first_parent().unwrap();
    }
    history.reverse();
    let position: HashMap<Id, usize> = history.iter().enumerate().map(|(i, (id, _))| (*id, i)).collect();

    // 0. No client change lands twice (messages are unique per attempt).
    let mut seen = std::collections::HashSet::new();
    for (id, cs) in &history {
        assert!(
            seen.insert(cs.message.clone()),
            "seed {seed}: change `{}` is in history twice (again at {id})",
            cs.message
        );
    }

    // 1. Every acknowledged land is in history: the id the server answered with, and the
    // change itself. They differ when a retried change was already there (its reply had been
    // lost): the server answers with the tip that holds it, and the change sits earlier.
    let by_msg: HashMap<&str, usize> =
        history.iter().enumerate().map(|(i, (_, cs))| (cs.message.as_str(), i)).collect();
    let mut landed = Vec::new();
    for a in attempts {
        if let Some(id) = a.landed {
            assert!(position.contains_key(&id), "seed {seed}: acknowledged land {id} is not in history");
            let at = *by_msg
                .get(a.msg.as_str())
                .unwrap_or_else(|| panic!("seed {seed}: acknowledged change `{}` is not in history", a.msg));
            landed.push((at, a));
        }
    }

    // 2. The tree equals replaying history.
    let mut model: BTreeMap<String, FileChange> = BTreeMap::new();
    for (_, cs) in &history {
        for c in &cs.changes {
            model.insert(c.path.clone(), c.clone());
        }
    }
    let root = truth.root_tree(head).await.unwrap();
    let actual: BTreeMap<String, FileChange> = tree::diff_trees(truth.objects.as_ref(), None, Some(root))
        .await
        .unwrap()
        .into_iter()
        .map(|c| (c.path.clone(), c))
        .collect();
    assert_eq!(actual, model, "seed {seed}: tree differs from replayed history");

    // 3. A later change to a path someone else changed must have been based on that change.
    landed.sort_by_key(|(at, _)| *at);
    for (i, (at_b, b)) in landed.iter().enumerate() {
        for (_at_a, a) in &landed[..i] {
            if a.path == b.path && a.blob != b.blob {
                assert!(
                    truth.is_ancestor(history[*_at_a].0, b.base).await.unwrap(),
                    "seed {seed}: change at position {at_b} to `{}` landed over a change it never saw",
                    b.path
                );
            }
        }
    }
}

fn seeds() -> Vec<u64> {
    if let Ok(s) = std::env::var("PANDO_SIM_SEED") {
        return vec![s.parse().expect("PANDO_SIM_SEED")];
    }
    let var = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let (base, n) = (var("PANDO_SIM_SEED_BASE", 0), var("PANDO_SIM_SEEDS", 200));
    (base..base + n).collect()
}

#[test]
fn landing_survives_faults_crashes_and_conflicts() {
    let seeds = seeds();
    let mut total: HashMap<Outcome, usize> = HashMap::new();
    for &seed in &seeds {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().start_paused(true).build().unwrap();
        for (k, v) in rt.block_on(simulate(seed)) {
            *total.entry(k).or_default() += v;
        }
    }
    eprintln!("{} seeds: {total:?}", seeds.len());
    // The run must actually exercise every outcome, or the invariants prove little.
    if seeds.len() >= 20 {
        for o in [Outcome::Landed, Outcome::Conflict, Outcome::Failed] {
            assert!(total.get(&o).copied().unwrap_or(0) > seeds.len() / 2, "too few {o:?} outcomes: {total:?}");
        }
    }
}
