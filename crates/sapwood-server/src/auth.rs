//! Request authentication. `Authenticator` is a port so static tokens can be swapped for
//! OIDC/JWT, mTLS identity, or a corporate IAM service without touching feature code.

use std::sync::Arc;

use tonic::service::Interceptor;
use tonic::{Request, Status};

pub trait Authenticator: Send + Sync {
    /// Ok when the bearer credential (if any) grants access.
    fn authenticate(&self, bearer: Option<&str>) -> Result<(), Status>;
}

/// Allows everything. Default for local development only.
pub struct AllowAll;
impl Authenticator for AllowAll {
    fn authenticate(&self, _: Option<&str>) -> Result<(), Status> {
        Ok(())
    }
}

/// Fixed set of bearer tokens (e.g. from `SAPWOOD_TOKENS`, comma separated).
pub struct StaticTokens(Vec<Vec<u8>>);

impl StaticTokens {
    pub fn new(tokens: impl IntoIterator<Item = String>) -> Self {
        Self(tokens.into_iter().filter(|t| !t.is_empty()).map(String::into_bytes).collect())
    }
}

/// Constant-time comparison so token checks do not leak prefix length via timing.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl Authenticator for StaticTokens {
    fn authenticate(&self, bearer: Option<&str>) -> Result<(), Status> {
        let presented = bearer.ok_or_else(|| Status::unauthenticated("missing bearer token"))?;
        // Check every token regardless of match to keep timing independent of position.
        let ok = self.0.iter().fold(false, |acc, t| acc | ct_eq(t, presented.as_bytes()));
        if ok { Ok(()) } else { Err(Status::unauthenticated("invalid token")) }
    }
}

#[derive(Clone)]
pub struct AuthInterceptor(pub Arc<dyn Authenticator>);

impl Interceptor for AuthInterceptor {
    fn call(&mut self, req: Request<()>) -> Result<Request<()>, Status> {
        let bearer = req
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        self.0.authenticate(bearer)?;
        Ok(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_tokens() {
        let a = StaticTokens::new(["s3cret".to_owned(), String::new()]);
        assert!(a.authenticate(Some("s3cret")).is_ok());
        assert!(a.authenticate(Some("s3cre")).is_err());
        assert!(a.authenticate(Some("")).is_err());
        assert!(a.authenticate(None).is_err());
        assert!(AllowAll.authenticate(None).is_ok());
    }
}
