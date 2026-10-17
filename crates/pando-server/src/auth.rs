//! Request authentication. `Authenticator` is a port so static tokens can be swapped for
//! OIDC/JWT, mTLS identity, or a corporate IAM service without touching feature code.

use std::sync::Arc;

use pando_core::acl::Principal;

pub use crate::directory::TokenScope;
use tonic::service::Interceptor;
use tonic::{Request, Status};

pub trait Authenticator: Send + Sync {
    /// The caller's identity when the bearer credential (if any) grants access.
    fn authenticate(&self, bearer: Option<&str>) -> Result<Principal, Status>;

    /// The identity and, for a scoped credential such as a platform token, what it is limited
    /// to.
    fn identify(&self, bearer: Option<&str>) -> Result<(Principal, Option<TokenScope>), Status> {
        self.authenticate(bearer).map(|p| (p, None))
    }
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

/// Shortest static token accepted at startup.
const MIN_STATIC_TOKEN: usize = 16;

/// Refuses static tokens that are empty, short or a placeholder left from a template
/// (`CHANGE-ME`, `changeme`, ...), naming the principal but never the token.
pub fn check_static_tokens(spec: &str) -> anyhow::Result<()> {
    for e in spec.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let (who, tok) = e.split_once(':').unwrap_or(("default", e));
        let placeholder = tok.to_ascii_lowercase().replace(['-', '_'], "").contains("changeme");
        if tok.len() < MIN_STATIC_TOKEN || placeholder {
            anyhow::bail!(
                "the PANDO_TOKENS entry for `{who}` is {}; use a random secret of at least {MIN_STATIC_TOKEN} characters, e.g. `{who}:$(openssl rand -hex 24)`",
                if placeholder { "a placeholder" } else { "empty or too short" }
            );
        }
    }
    Ok(())
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
        let (who, scope) = self.0.identify(bearer).inspect_err(|e| {
            let peer = req.remote_addr().map(|a| a.ip().to_string()).unwrap_or_default();
            let presented = bearer.is_some();
            tracing::info!(target: crate::telemetry::AUDIT, kind = "auth_failed", peer, presented, reason = e.message());
        })?;
        req.extensions_mut().insert(who);
        if let Some(scope) = scope {
            req.extensions_mut().insert(scope);
        }
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

    /// A rejected request leaves an audit record, and never the token it presented.
    #[test]
    fn failed_authentication_is_audited() {
        #[derive(Clone, Default)]
        struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Captured {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let out = Captured::default();
        let writer = out.clone();
        let subscriber = tracing_subscriber::fmt().json().with_writer(move || writer.clone()).finish();
        let mut auth = AuthInterceptor(Arc::new(StaticTokens::new(["alice:al1ce".to_owned()])));
        tracing::subscriber::with_default(subscriber, || {
            let mut req = Request::new(());
            req.metadata_mut().insert("authorization", "Bearer guessed-token".parse().unwrap());
            assert!(auth.call(req).is_err());
        });
        let log = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        assert!(log.contains("auth_failed") && log.contains("invalid token"), "{log}");
        assert!(!log.contains("guessed-token"), "the presented token must not be logged: {log}");
    }

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

    #[test]
    fn weak_static_tokens_are_refused() {
        for bad in
            ["admin:CHANGE-ME", "admin:changeme", "admin:", "abc", "ci:0123456789abcde", "a:xx-Change_Me-xxxxxxxxxx"]
        {
            let e = check_static_tokens(bad).unwrap_err().to_string();
            let tok = bad.split_once(':').map_or(bad, |p| p.1);
            assert!(tok.is_empty() || !e.contains(tok), "never echo the token: {e}");
        }
        check_static_tokens("alice:0123456789abcdef0123, 9f8e7d6c5b4a39281706,").unwrap();
    }
}
