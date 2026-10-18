//! `pando login`: the OAuth device authorization grant (RFC 8628). The CLI asks for a code, the
//! person opens `/login/device`, signs in and approves, and the CLI, polling, gets a platform
//! token once. Only the device code's hash is stored; requests last 10 minutes.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Form, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use super::store::Poll;
use super::{Login, Pending, broken, client_ip, esc, fail, hash, page, random_hex};
use crate::directory::TokenRec;
use crate::hosting::now_ms;

const TTL_MS: i64 = 10 * 60_000;
const INTERVAL_S: i64 = 5;
const TOKEN_DAYS: i64 = 90;
/// Consonants only, without look-alikes: codes can't spell words or be misread (RFC 8628 §6.1).
const ALPHABET: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ";
const NEXT: &str = "/login/device?user_code=";
const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// A user code as typed: upper-cased, with the dash put in; None unless it could be one.
fn normal_code(s: &str) -> Option<String> {
    let c: String = s.chars().filter(|c| !matches!(c, '-' | ' ')).map(|c| c.to_ascii_uppercase()).collect();
    (c.len() == 8 && c.bytes().all(|b| ALPHABET.contains(&b))).then(|| format!("{}-{}", &c[..4], &c[4..]))
}

/// The user code a sign-in is for, if it goes on to approving a CLI.
pub(super) fn code_in(next: &str) -> Option<&str> {
    next.strip_prefix(NEXT).filter(|c| normal_code(c).as_deref() == Some(*c))
}

fn user_code() -> anyhow::Result<String> {
    let mut out = Vec::with_capacity(8);
    while out.len() < 8 {
        let mut b = [0u8; 16];
        getrandom::fill(&mut b)?;
        // Rejection sampling keeps every letter equally likely.
        let limit = (256 / ALPHABET.len() * ALPHABET.len()) as u8;
        out.extend(b.iter().filter(|&&x| x < limit).map(|&x| ALPHABET[x as usize % ALPHABET.len()]));
    }
    let c = String::from_utf8(out[..8].to_vec())?;
    Ok(format!("{}-{}", &c[..4], &c[4..]))
}

/// Printable ASCII, at most `max` characters, for showing what a client says about itself.
fn label(s: &str, max: usize) -> String {
    s.chars().filter(|c| c.is_ascii_graphic() || *c == ' ').take(max).collect::<String>().trim().to_owned()
}

fn oauth_error(status: StatusCode, error: &str) -> Response {
    (status, axum::Json(json!({ "error": error }))).into_response()
}

#[derive(Deserialize)]
pub(super) struct CodeRequest {
    /// The machine the CLI runs on; it names the token (`cli on HOST`).
    host: Option<String>,
}

/// `POST /login/device/code`: a new CLI sign-in.
pub(super) async fn code(
    State(l): State<Arc<Login>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(f): Form<CodeRequest>,
) -> Response {
    let ip = client_ip(peer, &headers);
    if !l.buckets.take(format!("device:{ip}"), 10.0, Duration::from_secs(600)) {
        return oauth_error(StatusCode::TOO_MANY_REQUESTS, "slow_down");
    }
    let client = label(headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or(""), 120);
    let host = label(f.host.as_deref().unwrap_or(""), 64);
    let host = if host.is_empty() { "an unnamed machine".to_owned() } else { host };
    let Ok(device_code) = random_hex() else { return broken("no randomness") };
    for _ in 0..5 {
        let Ok(uc) = user_code() else { return broken("no randomness") };
        match l.registry.put_device_login(hash(&device_code).as_bytes(), &uc, &client, &host, now_ms() + TTL_MS) {
            Ok(true) => {
                let uri = format!("{}/login/device", l.public_url);
                return axum::Json(json!({
                    "device_code": device_code,
                    "user_code": uc,
                    "verification_uri": uri,
                    "verification_uri_complete": format!("{uri}?user_code={uc}"),
                    "expires_in": TTL_MS / 1000,
                    "interval": INTERVAL_S,
                }))
                .into_response();
            }
            Ok(false) => continue,
            Err(e) => return broken(e),
        }
    }
    broken("no free user code")
}

#[derive(Deserialize)]
pub(super) struct TokenRequest {
    grant_type: Option<String>,
    device_code: Option<String>,
}

/// `POST /login/device/token`: the CLI asking whether it was approved.
pub(super) async fn token(
    State(l): State<Arc<Login>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(f): Form<TokenRequest>,
) -> Response {
    if f.grant_type.as_deref() != Some(GRANT_TYPE) {
        return oauth_error(StatusCode::BAD_REQUEST, "unsupported_grant_type");
    }
    // Per address, against guessing device codes (each also has its own interval below).
    if !l.buckets.take(format!("poll:{}", client_ip(peer, &headers)), 60.0, Duration::from_secs(60)) {
        return oauth_error(StatusCode::BAD_REQUEST, "slow_down");
    }
    let code = f.device_code.unwrap_or_default();
    // A little under the interval, so a client keeping to it isn't told off for network jitter.
    let poll = match l.registry.poll_device_login(hash(&code).as_bytes(), INTERVAL_S * 1000 - 1000) {
        Ok(p) => p,
        Err(e) => return broken(e),
    };
    let (login, host) = match poll {
        Poll::Unknown => return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant"),
        Poll::Expired => return oauth_error(StatusCode::BAD_REQUEST, "expired_token"),
        Poll::Denied => return oauth_error(StatusCode::BAD_REQUEST, "access_denied"),
        Poll::Pending => return oauth_error(StatusCode::BAD_REQUEST, "authorization_pending"),
        Poll::SlowDown => return oauth_error(StatusCode::BAD_REQUEST, "slow_down"),
        Poll::Approved { login, host } => (login, host),
    };
    let now = now_ms();
    let made = l.registry.create_token(TokenRec {
        id: String::new(),
        account: login.clone(),
        name: format!("cli on {host}"),
        acts_as: login.clone(),
        repositories: vec![],
        max_role: pando_proto::Role::Admin,
        created_at_ms: now,
        expires_at_ms: now + TOKEN_DAYS * 86_400_000,
        last_used_at_ms: 0,
    });
    match made {
        Ok((t, secret)) => {
            tracing::info!(target: crate::telemetry::AUDIT, kind = "sign_in", principal = %login, target = %format!("cli token {}", t.id));
            axum::Json(json!({
                "access_token": secret,
                "token_type": "Bearer",
                "expires_in": TOKEN_DAYS * 86_400,
            }))
            .into_response()
        }
        Err(e) => broken(e),
    }
}

