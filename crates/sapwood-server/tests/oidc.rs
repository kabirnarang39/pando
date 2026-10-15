//! OIDC authenticator: accepted and rejected tokens, discovery, and key rotation.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use sapwood_server::auth::Authenticator;
use sapwood_server::oidc::{FirstMatch, OidcAuthenticator, OidcConfig};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const KEY: &[u8] = include_bytes!("fixtures/test-oidc-key.pem");
const PUB: &[u8] = include_bytes!("fixtures/test-oidc-pub.pem");
const JWKS: &str = include_str!("fixtures/test-oidc-jwks.json");
const ISS: &str = "https://idp.example.com";
const AUD: &str = "sapwood";

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn claims() -> Value {
    json!({ "iss": ISS, "aud": AUD, "sub": "user-123", "email": "dev@example.com", "exp": now() + 300 })
}

fn sign(claims: &Value, kid: Option<&str>) -> String {
    let mut h = Header::new(Algorithm::RS256);
    h.kid = kid.map(str::to_owned);
    encode(&h, claims, &EncodingKey::from_rsa_pem(KEY).unwrap()).unwrap()
}

fn cfg(claim: &str) -> OidcConfig {
    OidcConfig { issuer: ISS.into(), audience: AUD.into(), principal_claim: claim.into() }
}

fn auth(claim: &str) -> OidcAuthenticator {
    OidcAuthenticator::with_jwks(cfg(claim), serde_json::from_str::<JwkSet>(JWKS).unwrap())
}

fn rejected(a: &OidcAuthenticator, token: &str) -> String {
    a.verify(token).expect_err("token must be rejected").message().to_owned()
}

#[test]
fn accepts_valid_tokens_and_maps_the_configured_claim() {
    assert_eq!(auth("sub").verify(&sign(&claims(), Some("test-key-1"))).unwrap().0, "user-123");
    assert_eq!(auth("email").verify(&sign(&claims(), Some("test-key-1"))).unwrap().0, "dev@example.com");
    // A single published key may be used by tokens without a key id.
    assert!(auth("sub").verify(&sign(&claims(), None)).is_ok());
    // Within the clock-skew leeway.
    let mut c = claims();
    c["exp"] = json!(now() - 30);
    assert!(auth("sub").verify(&sign(&c, Some("test-key-1"))).is_ok());
}

#[test]
fn rejects_bad_claims() {
    let a = auth("sub");
    let mut c = claims();
    c["exp"] = json!(now() - 3600);
    assert!(rejected(&a, &sign(&c, Some("test-key-1"))).contains("Expired"));
    let mut c = claims();
    c["aud"] = json!("someone-else");
    assert!(rejected(&a, &sign(&c, Some("test-key-1"))).contains("Audience"));
    let mut c = claims();
    c["iss"] = json!("https://evil.example.com");
    assert!(rejected(&a, &sign(&c, Some("test-key-1"))).contains("Issuer"));
    let mut c = claims();
    c.as_object_mut().unwrap().remove("exp");
    assert!(a.verify(&sign(&c, Some("test-key-1"))).is_err(), "exp is required");
    let mut c = claims();
    c["nbf"] = json!(now() + 3600);
    assert!(rejected(&a, &sign(&c, Some("test-key-1"))).contains("Immature"));
    let mut c = claims();
    c.as_object_mut().unwrap().remove("sub");
    assert!(rejected(&a, &sign(&c, Some("test-key-1"))).contains("missing `sub`"));
}

#[test]
fn rejects_forged_and_confused_tokens() {
    let a = auth("sub");
    assert!(rejected(&a, &sign(&claims(), Some("other-key"))).contains("unknown signing key"));
    assert!(rejected(&a, "not.a.jwt").contains("malformed"));

    // Tampered payload: signature no longer matches.
    let good = sign(&claims(), Some("test-key-1"));
    let parts: Vec<&str> = good.split('.').collect();
    let mut evil = claims();
    evil["sub"] = json!("admin");
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let tampered = format!("{}.{}.{}", parts[0], b64.encode(evil.to_string()), parts[2]);
    assert!(a.verify(&tampered).is_err());

    // alg=none, unsigned.
    let none = format!("{}.{}.", b64.encode(r#"{"alg":"none","kid":"test-key-1"}"#), b64.encode(claims().to_string()));
    assert!(a.verify(&none).is_err());

    // Key confusion: HMAC "signed" with the public key as the shared secret.
    let mut h = Header::new(Algorithm::HS256);
    h.kid = Some("test-key-1".into());
    let confused = encode(&h, &claims(), &EncodingKey::from_secret(PUB)).unwrap();
    assert!(rejected(&a, &confused).contains("symmetric"));
}

#[test]
fn first_match_combines_static_tokens_and_sso() {
    let combined = FirstMatch(vec![
        Arc::new(sapwood_server::auth::StaticTokens::new(["ci:ci-secret".to_owned()])),
        Arc::new(auth("email")),
    ]);
    assert_eq!(combined.authenticate(Some("ci-secret")).unwrap().0, "ci");
    assert_eq!(combined.authenticate(Some(&sign(&claims(), Some("test-key-1")))).unwrap().0, "dev@example.com");
    assert!(combined.authenticate(Some("nope")).is_err());
    assert!(combined.authenticate(None).is_err());
}

/// Minimal HTTP/1.1 server for the discovery document and a swappable JWKS.
async fn idp(jwks: Arc<Mutex<String>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let issuer = base.clone();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let (jwks, issuer) = (jwks.clone(), issuer.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let body = if req.starts_with("GET /.well-known/openid-configuration") {
                    json!({ "issuer": issuer, "jwks_uri": format!("{issuer}/jwks") }).to_string()
                } else {
                    jwks.lock().unwrap().clone()
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    base
}

#[tokio::test(flavor = "multi_thread")]
async fn discovery_and_rotation_on_unknown_key_id() {
    // The IdP initially publishes the key under id "old"; tokens arrive signed as "new".
    let rotated = JWKS.replace("test-key-1", "new");
    let jwks = Arc::new(Mutex::new(JWKS.replace("test-key-1", "old")));
    let issuer = idp(jwks.clone()).await;
    let cfg = OidcConfig { issuer: issuer.clone(), audience: AUD.into(), principal_claim: "sub".into() };
    let a = OidcAuthenticator::discover(cfg.clone()).await.unwrap();
    let mut c = claims();
    c["iss"] = json!(issuer);
    let token = sign(&c, Some("new"));
    assert!(a.verify(&token).is_err(), "unknown key id before rotation");
    *jwks.lock().unwrap() = rotated;
    // The rejection above signalled a refresh; the new key appears without a restart.
    let mut ok = false;
    for _ in 0..50 {
        let _ = a.verify(&token); // re-signal in case the first fetch raced the swap
        if a.verify(&token).is_ok() {
            ok = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(ok, "rotated key was not picked up");

    // A discovery document claiming a different issuer is refused.
    let wrong = OidcConfig { issuer: format!("{issuer}/"), ..cfg };
    assert!(OidcAuthenticator::discover(wrong).await.is_err());
}
