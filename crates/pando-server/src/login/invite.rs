//! Invitations to an organisation (ADR-033): a single-use link, valid 7 days, stored as a hash.
//! Whoever opens it signs in (any provider, creating an account if need be) and joins with the
//! invited role. The link is the authority: the address it was sent to is shown, not enforced.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use lettre::AsyncTransport as _;
use pando_proto::{Invitation, Role};

use super::store::invited_role;
use super::{Login, Pending, broken, client_ip, esc, fail, hash, random_hex};
use crate::hosting::now_ms;
use crate::permissions::role_name;

const TTL_MS: i64 = 7 * 86_400_000;
const NEXT: &str = "/login/invite/";

/// The invitation token a sign-in is for, if it goes on to accepting one.
pub(super) fn token_in(next: &str) -> Option<&str> {
    next.strip_prefix(NEXT).filter(|t| t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn gone() -> Response {
    fail(StatusCode::BAD_REQUEST, "This invitation has expired, was already used, or was withdrawn. Ask for a new one.")
}

/// Records an invitation (email already normalised) and emails its link when the server sends
/// mail. Returns it with the link, shown once, and whether it was emailed.
pub(crate) async fn create_invitation(
    l: &Login,
    org: &str,
    email: &str,
    role: Role,
    invited_by: &str,
) -> anyhow::Result<(Invitation, String, bool)> {
    let token = random_hex()?;
    let mut id = [0u8; 8];
    getrandom::fill(&mut id)?;
    let now = now_ms();
    let inv = Invitation {
        id: hex::encode(id),
        org: org.to_owned(),
        email: email.to_owned(),
        role: role as i32,
        invited_by: invited_by.to_owned(),
        created_at_ms: now,
        expires_at_ms: now + TTL_MS,
    };
    l.registry.create_invitation(&inv, hash(&token).as_bytes())?;
    let link = format!("{}{NEXT}{token}", l.public_url);
    let emailed = match send(l, &inv, &link).await {
        Ok(sent) => sent,
        Err(e) => {
            tracing::warn!(error = %e, org, "sending an invitation");
            false
        }
    };
    Ok((inv, link, emailed))
}

async fn send(l: &Login, inv: &Invitation, link: &str) -> anyhow::Result<bool> {
    let Some(m) = &l.mailer else { return Ok(false) };
    let msg = lettre::Message::builder()
        .from(m.from.clone())
        .to(inv.email.parse()?)
        .subject(format!("{} invited you to {} on Grovedock", inv.invited_by, inv.org))
        .header(lettre::message::header::ContentType::TEXT_PLAIN)
        .body(format!(
            "{} invited you to join the organisation {} on {} as {}.\n\nAccept the invitation:\n\n{link}\n\n\
             It works once, within 7 days. If you didn't expect it, ignore this email.\n",
            inv.invited_by,
            inv.org,
            l.public_url,
            role_name(invited_role(inv.role)),
        ))?;
    m.transport.send(msg).await?;
    Ok(true)
}

/// `GET /login/invite/TOKEN`: what the invitation is for, and the ways to sign in to accept it.
pub(super) async fn page(
    State(l): State<Arc<Login>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    if !l.buckets.take(format!("invite:{}", client_ip(peer, &headers)), 30.0, Duration::from_secs(600)) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "Too many tries. Please wait a few minutes and try again.");
    }
    let next = format!("{NEXT}{token}");
    if token_in(&next).is_none() {
        return gone();
    }
    let inv = match l.registry.invitation(hash(&token).as_bytes()) {
        Ok(Some(i)) => i,
        Ok(None) => return gone(),
        Err(e) => return broken(e),
    };
    l.sign_in_page(
        &format!("Join {}", inv.org),
        &format!(
            "<p>{} invited you to join <strong>{}</strong> as {}. The invitation was sent to {}.</p>\
             <p>Sign in, or create an account, to accept it.</p>",
            esc(&inv.invited_by),
            esc(&inv.org),
            role_name(invited_role(inv.role)),
            esc(&inv.email),
        ),
        &next,
    )
}

/// After signing in: the button that joins.
pub(super) fn join_page(l: &Login, login: &str, token: &str, next: &str) -> Response {
    let inv = match l.registry.invitation(hash(token).as_bytes()) {
        Ok(Some(i)) => i,
        Ok(None) => return gone(),
        Err(e) => return broken(e),
    };
    let body = format!(
        "<p>Join <strong>{org}</strong> as {role}?</p><p>You are signed in as <strong>{login}</strong>. \
         {by} sent this invitation to {email}.</p><button>Join {org}</button>",
        org = esc(&inv.org),
        role = role_name(invited_role(inv.role)),
        login = esc(login),
        by = esc(&inv.invited_by),
        email = esc(&inv.email),
    );
    l.confirm_page(
        StatusCode::OK,
        Pending::SignedIn { login: login.to_owned() },
        next,
        &format!("Join {}", inv.org),
        &body,
    )
}

/// Joins `login` to the invitation's organisation (once), then opens the console.
pub(super) fn accept(l: &Login, login: &str, token: &str) -> Response {
    let key = hash(token);
    let inv = match l.registry.take_invitation(key.as_bytes()) {
        Ok(Some(i)) => i,
        Ok(None) => return gone(),
        Err(e) => return broken(e),
    };
    let refused = |message: &str| {
        if let Err(e) = l.registry.untake_invitation(key.as_bytes()) {
            tracing::error!(error = %e, "putting back an invitation");
        }
        fail(StatusCode::CONFLICT, message)
    };
    match l.registry.memberships(login) {
        Ok(m) if m.iter().any(|(o, _)| *o == inv.org) => {
            return refused(&format!("You are already a member of {}.", inv.org));
        }
        Ok(_) => {}
        Err(e) => return broken(e),
    }
    let role = invited_role(inv.role);
    // The same path as SetMember, so its checks on joining (such as seat limits) apply.
    if let Err(s) = crate::directory::add_member(&l.registry, &inv.org, login, role) {
        return refused(s.message());
    }
    tracing::info!(target: crate::telemetry::AUDIT, kind = "accept_invitation", principal = %login, target = %format!("{}:{}={}", inv.org, inv.id, role_name(role)));
    l.session(login, "invitation", "/")
}