#[derive(Deserialize)]
pub(super) struct PageQuery {
    user_code: Option<String>,
}

/// `GET /login/device`: enter the code (unless the link carries it), then sign in.
pub(super) async fn enter(
    State(l): State<Arc<Login>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<PageQuery>,
) -> Response {
    let ask = |problem: &str| {
        page(
            if problem.is_empty() { StatusCode::OK } else { StatusCode::BAD_REQUEST },
            "Sign in the Pando CLI",
            &format!(
                "<p>Enter the code that <code>pando login</code> shows in your terminal.</p>{problem}\
                 <form method=get action=\"/login/device\"><label>Code\
                 <input class=code name=user_code required autocomplete=off autocapitalize=characters spellcheck=false placeholder=ABCD-EFGH></label>\
                 <button>Continue</button></form>"
            ),
        )
    };
    let Some(typed) = q.user_code.filter(|c| !c.trim().is_empty()) else { return ask("") };
    if !l.buckets.take(format!("code:{}", client_ip(peer, &headers)), 30.0, Duration::from_secs(600)) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "Too many tries. Please wait a few minutes and try again.");
    }
    let unknown =
        "<p role=alert>That code is not valid or has expired. Run <code>pando login</code> again for a new one.</p>";
    let Some(uc) = normal_code(&typed) else { return ask(unknown) };
    match l.registry.device_login(&uc) {
        Ok(Some(_)) => l.sign_in_page(
            "Sign in the Pando CLI",
            &format!("<p>Sign in to approve the code <strong class=code>{uc}</strong> for the Pando CLI.</p>"),
            &format!("{NEXT}{uc}"),
        ),
        Ok(None) => ask(unknown),
        Err(e) => broken(e),
    }
}

/// After signing in: approve or deny the CLI.
pub(super) fn approve_page(l: &Login, login: &str, uc: &str, next: &str) -> Response {
    let (client, host) = match l.registry.device_login(uc) {
        Ok(Some(d)) => d,
        Ok(None) => return fail(StatusCode::BAD_REQUEST, "That code has expired. Run pando login again."),
        Err(e) => return broken(e),
    };
    let client = if client.is_empty() { "a command-line client".to_owned() } else { client };
    let body = format!(
        "<p>Approve CLI sign-in from <strong>{}</strong> on <strong>{}</strong> as <strong>{}</strong>?</p>\
         <p>Only approve if you ran <code>pando login</code> yourself and your terminal shows the code <strong>{uc}</strong>. \
         The CLI gets a token that can do anything you can, for 90 days.</p>\
         <div class=actions><button name=decision value=approve>Approve</button><button class=quiet name=decision value=deny>Deny</button></div>",
        esc(&client),
        esc(&host),
        esc(login),
    );
    l.confirm_page(StatusCode::OK, Pending::SignedIn { login: login.to_owned() }, next, "Approve CLI sign-in", &body)
}

pub(super) fn decide(l: &Login, login: &str, uc: &str, approve: bool) -> Response {
    match l.registry.decide_device_login(uc, approve.then_some(login)) {
        Ok(true) if approve => {
            tracing::info!(target: crate::telemetry::AUDIT, kind = "device_approve", principal = %login, target = uc);
            page(StatusCode::OK, "CLI signed in", "<p>You can close this tab and go back to your terminal.</p>")
        }
        Ok(true) => page(
            StatusCode::OK,
            "CLI sign-in denied",
            "<p>The CLI was not signed in. If you didn't run <code>pando login</code> yourself, nothing else is needed. You can close this tab.</p>",
        ),
        Ok(false) => fail(StatusCode::BAD_REQUEST, "That code has expired. Run pando login again."),
        Err(e) => broken(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_codes() {
        let c = user_code().unwrap();
        assert_eq!(normal_code(&c).as_deref(), Some(c.as_str()));
        assert_eq!(normal_code("bcdf ghjk").as_deref(), Some("BCDF-GHJK"));
        for bad in ["BCDF-GHJ", "ABCD-EFGH", "BCDF-GHJKL", "BCDF-GH1K"] {
            assert!(normal_code(bad).is_none(), "{bad}");
        }
        assert_eq!(code_in("/login/device?user_code=BCDF-GHJK"), Some("BCDF-GHJK"));
        assert_eq!(code_in("/login/device?user_code=bcdf-ghjk"), None);
        assert_eq!(code_in("/"), None);
    }
}
