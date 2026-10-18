//! Changesets slice: server-side commit creation. Clients upload blobs, then send only
//! the list of changed paths; the server builds trees and derived data.

use std::sync::Arc;

use pando_core::acl::{AccessPolicy, Principal, effective_scope, require_write};
use pando_core::flags::{FeatureFlags, Flag, require};
use pando_core::object::{Changeset, FileChange};
use pando_core::{Id, Repo, Result};
use pando_proto::changesets_server::{self, ChangesetsServer};
use pando_proto::{ChangesetInfo, CreateChangesetRequest, ResolveRequest};
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

    pub async fn get(&self, who: &Principal, rev: &str) -> Result<ChangesetInfo> {
        require(self.flags.as_ref(), Flag::Changesets)?;
        let scope = effective_scope(self.flags.as_ref(), self.policy.as_ref(), who);
        changeset_info(&self.repo, self.repo.resolve(rev).await?, &scope).await
    }
}

pub fn service(s: Changesets) -> ChangesetsServer<Changesets> {
    ChangesetsServer::new(s).max_decoding_message_size(crate::max_request_bytes())
}

#[tonic::async_trait]
impl changesets_server::Changesets for Changesets {
    async fn create(&self, req: Request<CreateChangesetRequest>) -> RpcResult<ChangesetInfo> {
        let who = principal(&req);
        let r = req.into_inner();
        let cs = Changeset {
            parents: r.parents,
            changes: r.changes.into_iter().map(|c| FileChange { path: c.path, blob: c.blob, mode: c.mode }).collect(),
            // Unnamed changes are attributed to whoever made them; the access policy may insist
            // on that for every change (platform servers do, except for administrators).
            author: if r.author.trim().is_empty() {
                who.0.clone()
            } else if self.flags.enabled(Flag::Permissions) {
                self.policy.author_for(&who, &r.author)
            } else {
                r.author
            },
            message: r.message,
            timestamp_ms: r.timestamp_ms,
        };
        let id = Changesets::create(self, &who, cs).await.map_err(to_status)?;
        let scope = effective_scope(self.flags.as_ref(), self.policy.as_ref(), &who);
        Ok(Response::new(changeset_info(&self.repo, id, &scope).await.map_err(to_status)?))
    }

    async fn get(&self, req: Request<ResolveRequest>) -> RpcResult<ChangesetInfo> {
        let who = principal(&req);
        Ok(Response::new(Changesets::get(self, &who, &req.get_ref().rev).await.map_err(to_status)?))
    }
}
