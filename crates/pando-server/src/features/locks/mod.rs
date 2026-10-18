//! Locks slice: exclusive locks on files that cannot be merged (art, models, video).
//!
//! A lock is the system ref `_lock/<hash of the path>` pointing at a small record object
//! (path, owner, time). Taking a lock is a compare-and-swap on that ref, so two people can
//! never both hold it, across server replicas too. Refs cannot be deleted, so unlocking
//! points the ref at [`UNLOCKED`]. User ref names cannot contain `_`, so clients cannot
//! touch these refs through the refs API. Locks do not block reads; landing a change to a
//! path someone else has locked is refused (checked when the land arrives, as Git LFS
//! checks at push).

use std::collections::HashMap;
use std::sync::Arc;

use pando_core::acl::{AccessPolicy, Principal, effective_scope, require_write};
use pando_core::flags::{FeatureFlags, Flag, require};
use pando_core::object::{Kind, decode_object, encode_object};
use pando_core::path::components;
use pando_core::{Error, Id, Repo, Result};
use pando_proto::locks_server::{self, LocksServer};
use pando_proto::{Empty, ListLocksRequest, LockInfo, LockList, LockRequest, UnlockRequest};
use serde::{Deserialize, Serialize};
use tonic::{Request, Response};

use crate::auth::principal;
use crate::status::{RpcResult, to_status};
use crate::telemetry::AUDIT;

const PREFIX: &str = "_lock/";
/// What a released lock's ref points at.
const UNLOCKED: Id = Id([0; 32]);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lock {
    pub path: String,
    pub owner: String,
    pub locked_at_ms: i64,
}

fn lock_ref(path: &str) -> String {
    format!("{PREFIX}{}", Id::hash(0, path.as_bytes()).to_hex())
}

async fn record(repo: &Repo, id: Id) -> Result<Lock> {
    let payload = repo.objects.get(&id).await?.ok_or_else(|| Error::not_found(format!("lock record {id}")))?;
    let (_, bytes) = decode_object(&id, &payload)?;
    serde_json::from_slice(&bytes).map_err(|e| Error::Corrupt(format!("lock record {id}: {e}")))
}

/// The holder of `path`'s lock, if anyone holds it.
async fn holder(repo: &Repo, path: &str) -> Result<Option<(Id, Lock)>> {
    match repo.refs.get(&lock_ref(path)).await? {
        Some(id) if id != UNLOCKED => Ok(Some((id, record(repo, id).await?))),
        _ => Ok(None),
    }
}

/// Refuse when any of `paths` is locked by someone other than `who`. Lists the lock refs
/// once, so a land touching thousands of files costs one listing, not thousands of reads.
pub async fn check(repo: &Repo, who: &Principal, paths: &[&str]) -> Result<()> {
    let held: HashMap<String, Id> =
        repo.refs.list().await?.into_iter().filter(|(name, id)| name.starts_with(PREFIX) && *id != UNLOCKED).collect();
    if held.is_empty() {
        return Ok(());
    }
    for path in paths {
        if let Some(id) = held.get(&lock_ref(path)) {
            let lock = record(repo, *id).await?;
            if lock.owner != who.0 {
                return Err(Error::Rejected(format!("`{path}` is locked by {}", lock.owner)));
            }
        }
    }
    Ok(())
}

#[derive(Clone)]
pub struct Locks {
    repo: Repo,
    flags: Arc<dyn FeatureFlags>,
    policy: Arc<dyn AccessPolicy>,
}

impl Locks {
    pub fn new(repo: Repo, flags: Arc<dyn FeatureFlags>, policy: Arc<dyn AccessPolicy>) -> Self {
        Self { repo, flags, policy }
    }

    fn guard(&self, who: &Principal, path: &str) -> Result<()> {
        require(self.flags.as_ref(), Flag::Locks)?;
        components(path)?;
        if self.flags.enabled(Flag::Permissions) {
            require_write(self.policy.as_ref(), who, [path])?;
        }
        Ok(())
    }

