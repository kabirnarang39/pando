//! Changesets slice: server-side commit creation. Clients upload blobs, then send only
//! the list of changed paths; the server builds trees and derived data.

use std::sync::Arc;

use sapwood_core::acl::{AccessPolicy, Principal, require_write};
use sapwood_core::flags::{FeatureFlags, Flag, require};
use sapwood_core::object::{Changeset, FileChange};
use sapwood_core::{Id, Repo, Result};
use sapwood_proto::changesets_server::{self, ChangesetsServer};
use sapwood_proto::{ChangesetInfo, CreateChangesetRequest, ResolveRequest};
use tonic::{Request, Response};

use crate::auth::principal;
use crate::status::{RpcResult, changeset_info, to_status};

#[derive(Clone)]
pub struct Changesets {
    repo: Repo,
    flags: Arc<dyn FeatureFlags>,
    policy: Arc<dyn AccessPolicy>,
}

impl Changesets {
    pub fn new(repo: Repo, flags: Arc<dyn FeatureFlags>, policy: Arc<dyn AccessPolicy>) -> Self {
        Self { repo, flags, policy }
    }

    /// Per-directory write permissions are enforced on every changed path.
    pub async fn create(&self, who: &Principal, cs: Changeset) -> Result<Id> {
        require(self.flags.as_ref(), Flag::Changesets)?;
        if self.flags.enabled(Flag::Permissions) {
            require_write(self.policy.as_ref(), who, cs.changes.iter().map(|c| c.path.as_str()))?;
        }
        self.repo.create_changeset(cs).await
    }

    pub async fn get(&self, rev: &str) -> Result<ChangesetInfo> {
        require(self.flags.as_ref(), Flag::Changesets)?;
        changeset_info(&self.repo, self.repo.resolve(rev).await?).await
    }
}

pub fn service(s: Changesets) -> ChangesetsServer<Changesets> {
    ChangesetsServer::new(s).max_decoding_message_size(sapwood_proto::MAX_MESSAGE_BYTES)
}

#[tonic::async_trait]
impl changesets_server::Changesets for Changesets {
    async fn create(&self, req: Request<CreateChangesetRequest>) -> RpcResult<ChangesetInfo> {
        let who = principal(&req);
        let r = req.into_inner();
        let cs = Changeset {
            parents: r.parents,
            changes: r.changes.into_iter().map(|c| FileChange { path: c.path, blob: c.blob, mode: c.mode }).collect(),
            author: r.author,
            message: r.message,
            timestamp_ms: r.timestamp_ms,
        };
        let id = Changesets::create(self, &who, cs).await.map_err(to_status)?;
        Ok(Response::new(changeset_info(&self.repo, id).await.map_err(to_status)?))
    }

    async fn get(&self, req: Request<ResolveRequest>) -> RpcResult<ChangesetInfo> {
        Ok(Response::new(Changesets::get(self, &req.get_ref().rev).await.map_err(to_status)?))
    }
}
