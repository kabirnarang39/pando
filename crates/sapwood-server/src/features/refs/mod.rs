//! Refs slice: named pointers moved only by compare-and-swap.

use std::sync::Arc;

use sapwood_core::acl::{AccessPolicy, Principal, require_ref};
use sapwood_core::flags::{FeatureFlags, Flag, require};
use sapwood_core::{Error, Id, Repo, Result, validate_ref_name};
use sapwood_proto::refs_server::{self, RefsServer};
use sapwood_proto::{CasRequest, CasResponse, Empty, RefEntry, RefList, RefName, RefValue};
use tonic::{Request, Response};

use crate::auth::principal;
use crate::status::{RpcResult, id_from, to_status};

#[derive(Clone)]
pub struct Refs {
    repo: Repo,
    flags: Arc<dyn FeatureFlags>,
    policy: Arc<dyn AccessPolicy>,
}

impl Refs {
    pub fn new(repo: Repo, flags: Arc<dyn FeatureFlags>, policy: Arc<dyn AccessPolicy>) -> Self {
        Self { repo, flags, policy }
    }

    pub async fn get(&self, name: &str) -> Result<Option<Id>> {
        require(self.flags.as_ref(), Flag::Refs)?;
        validate_ref_name(name)?;
        self.repo.refs.get(name).await
    }

    /// The target must be an existing changeset: refs never dangle.
    pub async fn compare_and_swap(&self, who: &Principal, name: &str, expected: Option<Id>, new: Id) -> Result<bool> {
        require(self.flags.as_ref(), Flag::Refs)?;
        validate_ref_name(name)?;
        if self.flags.enabled(Flag::Permissions) {
            require_ref(self.policy.as_ref(), who, name)?;
        }
        if self.repo.log.get(&new).await?.is_none() {
            return Err(Error::not_found(format!("changeset {new}")));
        }
        self.repo.refs.compare_and_swap(name, expected, new).await
    }

    pub async fn list(&self) -> Result<Vec<(String, Id)>> {
        require(self.flags.as_ref(), Flag::Refs)?;
        self.repo.refs.list().await
    }
}

pub fn service(s: Refs) -> RefsServer<Refs> {
    RefsServer::new(s)
}

#[tonic::async_trait]
impl refs_server::Refs for Refs {
    async fn get(&self, req: Request<RefName>) -> RpcResult<RefValue> {
        let v = Refs::get(self, &req.get_ref().name).await.map_err(to_status)?;
        Ok(Response::new(RefValue { exists: v.is_some(), id: v.map(|i| i.0.to_vec()).unwrap_or_default() }))
    }

    async fn compare_and_swap(&self, req: Request<CasRequest>) -> RpcResult<CasResponse> {
        let who = principal(&req);
        let r = req.into_inner();
        let expected = if r.expect_exists { Some(id_from(&r.expected)?) } else { None };
        let swapped =
            Refs::compare_and_swap(self, &who, &r.name, expected, id_from(&r.new)?).await.map_err(to_status)?;
        Ok(Response::new(CasResponse { swapped }))
    }

    async fn list(&self, _: Request<Empty>) -> RpcResult<RefList> {
        let refs = Refs::list(self).await.map_err(to_status)?;
        Ok(Response::new(RefList {
            refs: refs.into_iter().map(|(name, id)| RefEntry { name, id: id.0.to_vec() }).collect(),
        }))
    }
}
