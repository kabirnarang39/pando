//! Access slice: tells clients who they are and what they may read, so `pando` can switch to
//! path-scoped checkout for principals with restricted read access.

use std::sync::Arc;

use pando_core::acl::{AccessPolicy, ReadScope, effective_scope};
use pando_core::flags::FeatureFlags;
use pando_proto::access_server::{self, AccessServer};
use pando_proto::{Empty, WhoAmIResponse};
use tonic::{Request, Response};

use crate::auth::principal;
use crate::status::RpcResult;

#[derive(Clone)]
pub struct Access {
    flags: Arc<dyn FeatureFlags>,
    policy: Arc<dyn AccessPolicy>,
}

impl Access {
    pub fn new(flags: Arc<dyn FeatureFlags>, policy: Arc<dyn AccessPolicy>) -> Self {
        Self { flags, policy }
    }
}

pub fn service(s: Access) -> AccessServer<Access> {
    AccessServer::new(s)
}

#[tonic::async_trait]
impl access_server::Access for Access {
    async fn who_am_i(&self, req: Request<Empty>) -> RpcResult<WhoAmIResponse> {
        let who = principal(&req);
        let scope = effective_scope(self.flags.as_ref(), self.policy.as_ref(), &who);
        let (full_read, read_prefixes) = match scope {
            ReadScope::All => (true, Vec::new()),
            ReadScope::Prefixes(p) => (false, p),
        };
        Ok(Response::new(WhoAmIResponse { principal: who.0, full_read, read_prefixes }))
    }
}
