//! Webhook pre-land check: the server POSTs each proposed land to a URL (a CI gate, a code
//! review service, a policy bot) and lands only on a 2xx answer. Any other answer, a timeout
//! or an unreachable hook refuses the land (fail closed); the response body becomes the
//! reason shown to the author.

use std::time::Duration;

use sapwood_core::ports::{LandCheck, LandProposal};
use sapwood_core::{Error, Result};

/// Paths sent per proposal; larger changes set `paths_truncated`.
pub const MAX_PATHS: usize = 10_000;
const MAX_REASON: usize = 500;

pub struct WebhookCheck {
    http: reqwest::Client,
    url: String,
    token: Option<String>,
}

impl WebhookCheck {
    pub fn new(url: &str, token: Option<String>, timeout: Duration) -> anyhow::Result<Self> {
        // reqwest uses rustls without a bundled provider; use ring, as the gRPC server does.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self { http, url: url.to_owned(), token })
    }
}

pub fn payload(p: &LandProposal) -> serde_json::Value {
    serde_json::json!({
        "ref": p.ref_name,
        "changeset": p.changeset.to_hex(),
        "parent": p.parent.map(|id| id.to_hex()),
        "principal": p.principal,
        "author": p.author,
        "message": p.message,
        "paths": &p.paths[..p.paths.len().min(MAX_PATHS)],
        "paths_truncated": p.paths.len() > MAX_PATHS,
    })
}

#[tonic::async_trait]
impl LandCheck for WebhookCheck {
    async fn check(&self, p: &LandProposal) -> Result<()> {
        let mut req = self.http.post(&self.url).json(&payload(p));
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await.map_err(|e| {
            tracing::warn!(error = %e.without_url(), "land check unreachable");
            Error::Rejected("land check unavailable; try again later".into())
        })?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let body = resp.text().await.unwrap_or_default();
        let reason: String = body.trim().chars().take(MAX_REASON).collect();
        Err(Error::Rejected(if reason.is_empty() { format!("land check answered HTTP {status}") } else { reason }))
    }
}