    /// Lock `path` for `who`. Locking a path you already hold returns your lock.
    pub async fn lock(&self, who: &Principal, path: &str) -> Result<Lock> {
        self.guard(who, path)?;
        let lock = Lock { path: path.to_owned(), owner: who.0.clone(), locked_at_ms: now_ms() };
        let (id, payload) = encode_object(Kind::Blob, &serde_json::to_vec(&lock).expect("serializable"));
        self.repo.objects.put(id, payload).await?;
        let name = lock_ref(path);
        loop {
            let current = self.repo.refs.get(&name).await?;
            if let Some(cur) = current.filter(|c| *c != UNLOCKED) {
                let held = record(&self.repo, cur).await?;
                return if held.owner == who.0 {
                    Ok(held)
                } else {
                    Err(Error::Conflict(format!("`{path}` is locked by {}", held.owner)))
                };
            }
            if self.repo.refs.compare_and_swap(&name, current, id).await? {
                return Ok(lock);
            }
        }
    }

    /// Release `path`. Only the owner may, unless `force` (which still needs write access
    /// to the path). Releasing a path nobody holds succeeds.
    pub async fn unlock(&self, who: &Principal, path: &str, force: bool) -> Result<Option<Lock>> {
        self.guard(who, path)?;
        let name = lock_ref(path);
        loop {
            let Some((id, held)) = holder(&self.repo, path).await? else { return Ok(None) };
            if held.owner != who.0 && !force {
                return Err(Error::PermissionDenied(format!(
                    "`{path}` is locked by {}; use --force to release it",
                    held.owner
                )));
            }
            if self.repo.refs.compare_and_swap(&name, Some(id), UNLOCKED).await? {
                return Ok(Some(held));
            }
        }
    }

