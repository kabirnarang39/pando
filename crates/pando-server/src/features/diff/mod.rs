//! Diff slice: file-level diff between any two revisions, computed server-side by
//! Merkle tree comparison (only differing subtrees are visited).

use std::sync::Arc;

use pando_core::acl::{AccessPolicy, Principal, effective_scope};
use pando_core::flags::{FeatureFlags, Flag, require};
use pando_core::object::FileChange;
use pando_core::{Repo, Result, tree};
use pando_proto::diff_server::{self, DiffServer};
use pando_proto::{DiffRequest, DiffResponse};
use tonic::{Request, Response};

use crate::auth::principal;
use crate::status::{RpcResult, to_status};

#[derive(Clone)]
pub struct Diff {
    repo: Repo,
    flags: Arc<dyn FeatureFlags>,
    policy: Arc<dyn AccessPolicy>,
}

impl Diff {
    pub fn new(repo: Repo, flags: Arc<dyn FeatureFlags>, policy: Arc<dyn AccessPolicy>) -> Self {
        Self { repo, flags, policy }
    }

    /// Changes outside the caller's read scope are omitted.
    pub async fn diff(&self, who: &Principal, from: &str, to: &str) -> Result<Vec<FileChange>> {
        let scope = effective_scope(self.flags.as_ref(), self.policy.as_ref(), who);
        let mut changes = self.diff_all(from, to).await?;
        changes.retain(|c| scope.allows(&c.path));
        Ok(changes)
    }

    async fn diff_all(&self, from: &str, to: &str) -> Result<Vec<FileChange>> {
        require(self.flags.as_ref(), Flag::Diff)?;
        let (a, b) = futures::try_join!(self.repo.resolve(from), self.repo.resolve(to))?;
        let (ra, rb) = futures::try_join!(self.repo.root_tree(a), self.repo.root_tree(b))?;
        tree::diff_trees(self.repo.objects.as_ref(), Some(ra), Some(rb)).await
    }
}

pub fn service(s: Diff) -> DiffServer<Diff> {
    DiffServer::new(s).max_encoding_message_size(pando_proto::MAX_MESSAGE_BYTES)
}

#[tonic::async_trait]
impl diff_server::Diff for Diff {
    async fn diff(&self, req: Request<DiffRequest>) -> RpcResult<DiffResponse> {
        let who = principal(&req);
        let r = req.into_inner();
        let changes = Diff::diff(self, &who, &r.from, &r.to).await.map_err(to_status)?;
        Ok(Response::new(DiffResponse {
            changes: changes
                .into_iter()
                .map(|c| pando_proto::FileChange { path: c.path, blob: c.blob, mode: c.mode })
                .collect(),
        }))
    }
}
