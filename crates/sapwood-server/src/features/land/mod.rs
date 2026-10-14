//! Land slice: server-side rebase + lock-free ref advance.
//!
//! There is no merge queue and no global lock. Each attempt reads the ref head, rebases
//! the changeset onto it if needed (path-level conflict check, cost proportional to the
//! changes since the changeset's base), and compare-and-swaps the ref. A lost race just
//! retries against the new head.

use std::collections::BTreeMap;
use std::sync::Arc;

use sapwood_core::flags::{FeatureFlags, Flag, require};
use sapwood_core::object::{Changeset, FileChange};
use sapwood_core::{Error, Id, Repo, Result, tree, validate_ref_name};
use sapwood_proto::land_server::{self, LandServer};
use sapwood_proto::{LandRequest, LandResponse};
use tonic::{Request, Response};

use crate::status::{RpcResult, id_from, to_status};

pub const MAX_ATTEMPTS: u32 = 64;

#[derive(Clone)]
pub struct Land {
    repo: Repo,
    flags: Arc<dyn FeatureFlags>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landed {
    pub id: Id,
    pub rebased: bool,
    pub attempts: u32,
}

impl Land {
    pub fn new(repo: Repo, flags: Arc<dyn FeatureFlags>) -> Self {
        Self { repo, flags }
    }

    pub async fn land(&self, ref_name: &str, cs_id: Id) -> Result<Landed> {
        require(self.flags.as_ref(), Flag::Land)?;
        validate_ref_name(ref_name)?;
        let cs = self.repo.changeset(&cs_id).await?;
        let base = cs.first_parent()?;
        for attempt in 1..=MAX_ATTEMPTS {
            let head = self.repo.refs.get(ref_name).await?;
            let (candidate, rebased) = match (head, base) {
                (None, _) => (cs_id, false),
                (Some(h), _) if h == cs_id => return Ok(Landed { id: cs_id, rebased: false, attempts: attempt }),
                (Some(h), Some(b)) if h == b => (cs_id, false),
                (Some(h), Some(b)) => {
                    if self.repo.is_ancestor(cs_id, h).await? {
                        return Ok(Landed { id: cs_id, rebased: false, attempts: attempt });
                    }
                    (self.rebase(&cs, b, h).await?, true)
                }
                (Some(_), None) => {
                    return Err(Error::Conflict(format!("root changeset cannot land on existing ref `{ref_name}`")));
                }
            };
            if self.repo.refs.compare_and_swap(ref_name, head, candidate).await? {
                metrics::counter!("sapwood_land_total", "rebased" => rebased.to_string()).increment(1);
                metrics::histogram!("sapwood_land_attempts").record(attempt as f64);
                return Ok(Landed { id: candidate, rebased, attempts: attempt });
            }
        }
        Err(Error::Conflict(format!("ref `{ref_name}` too contended after {MAX_ATTEMPTS} attempts")))
    }

    /// Replay `cs` (based on `base`) on top of `head`. Conflicts are path overlaps with
    /// changes made between `base` and `head`, unless both sides made the same change.
    async fn rebase(&self, cs: &Changeset, base: Id, head: Id) -> Result<Id> {
        if !self.repo.is_ancestor(base, head).await? {
            return Err(Error::Conflict("changeset base is not in the ref's history; sync and retry".into()));
        }
        let (base_root, head_root) = futures::try_join!(self.repo.root_tree(base), self.repo.root_tree(head))?;
        let theirs: BTreeMap<String, FileChange> = tree::diff_trees(self.repo.objects.as_ref(), Some(base_root), Some(head_root))
            .await?
            .into_iter()
            .map(|c| (c.path.clone(), c))
            .collect();
        let conflicts = conflicting_paths(&cs.changes, &theirs);
        if !conflicts.is_empty() {
            let shown: Vec<_> = conflicts.iter().take(20).map(String::as_str).collect();
            return Err(Error::Conflict(format!("{} conflicting path(s): {}", conflicts.len(), shown.join(", "))));
        }
        let mut parents = vec![head.0.to_vec()];
        parents.extend(cs.parents.iter().skip(1).cloned());
        self.repo
            .create_changeset(Changeset { parents, ..cs.clone() })
            .await
    }
}

/// Paths in `ours` that collide with `theirs`: same path with a different change, or a
/// file/directory nesting in either direction.
pub fn conflicting_paths(ours: &[FileChange], theirs: &BTreeMap<String, FileChange>) -> Vec<String> {
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

pub fn service(s: Land) -> LandServer<Land> {
    LandServer::new(s)
}

#[tonic::async_trait]
impl land_server::Land for Land {
    async fn land(&self, req: Request<LandRequest>) -> RpcResult<LandResponse> {
        let r = req.into_inner();
        let landed = Land::land(self, &r.r#ref, id_from(&r.changeset)?).await.map_err(to_status)?;
        Ok(Response::new(LandResponse { landed: landed.id.0.to_vec(), rebased: landed.rebased, attempts: landed.attempts }))
    }
}

#[cfg(test)]
mod tests {
    use sapwood_core::object::Mode;

    use super::*;

    fn up(p: &str, n: u8) -> FileChange {
        FileChange::upsert(p, Mode::File, Id([n; 32]))
    }

    #[test]
    fn conflict_detection() {
        let theirs: BTreeMap<_, _> =
            [up("a/x", 1), up("dir/f", 2), up("file", 3), FileChange::delete("gone")].into_iter().map(|c| (c.path.clone(), c)).collect();
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
}
