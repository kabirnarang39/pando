//! History slice: first-parent log, optionally filtered to a path, streamed.

use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use pando_core::acl::{AccessPolicy, Principal, ReadScope, effective_scope, require_read};
use pando_core::flags::{FeatureFlags, Flag, require};
use pando_core::object::{Kind, Mode};
use pando_core::path::components_or_root;
use pando_core::{Error, Id, Repo, Result, paths_overlap, tree};
use pando_proto::history_server::{self, HistoryServer};
use pando_proto::{BlameLine, BlameRequest, BlameResponse, ChangesetInfo, LogRequest};
use similar::{Algorithm, DiffOp, capture_diff_slices};
use tonic::{Request, Response, Status};

use crate::auth::principal;
use crate::status::{RpcResult, async_stream, changeset_info, to_status};

pub const DEFAULT_LIMIT: u32 = 50;
pub const MAX_LIMIT: u32 = 10_000;
/// Bound on changesets scanned for one path-filtered request.
pub const MAX_SCAN: usize = 1_000_000;
/// Blame looks at most this many versions of a file; older lines are attributed to
/// the oldest version examined.
pub const MAX_BLAME_VERSIONS: u32 = 5_000;
/// Files larger than this are not blamed (diffing cost grows with size x versions).
pub const MAX_BLAME_BYTES: usize = 8 << 20;

#[derive(Clone)]
pub struct History {
    repo: Repo,
    flags: Arc<dyn FeatureFlags>,
    policy: Arc<dyn AccessPolicy>,
}

impl History {
    pub fn new(repo: Repo, flags: Arc<dyn FeatureFlags>, policy: Arc<dyn AccessPolicy>) -> Self {
        Self { repo, flags, policy }
    }

    fn scope(&self, who: &Principal) -> ReadScope {
        effective_scope(self.flags.as_ref(), self.policy.as_ref(), who)
    }

    /// Walk first parents from `rev`, returning up to `limit` changesets that touch `path`
    /// (all changesets when `path` is empty), newest first. With a restricted read scope,
    /// only changesets touching something readable are returned.
    pub async fn walk(&self, scope: &ReadScope, rev: &str, path: &str, limit: u32) -> Result<Vec<Id>> {
        require(self.flags.as_ref(), Flag::History)?;
        if !scope.traversable(&components_or_root(path)?.join("/")) {
            return Err(Error::PermissionDenied(format!("no read access to `{path}`")));
        }
        let limit = if limit == 0 { DEFAULT_LIMIT } else { limit.min(MAX_LIMIT) };
        self.walk_from(scope, self.repo.resolve(rev).await?, path, limit).await
    }

    async fn walk_from(&self, scope: &ReadScope, start: Id, path: &str, limit: u32) -> Result<Vec<Id>> {
        let filter = components_or_root(path)?.join("/");
        let mut cur = Some(start);
        let (mut out, mut scanned) = (Vec::new(), 0);
        while let Some(id) = cur {
            if out.len() >= limit as usize || scanned >= MAX_SCAN {
                break;
            }
            scanned += 1;
            let cs = self.repo.changeset(&id).await?;
            let visible = if scope.is_full() {
                filter.is_empty() || cs.changes.iter().any(|c| paths_overlap(&c.path, &filter))
            } else {
                cs.changes
                    .iter()
                    .any(|c| scope.allows(&c.path) && (filter.is_empty() || paths_overlap(&c.path, &filter)))
            };
            if visible {
                out.push(id);
            }
            cur = cs.first_parent()?;
        }
        Ok(out)
    }

