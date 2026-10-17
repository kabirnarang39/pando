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
pub struct BearerAuth {
    token: Option<AsciiMetadataValue>,
    repo: Option<AsciiMetadataValue>,
}

/// The request header that names the repository a call is about, on servers hosting many.
pub const REPO_HEADER: &str = "x-pando-repo";

impl BearerAuth {
    pub fn new(token: Option<&str>) -> Result<Self, Status> {
        let token = token
            .map(|t| MetadataValue::try_from(format!("Bearer {t}")).map_err(|_| Status::invalid_argument("token")))
            .transpose()?;
        Ok(Self { token, repo: None })
    }

    /// Names the repository (`OWNER/NAME`) on every call.
    pub fn with_repo(mut self, repo: Option<&str>) -> Result<Self, Status> {
        self.repo = repo
            .map(|r| MetadataValue::try_from(r).map_err(|_| Status::invalid_argument(format!("repository `{r}`"))))
            .transpose()?;
        Ok(self)
    }
}

impl Interceptor for BearerAuth {
    fn call(&mut self, mut req: Request<()>) -> Result<Request<()>, Status> {
        if let Some(v) = &self.token {
            req.metadata_mut().insert("authorization", v.clone());
        }
        if let Some(v) = &self.repo {
            req.metadata_mut().insert(REPO_HEADER, v.clone());
        }
        Ok(req)
    }
}

/// Splits `https://host[:port]/OWNER/NAME` into the server address and the repository. A URL
/// with no path is the server's default repository; any other path is left alone.
pub fn split_repo_url(url: &str) -> (String, Option<String>) {
    let trimmed = url.trim_end_matches('/');
    let Some(scheme_end) = trimmed.find("://") else { return (trimmed.to_owned(), None) };
    let rest = &trimmed[scheme_end + 3..];
    let Some(slash) = rest.find('/') else { return (trimmed.to_owned(), None) };
    let path = &rest[slash + 1..];
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() == 2 && parts.iter().all(|p| !p.is_empty()) {
        (trimmed[..scheme_end + 3 + slash].to_owned(), Some(path.to_owned()))
    } else {
        (trimmed.to_owned(), None)
    }
}

#[cfg(test)]
mod repo_url_tests {
    use super::split_repo_url;

    #[test]
    fn splits_owner_and_name_from_the_server_address() {
        assert_eq!(
            split_repo_url("https://pando.example.com/lumen/fleet"),
            ("https://pando.example.com".into(), Some("lumen/fleet".into()))
        );
        assert_eq!(split_repo_url("http://127.0.0.1:7420/a/b/"), ("http://127.0.0.1:7420".into(), Some("a/b".into())));
        assert_eq!(split_repo_url("http://127.0.0.1:7420"), ("http://127.0.0.1:7420".into(), None));
        assert_eq!(split_repo_url("http://127.0.0.1:7420/"), ("http://127.0.0.1:7420".into(), None));
        assert_eq!(split_repo_url("http://h/one"), ("http://h/one".into(), None));
    }
}