    /// Locks held on paths under `prefix` ("" for all), ordered by path.
    pub async fn list(&self, prefix: &str) -> Result<Vec<Lock>> {
        require(self.flags.as_ref(), Flag::Locks)?;
        let mut out = Vec::new();
        for (name, id) in self.repo.refs.list().await? {
            if name.starts_with(PREFIX) && id != UNLOCKED {
                let lock = record(&self.repo, id).await?;
                if pando_core::paths_overlap(prefix, &lock.path) || prefix.is_empty() {
                    out.push(lock);
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn info(l: Lock) -> LockInfo {
    LockInfo { path: l.path, owner: l.owner, locked_at_ms: l.locked_at_ms }
}

pub fn service(s: Locks) -> LocksServer<Locks> {
    LocksServer::new(s)
}

#[tonic::async_trait]
impl locks_server::Locks for Locks {
    async fn lock(&self, req: Request<LockRequest>) -> RpcResult<LockInfo> {
        let who = principal(&req);
        let path = req.into_inner().path;
        let r = Locks::lock(self, &who, &path).await;
        tracing::info!(target: AUDIT, kind = "lock", principal = %who.0, path, ok = r.is_ok());
        Ok(Response::new(info(r.map_err(to_status)?)))
    }

    async fn unlock(&self, req: Request<UnlockRequest>) -> RpcResult<Empty> {
        let who = principal(&req);
        let r = req.into_inner();
        let out = Locks::unlock(self, &who, &r.path, r.force).await;
        let released_from = out.as_ref().ok().and_then(|l| l.as_ref().map(|l| l.owner.clone())).unwrap_or_default();
        tracing::info!(target: AUDIT, kind = "unlock", principal = %who.0, path = r.path, force = r.force, released_from, ok = out.is_ok());
        out.map_err(to_status)?;
        Ok(Response::new(Empty {}))
    }

    async fn list(&self, req: Request<ListLocksRequest>) -> RpcResult<LockList> {
        // A path-scoped reader sees only locks on paths they may read (L7).
        let scope = effective_scope(self.flags.as_ref(), self.policy.as_ref(), &principal(&req));
        let locks = Locks::list(self, &req.into_inner().prefix).await.map_err(to_status)?;
        Ok(Response::new(LockList { locks: locks.into_iter().filter(|l| scope.allows(&l.path)).map(info).collect() }))
    }
}

#[cfg(test)]
mod tests {
    use pando_adapters::memory::MemoryStore;

    use super::*;

    fn locks() -> Locks {
        let s = Arc::new(MemoryStore::new());
        Locks::new(
            Repo { objects: s.clone(), log: s.clone(), refs: s.clone(), derived: s },
            Arc::new(pando_adapters::flags::SpecFlags::all_on()),
            Arc::new(pando_core::acl::AllowAllPolicy),
        )
    }

    #[tokio::test]
    async fn one_holder_per_path_and_land_respects_it() {
        let l = locks();
        let (alice, bob) = (Principal("alice".into()), Principal("bob".into()));
        let path = "art/hero.psd";

        let held = l.lock(&alice, path).await.unwrap();
        assert_eq!(held.owner, "alice");
        assert_eq!(l.lock(&alice, path).await.unwrap(), held, "locking again returns the same lock");
        assert!(matches!(l.lock(&bob, path).await, Err(Error::Conflict(_))));

        // Landing: bob is refused on the locked file, fine elsewhere; alice may land.
        assert!(matches!(check(&l.repo, &bob, &[path, "src/main.rs"]).await, Err(Error::Rejected(_))));
        assert!(check(&l.repo, &bob, &["src/main.rs"]).await.is_ok());
        assert!(check(&l.repo, &alice, &[path]).await.is_ok());

        assert_eq!(l.list("art").await.unwrap(), vec![held.clone()]);
        assert!(l.list("src").await.unwrap().is_empty());

        // Only the owner releases, unless forced.
        assert!(matches!(l.unlock(&bob, path, false).await, Err(Error::PermissionDenied(_))));
        assert_eq!(l.unlock(&bob, path, true).await.unwrap(), Some(held));
        assert!(l.list("").await.unwrap().is_empty());
        assert_eq!(l.unlock(&alice, path, false).await.unwrap(), None, "releasing a free path succeeds");

        // A released path can be locked again by someone else.
        assert_eq!(l.lock(&bob, path).await.unwrap().owner, "bob");
    }

    #[tokio::test]
    async fn concurrent_lockers_get_exactly_one_lock() {
        let l = locks();
        let tries = futures::future::join_all((0..20).map(|i| {
            let l = l.clone();
            async move { l.lock(&Principal(format!("user{i}")), "level.umap").await }
        }))
        .await;
        assert_eq!(tries.iter().filter(|r| r.is_ok()).count(), 1);
    }

    #[tokio::test]
    async fn locks_are_off_with_their_flag() {
        let s = Arc::new(MemoryStore::new());
        let l = Locks::new(
            Repo { objects: s.clone(), log: s.clone(), refs: s.clone(), derived: s },
            Arc::new(pando_adapters::flags::SpecFlags::parse("locks=off").unwrap()),
            Arc::new(pando_core::acl::AllowAllPolicy),
        );
        assert!(matches!(l.lock(&Principal("a".into()), "x").await, Err(Error::Disabled(_))));
    }

    /// A reader limited to some paths doesn't learn the names of locked files elsewhere.
    #[tokio::test]
    async fn listing_shows_only_readable_paths() {
        struct DocsOnly;
        impl AccessPolicy for DocsOnly {
            fn can_write(&self, _: &Principal, _: &str) -> bool {
                true
            }
            fn can_update_ref(&self, _: &Principal, _: &str) -> bool {
                true
            }
            fn read_scope(&self, who: &Principal) -> pando_core::acl::ReadScope {
                if who.0 == "bob" {
                    pando_core::acl::ReadScope::Prefixes(vec!["docs".into()])
                } else {
                    pando_core::acl::ReadScope::All
                }
            }
        }
        let s = Arc::new(MemoryStore::new());
        let l = Locks::new(
            Repo { objects: s.clone(), log: s.clone(), refs: s.clone(), derived: s },
            Arc::new(pando_adapters::flags::SpecFlags::all_on()),
            Arc::new(DocsOnly),
        );
        let alice = Principal("alice".into());
        l.lock(&alice, "secret/plan.psd").await.unwrap();
        l.lock(&alice, "docs/guide.md").await.unwrap();
        let list = async |who: &str| {
            let mut req = Request::new(ListLocksRequest { prefix: String::new() });
            req.extensions_mut().insert(Principal(who.into()));
            let r = locks_server::Locks::list(&l, req).await.unwrap().into_inner();
            r.locks.into_iter().map(|x| x.path).collect::<Vec<_>>()
        };
        assert_eq!(list("bob").await, vec!["docs/guide.md"]);
        assert_eq!(list("alice").await.len(), 2);
    }
}
