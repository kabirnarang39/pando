//! OIDC / JWT authentication: an [`Authenticator`] that accepts bearer tokens issued by
//! an OpenID Connect provider (Okta, Entra ID, Google, Auth0, Keycloak, GitHub Actions...).
//!
//! * Keys come from the issuer's JWKS (discovered via `/.well-known/openid-configuration`)
//!   and are refreshed periodically and when a token names an unknown key id.
//! * Only asymmetric algorithms are accepted, and the algorithm must match the key, so a
//!   public key can never be used as an HMAC secret (alg-confusion) and `alg: none` fails.
//! * `iss`, `aud` and `exp` are required; `nbf` is honoured; 60 s clock-skew leeway.
//! * The principal is a configurable claim (default `sub`), which the access policy maps
//!   to per-directory and per-ref permissions.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, bail};
use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use pando_core::acl::Principal;
use tonic::Status;

use crate::auth::Authenticator;

/// How often keys are re-fetched even if nothing looks wrong (providers rotate keys).
const REFRESH_EVERY: Duration = Duration::from_secs(600);
const LEEWAY_SECS: u64 = 60;

#[derive(Debug, Clone)]
pub struct OidcConfig {
    pub issuer: String,
    pub audience: String,
    /// Claim used as the principal name, e.g. `sub`, `email`, `preferred_username`.
    pub principal_claim: String,
}

pub struct OidcAuthenticator {
    cfg: OidcConfig,
    keys: Arc<RwLock<JwkSet>>,
    /// Signals the refresher that a token named an unknown key id.
    refresh: Option<tokio::sync::mpsc::Sender<()>>,
}

fn asymmetric(alg: Algorithm) -> bool {
    !matches!(alg, Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512)
}

async fn fetch_jwks(http: &reqwest::Client, url: &str) -> anyhow::Result<JwkSet> {
    let set = http.get(url).send().await?.error_for_status()?.json::<JwkSet>().await?;
    Ok(set)
}

impl OidcAuthenticator {
    /// Fixed key set (tests, air-gapped installs with a pinned JWKS file).
    pub fn with_jwks(cfg: OidcConfig, jwks: JwkSet) -> Self {
        Self { cfg, keys: Arc::new(RwLock::new(jwks)), refresh: None }
    }

    /// Discover the JWKS endpoint from the issuer and keep keys fresh in the background.
    pub async fn discover(cfg: OidcConfig) -> anyhow::Result<Self> {
        // reqwest uses rustls without a bundled provider; use ring, as the gRPC server does.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build()?;
        let well_known = format!("{}/.well-known/openid-configuration", cfg.issuer.trim_end_matches('/'));
        let meta: serde_json::Value = http
            .get(&well_known)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .with_context(|| format!("fetching {well_known}"))?
            .json()
            .await?;
        if meta["issuer"].as_str() != Some(cfg.issuer.as_str()) {
            bail!("issuer mismatch: discovery document says {:?}, configured {:?}", meta["issuer"], cfg.issuer);
        }
        let jwks_uri = meta["jwks_uri"].as_str().context("discovery document has no jwks_uri")?.to_owned();
        let keys = Arc::new(RwLock::new(fetch_jwks(&http, &jwks_uri).await?));
        let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(1);
        let shared = keys.clone();
        tokio::spawn(async move {
            loop {
                // Wake on schedule or when an unknown key id was seen (rate-limited by the
                // 1-slot channel and the fetch itself).
                let _ = tokio::time::timeout(REFRESH_EVERY, rx.recv()).await;
                match fetch_jwks(&http, &jwks_uri).await {
                    Ok(set) => *shared.write().expect("jwks lock") = set,
                    Err(e) => tracing::warn!(error = %e, "OIDC key refresh failed; keeping previous keys"),
                }
            }
        });
        Ok(Self { cfg, keys, refresh: Some(tx) })
    }

    fn key_for(&self, kid: Option<&str>) -> Option<Jwk> {
        let set = self.keys.read().expect("jwks lock");
        match kid {
            Some(kid) => set.find(kid).cloned(),
            // Tokens without a key id are accepted only when the issuer publishes one key.
            None if set.keys.len() == 1 => set.keys.first().cloned(),
            None => None,
        }
    }

    pub fn verify(&self, token: &str) -> Result<Principal, Status> {
        let reject = |why: &str| Status::unauthenticated(format!("invalid token: {why}"));
        let header = decode_header(token).map_err(|_| reject("malformed"))?;
        if !asymmetric(header.alg) {
            return Err(reject("symmetric algorithms are not accepted"));
        }
        let Some(jwk) = self.key_for(header.kid.as_deref()) else {
            if let Some(tx) = &self.refresh {
                let _ = tx.try_send(());
            }
            return Err(reject("unknown signing key"));
        };
        if let Some(key_alg) = jwk.common.key_algorithm
            && key_alg.to_string() != format!("{:?}", header.alg)
        {
            return Err(reject("algorithm does not match the signing key"));
        }
        let key = DecodingKey::from_jwk(&jwk).map_err(|_| reject("unusable signing key"))?;
        let mut v = Validation::new(header.alg);
        v.set_issuer(&[&self.cfg.issuer]);
        v.set_audience(&[&self.cfg.audience]);
        v.set_required_spec_claims(&["exp", "iss", "aud"]);
        v.validate_nbf = true;
        v.leeway = LEEWAY_SECS;
        let claims = decode::<serde_json::Value>(token, &key, &v).map_err(|e| reject(&e.to_string()))?.claims;
        let who = claims[&self.cfg.principal_claim]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| reject(&format!("missing `{}` claim", self.cfg.principal_claim)))?;
        Ok(Principal(who.to_owned()))
    }
}

impl Authenticator for OidcAuthenticator {
    fn authenticate(&self, bearer: Option<&str>) -> Result<Principal, Status> {
        self.verify(bearer.ok_or_else(|| Status::unauthenticated("missing bearer token"))?)
    }
}

/// Tries authenticators in order and accepts the first that does (e.g. static CI tokens
/// plus SSO users). Fails with the last error when none accept.
pub struct FirstMatch(pub Vec<Arc<dyn Authenticator>>);

impl Authenticator for FirstMatch {
    fn authenticate(&self, bearer: Option<&str>) -> Result<Principal, Status> {
        let mut last = Status::unauthenticated("no authenticator configured");
        for a in &self.0 {
            match a.authenticate(bearer) {
                Ok(p) => return Ok(p),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}
