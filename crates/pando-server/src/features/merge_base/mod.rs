//! Merge-base slice: best common ancestor of two revisions (generation-number search
//! with an O(log n) jump-pointer fast path for first-parent ancestry).

use std::sync::Arc;

use pando_core::flags::{FeatureFlags, Flag, require};
use pando_core::{Id, Repo, Result};
use pando_proto::merge_base_server::{self, MergeBaseServer};
use pando_proto::{MergeBaseRequest, MergeBaseResponse};
use tonic::{Request, Response};

use crate::status::{RpcResult, to_status};

#[derive(Clone)]
pub struct MergeBase {
    repo: Repo,
    flags: Arc<dyn FeatureFlags>,
}

impl MergeBase {
    pub fn new(repo: Repo, flags: Arc<dyn FeatureFlags>) -> Self {
        Self { repo, flags }
    }

    pub async fn merge_base(&self, a: &str, b: &str) -> Result<Option<Id>> {
        require(self.flags.as_ref(), Flag::MergeBase)?;
        let (a, b) = futures::try_join!(self.repo.resolve(a), self.repo.resolve(b))?;
        self.repo.merge_base(a, b).await
    }
}

pub fn service(s: MergeBase) -> MergeBaseServer<MergeBase> {
    MergeBaseServer::new(s)
}

#[tonic::async_trait]
impl merge_base_server::MergeBase for MergeBase {
    async fn merge_base(&self, req: Request<MergeBaseRequest>) -> RpcResult<MergeBaseResponse> {
        let r = req.into_inner();
        let found = MergeBase::merge_base(self, &r.a, &r.b).await.map_err(to_status)?;
        Ok(Response::new(MergeBaseResponse {
            found: found.is_some(),
            id: found.map(|i| i.0.to_vec()).unwrap_or_default(),
        }))
    }
}
