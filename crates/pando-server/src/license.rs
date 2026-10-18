//! Support licences, checked offline (ADR-035).
//!
//! A licence file is JSON: `{"license": BASE64(payload JSON), "signature": BASE64(Ed25519)}`,
//! signed over the payload's exact bytes, so nothing depends on how JSON is formatted. A licence
//! is for support, not a feature lock: a missing, expired or invalid one only shows in the logs
//! and in `GetLicense`; nothing is turned off.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;

/// The key licences are signed with: base64 of a 32-byte Ed25519 public key. The project owner
/// replaces this placeholder with the real key; until then every licence is reported invalid.
pub const PANDO_LICENSE_PUBLIC_KEY: &str = "REPLACE-WITH-THE-BASE64-ED25519-PUBLIC-KEY";

/// The only product a licence is checked for.
pub const PRODUCT: &str = "self-hosted-support";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct License {
    pub licensee: String,
    pub product: String,
    pub seats: u32,
    pub agents: u32,
    /// Unix seconds.
    pub issued_at: i64,
    pub expires_at: i64,
    pub grace_days: u32,
}

#[derive(Deserialize)]
struct File {
    license: String,
    signature: String,
}

/// The licence in `file`, if `public_key` (base64) signed it for this product.
pub fn verify(file: &[u8], public_key: &str) -> Result<License, String> {
    let key = STANDARD
        .decode(public_key.trim())
        .ok()
        .filter(|k| k.len() == 32)
        .ok_or("this build has no licence public key (PANDO_LICENSE_PUBLIC_KEY is a placeholder)")?;
    let f: File = serde_json::from_slice(file).map_err(|e| format!("not a licence file: {e}"))?;
    let payload = STANDARD.decode(f.license.trim()).map_err(|_| "the licence is not base64")?;
    let sig = STANDARD.decode(f.signature.trim()).map_err(|_| "the signature is not base64")?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &key)
        .verify(&payload, &sig)
        .map_err(|_| "the signature does not match: the licence was changed or not issued for Pando")?;
    let lic: License = serde_json::from_slice(&payload).map_err(|e| format!("the licence is malformed: {e}"))?;
    if lic.product != PRODUCT {
        return Err(format!("the licence is for `{}`, not {PRODUCT}", lic.product));
    }
    Ok(lic)
}

/// `valid`, `grace` (expired, within its grace days) or `expired`, at `now` (Unix seconds).
pub fn status(lic: &License, now: i64) -> &'static str {
    if now < lic.expires_at {
        "valid"
    } else if now < lic.expires_at + i64::from(lic.grace_days) * 86_400 {
        "grace"
    } else {
        "expired"
    }
}

pub fn now_s() -> i64 {
    crate::hosting::now_ms() / 1000
}

/// Reads and checks a licence file with the compiled-in key, logging what it found.
pub fn load(path: &std::path::Path) -> Result<License, String> {
    let checked = std::fs::read(path)
        .map_err(|e| format!("reading {}: {e}", path.display()))
        .and_then(|b| verify(&b, PANDO_LICENSE_PUBLIC_KEY));
    match &checked {
        Ok(l) => match status(l, now_s()) {
            "valid" => tracing::info!(licensee = %l.licensee, expires_at = l.expires_at, "support licence valid"),
            "grace" => {
                tracing::warn!(licensee = %l.licensee, "support licence expired; renew it within its grace period")
            }
            _ => tracing::warn!(
                licensee = %l.licensee,
                "support licence expired; Pando keeps working, but support has ended: renew it"
            ),
        },
        Err(e) => tracing::warn!(error = %e, "support licence not valid; Pando keeps working"),
    }
    checked
}

/// What `GetLicense` answers.
pub fn to_proto(checked: Option<&Result<License, String>>) -> pando_proto::License {
    match checked {
        None => pando_proto::License::default(),
        Some(Err(e)) => {
            pando_proto::License { present: true, status: "invalid".into(), error: e.clone(), ..Default::default() }
        }
        Some(Ok(l)) => pando_proto::License {
            present: true,
            status: status(l, now_s()).into(),
            error: String::new(),
            licensee: l.licensee.clone(),
            product: l.product.clone(),
            seats: l.seats,
            agents: l.agents,
            issued_at: l.issued_at,
            expires_at: l.expires_at,
            grace_days: l.grace_days,
        },
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    /// A throwaway key pair and a licence signed with it, for tests only.
    pub(crate) fn signed(payload: &serde_json::Value) -> (Vec<u8>, String) {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let bytes = serde_json::to_vec(payload).unwrap();
        let file = serde_json::json!({
            "license": STANDARD.encode(&bytes),
            "signature": STANDARD.encode(pair.sign(&bytes).as_ref()),
        });
        (serde_json::to_vec(&file).unwrap(), STANDARD.encode(pair.public_key().as_ref()))
    }

    fn payload(expires_at: i64) -> serde_json::Value {
        serde_json::json!({
            "licensee": "Lumen Labs", "product": PRODUCT, "seats": 25, "agents": 200,
            "issued_at": 1_700_000_000, "expires_at": expires_at, "grace_days": 14,
        })
    }

    #[test]
    fn signed_licences_verify_and_expire_after_grace() {
        let (file, key) = signed(&payload(2_000_000_000));
        let lic = verify(&file, &key).unwrap();
        assert_eq!((lic.licensee.as_str(), lic.seats, lic.agents), ("Lumen Labs", 25, 200));
        assert_eq!(status(&lic, 1_900_000_000), "valid");
        assert_eq!(status(&lic, 2_000_000_000 + 86_400), "grace");
        assert_eq!(status(&lic, 2_000_000_000 + 15 * 86_400), "expired");
    }

    #[test]
    fn tampered_foreign_or_unkeyed_licences_are_refused() {
        let (file, key) = signed(&payload(2_000_000_000));
        let (_, other_key) = signed(&payload(2_000_000_000));
        assert!(verify(&file, &other_key).unwrap_err().contains("does not match"));
        let mut f: serde_json::Value = serde_json::from_slice(&file).unwrap();
        let mut p = payload(2_100_000_000);
        p["seats"] = 9999.into();
        f["license"] = STANDARD.encode(serde_json::to_vec(&p).unwrap()).into();
        assert!(verify(&serde_json::to_vec(&f).unwrap(), &key).unwrap_err().contains("does not match"));
        assert!(verify(&file, PANDO_LICENSE_PUBLIC_KEY).unwrap_err().contains("placeholder"));
        let mut wrong = payload(2_000_000_000);
        wrong["product"] = "enterprise".into();
        let (file, key) = signed(&wrong);
        assert!(verify(&file, &key).unwrap_err().contains("enterprise"));
    }
}