    /// Changeset that last changed each line of `path` at `rev` (first-parent history).
    pub async fn blame(&self, who: &Principal, rev: &str, path: &str) -> Result<Vec<(Id, String)>> {
        require(self.flags.as_ref(), Flag::Blame)?;
        require_read(&self.scope(who), who, path)?;
        let start = self.repo.resolve(rev).await?;
        if !matches!(self.repo.lookup(start, path).await?, Some((m, _)) if m != Mode::Dir) {
            return Err(Error::not_found(format!("file `{path}` at {rev}")));
        }
        let versions = self.walk_from(&ReadScope::All, start, path, MAX_BLAME_VERSIONS).await?;
        let text = |id: Id| async move {
            match self.repo.lookup(id, path).await? {
                Some((m, blob)) if m != Mode::Dir => {
                    let bytes = tree::read_object(self.repo.objects.as_ref(), &blob, Kind::Blob).await?;
                    if bytes.len() > MAX_BLAME_BYTES {
                        return Err(Error::invalid(format!("`{path}` is too large to blame")));
                    }
                    Ok(String::from_utf8_lossy(&bytes).lines().map(str::to_owned).collect::<Vec<_>>())
                }
                _ => Ok(Vec::new()),
            }
        };
        let final_lines = text(start).await?;
        let mut owner: Vec<Option<Id>> = vec![None; final_lines.len()];
        // Position of each final line within the version currently being examined.
        let mut pos: Vec<Option<usize>> = (0..final_lines.len()).map(Some).collect();
        let mut current = final_lines.clone();
        for (k, v) in versions.iter().enumerate() {
            let older = match versions.get(k + 1) {
                // The file content just before `v` equals the content at the next older version.
                Some(o) => text(*o).await?,
                None => match self.repo.changeset(v).await?.first_parent()? {
                    Some(p) => text(p).await?,
                    None => Vec::new(),
                },
            };
            let mut to_older = vec![None; current.len()];
            for op in capture_diff_slices(Algorithm::Myers, &older, &current) {
                if let DiffOp::Equal { old_index, new_index, len } = op {
                    for i in 0..len {
                        to_older[new_index + i] = Some(old_index + i);
                    }
                }
            }
            let mut open = false;
            for (j, p) in pos.iter_mut().enumerate() {
                if let Some(cur) = *p {
                    match to_older[cur] {
                        Some(o) => {
                            *p = Some(o);
                            open = true;
                        }
                        None => {
                            owner[j] = Some(*v);
                            *p = None;
                        }
                    }
                }
            }
            current = older;
            if !open {
                break;
            }
        }
        let oldest = versions.last().copied().unwrap_or(start);
        Ok(final_lines.into_iter().zip(owner).map(|(l, o)| (o.unwrap_or(oldest), l)).collect())
    }
}

pub fn service(s: History) -> HistoryServer<History> {
    HistoryServer::new(s)
}

#[tonic::async_trait]
impl history_server::History for History {
    type LogStream = Pin<Box<dyn Stream<Item = Result<ChangesetInfo, Status>> + Send>>;

    async fn log(&self, req: Request<LogRequest>) -> RpcResult<Self::LogStream> {
        let who = principal(&req);
        let scope = self.scope(&who);
        let r = req.into_inner();
        tracing::info!(target: crate::telemetry::AUDIT, kind = "log", principal = %who.0, rev = %r.rev, path = %r.path);
        let ids = self.walk(&scope, &r.rev, &r.path, r.limit).await.map_err(to_status)?;
        let repo = self.repo.clone();
        let stream = async_stream(move |tx| async move {
            for id in ids {
                let item = changeset_info(&repo, id, &scope).await.map_err(to_status);
                let failed = item.is_err();
                if tx.send(item).await.is_err() || failed {
                    return;
                }
            }
        });
        Ok(Response::new(Box::pin(stream)))
    }

    async fn blame(&self, req: Request<BlameRequest>) -> RpcResult<BlameResponse> {
        let who = principal(&req);
        let r = req.into_inner();
        tracing::info!(target: crate::telemetry::AUDIT, kind = "blame", principal = %who.0, rev = %r.rev, path = %r.path);
        let lines = History::blame(self, &who, &r.rev, &r.path).await.map_err(to_status)?;
        Ok(Response::new(BlameResponse {
            lines: lines.into_iter().map(|(id, text)| BlameLine { changeset: id.0.to_vec(), text }).collect(),
        }))
    }
}
