//! Request authentication. `Authenticator` is a port so static tokens can be swapped for
//! OIDC/JWT, mTLS identity, or a corporate IAM service without touching feature code.

use std::sync::Arc;

use pando_core::acl::Principal;
use tonic::service::Interceptor;
use tonic::{Request, Status};

pub trait Authenticator: Send + Sync {
    /// The caller's identity when the bearer credential (if any) grants access.
    fn authenticate(&self, bearer: Option<&str>) -> Result<Principal, Status>;
}

/// Allows everything. Default for local development only.
pub struct AllowAll;
impl Authenticator for AllowAll {
    fn authenticate(&self, _: Option<&str>) -> Result<Principal, Status> {
        Ok(Principal::anonymous())
    }
}

/// Fixed bearer tokens, each bound to a principal. Spec (e.g. `PANDO_TOKENS`):
/// comma-separated `principal:token` pairs; a bare `token` maps to principal `default`.
pub struct StaticTokens(Vec<(Principal, Vec<u8>)>);

impl StaticTokens {
    pub fn new(entries: impl IntoIterator<Item = String>) -> Self {
        Self(
            entries
                .into_iter()
                .filter(|t| !t.is_empty())
                .map(|e| match e.split_once(':') {
                    Some((who, tok)) => (Principal(who.to_owned()), tok.as_bytes().to_vec()),
                    None => (Principal("default".into()), e.into_bytes()),
                })
                .filter(|(_, t)| !t.is_empty())
                .collect(),
        )
    }
}

/// Constant-time comparison so token checks do not leak prefix length via timing.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl Authenticator for StaticTokens {
    fn authenticate(&self, bearer: Option<&str>) -> Result<Principal, Status> {
        let presented = bearer.ok_or_else(|| Status::unauthenticated("missing bearer token"))?;
        // Compare against every token regardless of match so timing is position-independent.
        let mut found = None;
        for (who, t) in &self.0 {
            if ct_eq(t, presented.as_bytes()) && found.is_none() {
                found = Some(who.clone());
            }
        }
        found.ok_or_else(|| Status::unauthenticated("invalid token"))
    }
}

#[derive(Clone)]
pub struct AuthInterceptor(pub Arc<dyn Authenticator>);

impl Interceptor for AuthInterceptor {
    fn call(&mut self, mut req: Request<()>) -> Result<Request<()>, Status> {
        let bearer =
            req.metadata().get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
        let who = self.0.authenticate(bearer)?;
        req.extensions_mut().insert(who);
        Ok(req)
    }
}

/// Caller identity set by [`AuthInterceptor`].
pub fn principal<T>(req: &Request<T>) -> Principal {
    req.extensions().get::<Principal>().cloned().unwrap_or_else(Principal::anonymous)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_tokens() {
        let a = StaticTokens::new(["s3cret".to_owned(), String::new(), "alice:al1ce".to_owned()]);
        assert_eq!(a.authenticate(Some("s3cret")).unwrap(), Principal("default".into()));
        assert_eq!(a.authenticate(Some("al1ce")).unwrap(), Principal("alice".into()));
        assert!(a.authenticate(Some("s3cre")).is_err());
        assert!(a.authenticate(Some("")).is_err());
        assert!(a.authenticate(None).is_err());
        assert!(AllowAll.authenticate(None).is_ok());
    }
}
