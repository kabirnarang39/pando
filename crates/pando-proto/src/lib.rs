//! Generated gRPC types for the Pando protocol (`proto/pando.proto`) plus the
//! transport settings and auth interceptor shared by every client and tier.
pub mod v1 {
    tonic::include_proto!("pando.v1");
}
/// Canonical object encodings (`proto/pando_objects.proto`), published for SDKs and
/// third-party tools. The core crate's own types must stay byte-compatible with these.
pub mod objects {
    tonic::include_proto!("pando.objects.v1");
}
pub use v1::*;

/// Encoded descriptor set for gRPC server reflection (grpcurl, Postman, etc.).
pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("pando_descriptor");

use tonic::metadata::{AsciiMetadataValue, MetadataValue};
use tonic::service::Interceptor;
use tonic::{Request, Status};

/// Largest single gRPC message: one max-size object plus framing.
pub const MAX_MESSAGE_BYTES: usize = 260 << 20;

/// Objects per streamed `Get`/`Missing` request chunk.
pub const OBJECT_BATCH: usize = 1024;

/// Attaches `authorization: Bearer <token>` when a token is configured.
#[derive(Clone, Default)]
pub struct BearerAuth(Option<AsciiMetadataValue>);

impl BearerAuth {
    pub fn new(token: Option<&str>) -> Result<Self, Status> {
        token
            .map(|t| MetadataValue::try_from(format!("Bearer {t}")).map_err(|_| Status::invalid_argument("token")))
            .transpose()
            .map(Self)
    }
}

impl Interceptor for BearerAuth {
    fn call(&mut self, mut req: Request<()>) -> Result<Request<()>, Status> {
        if let Some(v) = &self.0 {
            req.metadata_mut().insert("authorization", v.clone());
        }
        Ok(req)
    }
}
