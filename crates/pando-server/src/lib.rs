//! Pando server library: feature slices plus the composition root that wires ports
//! to adapters. The binary (`main.rs`) is a thin CLI over [`serve`].

pub mod auth;
pub mod backup;
pub mod config;
pub mod directory;
pub mod features;
pub mod hosting;
pub mod land_hook;
pub mod license;
pub mod login;
pub mod oidc;
pub mod permissions;
pub mod plans;
mod status;
pub mod telemetry;
pub mod webhooks;

use std::future::Future;
use std::sync::Arc;

use pando_core::Repo;
use pando_core::acl::AccessPolicy;
use pando_core::flags::FeatureFlags;
use pando_core::ports::{LandCheck, ObjectStore};
use tokio::net::TcpListener;
use tonic::codegen::http::{HeaderName, HeaderValue, Method};
use tonic::service::{InterceptorLayer, Routes};
use tonic::transport::{Server, ServerTlsConfig};
use tower_http::cors::CorsLayer;

use crate::auth::{AuthInterceptor, Authenticator};
use crate::features::*;

/// All slices, served by the authoritative server.
pub fn authority_routes(
    repo: Repo,
    flags: Arc<dyn FeatureFlags>,
    policy: Arc<dyn AccessPolicy>,
    checks: Arc<dyn LandCheck>,
) -> Routes {
    let f = || flags.clone();
    Routes::new(objects::service(objects::Objects::new(repo.objects.clone(), f(), policy.clone())))
        .add_service(access::service(access::Access::new(f(), policy.clone())))
        .add_service(changesets::service(changesets::Changesets::new(repo.clone(), f(), policy.clone())))
        .add_service(refs::service(refs::Refs::new(repo.clone(), f(), policy.clone())))
        .add_service(land::service(land::Land::new(repo.clone(), f(), policy.clone(), checks)))
        .add_service(locks::service(locks::Locks::new(repo.clone(), f(), policy.clone())))
        .add_service(history::service(history::History::new(repo.clone(), f(), policy.clone())))
        .add_service(diff::service(diff::Diff::new(repo.clone(), f(), policy.clone())))
        .add_service(merge_base::service(merge_base::MergeBase::new(repo.clone(), f())))
        .add_service(browse::service(browse::Browse::new(repo, f(), policy)))
}

/// Objects slice only, served by caches and daemons over a tiered store. Tiers serve
/// objects by id and know no access policy: expose them only to principals with full read
/// access (e.g. build machines), never to path-restricted users.
pub fn tier_routes(store: Arc<dyn ObjectStore>, flags: Arc<dyn FeatureFlags>) -> Routes {
    Routes::new(objects::service(objects::Objects::new(store, flags, Arc::new(pando_core::acl::AllowAllPolicy))))
}

/// Browser access: gRPC-Web (for the web console and other browser clients) and the
/// origins allowed to call the server from another site.
#[derive(Debug, Clone, Default)]
pub struct WebAccess {
    /// Accept gRPC-Web requests (the `web` feature flag).
    pub enabled: bool,
    /// Origins allowed cross-origin, e.g. `https://console.example.com`; `*` allows any (only
    /// for development). Empty: same-origin only.
    pub origins: Vec<String>,
}

impl WebAccess {
    fn cors(&self) -> anyhow::Result<CorsLayer> {
        use tower_http::cors::{AllowOrigin, ExposeHeaders};
        let origin = if self.origins.iter().any(|o| o == "*") {
            AllowOrigin::any()
        } else {
            let list = self.origins.iter().map(|o| o.parse()).collect::<Result<Vec<HeaderValue>, _>>()?;
            AllowOrigin::list(list)
        };
        Ok(CorsLayer::new()
            .allow_origin(origin)
            .allow_methods([Method::POST, Method::OPTIONS])
            .allow_headers([
                HeaderName::from_static("authorization"),
                HeaderName::from_static("content-type"),
                HeaderName::from_static("x-grpc-web"),
                HeaderName::from_static("x-user-agent"),
                HeaderName::from_static("grpc-timeout"),
                HeaderName::from_static("connect-protocol-version"),
                // Names the repository a call is about (multi-repository servers).
                HeaderName::from_static("x-pando-repo"),
            ])
            .expose_headers(ExposeHeaders::list([
                HeaderName::from_static("grpc-status"),
                HeaderName::from_static("grpc-message"),
                HeaderName::from_static("grpc-status-details-bin"),
            ]))
            .max_age(std::time::Duration::from_secs(600)))
    }
}

/// Largest request message the server decodes (objects, changesets): `--max-request-mb`.
static MAX_REQUEST_BYTES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(pando_proto::MAX_MESSAGE_BYTES);

/// Sets the largest request message, at startup (before any service is built).
pub fn set_max_request_bytes(n: usize) {
    MAX_REQUEST_BYTES.store(n.clamp(1 << 20, pando_proto::MAX_MESSAGE_BYTES), std::sync::atomic::Ordering::Relaxed);
}

pub fn max_request_bytes() -> usize {
    MAX_REQUEST_BYTES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Protection against slow or flooding clients.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Longest any one call may run (large checkouts stream for a while; the default is generous).
    pub request_timeout: std::time::Duration,
    /// Calls in progress across all connections; beyond this the server answers Unavailable at
    /// once instead of queueing, so clients back off and retry.
    pub max_in_flight: usize,
    /// Concurrent calls on one connection.
    pub per_connection: usize,
    /// Concurrent calls by one principal across connections (0: no limit); beyond it calls
    /// are refused with ResourceExhausted, so one account can't take the whole server.
    pub per_principal: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            request_timeout: std::time::Duration::from_secs(600),
            max_in_flight: 4096,
            per_connection: 256,
            per_principal: 0,
        }
    }
}

/// How a server is exposed: TLS, browser access, limits, and the repositories it hosts.
#[derive(Default)]
pub struct ServeOptions {
    pub tls: Option<ServerTlsConfig>,
    pub web: WebAccess,
    pub limits: Limits,
    /// Many repositories (`--platform`); `None` serves the default repository only.
    pub hosting: Option<Arc<hosting::Hosting>>,
    /// Readiness, shared with the `/readyz` endpoint.
    pub health: Arc<telemetry::Health>,
    /// Local directories that must stay writable for the server to be ready (data volumes).
    pub ready_dirs: Vec<std::path::PathBuf>,
    /// Take client addresses from `X-Forwarded-For` sent by a proxy on this host.
    pub trust_proxy: bool,
}

/// Whether the registry answers and every data directory takes a synced write.
fn ready_now(registry: Option<&hosting::Registry>, dirs: &[std::path::PathBuf]) -> anyhow::Result<()> {
    use std::io::Write;
    if let Some(r) = registry {
        r.ready()?;
    }
    for dir in dirs {
        std::fs::create_dir_all(dir)?;
        let probe = dir.join(".pando-ready");
        let mut f = std::fs::File::create(&probe)?;
        f.write_all(b"ok")?;
        f.sync_all()?;
        std::fs::remove_file(&probe)?;
    }
    Ok(())
}

/// Refuses a principal's calls beyond `max` at once; each call holds its slot until its
/// response has been sent.
#[derive(Clone)]
struct PerPrincipal {
    max: usize,
    slots: Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Semaphore>>>>,
}

impl PerPrincipal {
    fn acquire(&self, who: &str) -> Option<tokio::sync::OwnedSemaphorePermit> {
        let mut slots = self.slots.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // ponytail: forget idle principals only when the map is large; a timed sweep if it matters.
        if slots.len() > 10_000 {
            slots.retain(|_, s| s.available_permits() < self.max);
        }
        let s = slots.entry(who.to_owned()).or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(self.max))).clone();
        s.try_acquire_owned().ok()
    }
}

impl<S> tower::Layer<S> for PerPrincipal {
    type Service = PerPrincipalService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        PerPrincipalService { inner, limit: self.clone() }
    }
}

#[derive(Clone)]
struct PerPrincipalService<S> {
    inner: S,
    limit: PerPrincipal,
}

impl<S, B> tower::Service<tonic::codegen::http::Request<B>> for PerPrincipalService<S>
where
    S: tower::Service<tonic::codegen::http::Request<B>, Response = tonic::codegen::http::Response<tonic::body::Body>>,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = tonic::codegen::BoxFuture<S::Response, S::Error>;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: tonic::codegen::http::Request<B>) -> Self::Future {
        let who = req.extensions().get::<pando_core::acl::Principal>().map(|p| p.0.clone()).unwrap_or_default();
        let permit = match self.limit.max {
            0 => None,
            _ => match self.limit.acquire(&who) {
                Some(p) => Some(p),
                None => {
                    return Box::pin(async move {
                        let msg = format!("`{who}` has too many calls in progress; retry shortly");
                        Ok(tonic::Status::resource_exhausted(msg).into_http())
                    });
                }
            },
        };
        let fut = self.inner.call(req);
        Box::pin(async move {
            let resp = fut.await?;
            Ok(resp.map(|body| {
                use http_body_util::BodyExt;
                tonic::body::Body::new(body.map_frame(move |f| {
                    let _ = &permit;
                    f
                }))
            }))
        })
    }
}

/// Serve `routes` with auth, health checks, reflection, metrics and graceful shutdown.
pub async fn serve(
    listener: TcpListener,
    routes: Routes,
    auth: Arc<dyn Authenticator>,
    options: ServeOptions,
    shutdown: impl Future<Output = ()> + Send,
) -> anyhow::Result<()> {
    let ServeOptions { tls, web, limits, hosting, health: ready, ready_dirs, trust_proxy } = options;
    let (health_reporter, health) = tonic_health::server::health_reporter();
    // Readiness: checked now and every few seconds; gRPC health follows it (M1).
    let registry = hosting.as_ref().map(|h| h.registry.clone());
    let check = {
        let (ready, reporter) = (ready.clone(), health_reporter.clone());
        move || {
            let (ready, reporter, registry, dirs) =
                (ready.clone(), reporter.clone(), registry.clone(), ready_dirs.clone());
            async move {
                let ok = match tokio::task::spawn_blocking(move || ready_now(registry.as_deref(), &dirs)).await {
                    Ok(Ok(())) => true,
                    Ok(Err(e)) => {
                        tracing::error!(error = %format!("{e:#}"), "not ready");
                        false
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "readiness check failed");
                        false
                    }
                };
                ready.set_ready(ok);
                let status = if ready.ready() {
                    tonic_health::ServingStatus::Serving
                } else {
                    tonic_health::ServingStatus::NotServing
                };
                reporter.set_service_status("", status).await;
            }
        }
    };
    check().await;
    let checker = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            check().await;
        }
    });
    let reflection = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(pando_proto::FILE_DESCRIPTOR_SET)
        .build_v1()?;
    let mut routes = routes.add_service(health).add_service(reflection);
    // Platform tokens first, then the server's own authentication; everyone gets an account.
    let auth: Arc<dyn Authenticator> = match &hosting {
        Some(h) => Arc::new(permissions::PlatformAuth::new(h.registry.clone(), auth)),
        None => auth,
    };
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let mut worker = None;
    if let Some(h) = &hosting {
        worker = Some(tokio::spawn(webhooks::run(h.registry.clone(), h.access.webhook_allow_private, stop_rx)));
        if h.access.multi_tenant {
            tokio::spawn(plans::run(h.clone()));
        }
        routes = routes
            .add_service(features::platform::service(features::platform::Platform::new(h.clone())))
            .add_service(features::reviews::service(features::reviews::Reviews::new(h.clone())))
            .add_service(features::issues::service(features::issues::Issues::new(h.clone())));
    }

    let mut builder = Server::builder()
        .timeout(limits.request_timeout)
        .concurrency_limit_per_connection(limits.per_connection)
        .load_shed(true)
        .accept_http1(web.enabled)
        .tcp_nodelay(true)
        .http2_adaptive_window(Some(true))
        .http2_keepalive_interval(Some(std::time::Duration::from_secs(30)));
    if let Some(tls) = tls {
        builder = builder.tls_config(tls)?;
    }
    // Shutdown: stop being ready at once (load balancers stop sending), finish the calls in
    // progress (lands included), then stop the webhook worker.
    let shutdown = {
        let (ready, reporter) = (ready.clone(), health_reporter.clone());
        async move {
            shutdown.await;
            ready.stop();
            checker.abort();
            reporter.set_service_status("", tonic_health::ServingStatus::NotServing).await;
            tracing::info!("draining calls in progress");
        }
    };
    let per_principal = PerPrincipal { max: limits.per_principal, slots: Default::default() };
    // CORS sits outside authentication: browsers send preflight requests without a token.
    let served = builder
        .layer(tower::util::option_layer(web.enabled.then(|| web.cors()).transpose()?))
        .layer(tower::limit::GlobalConcurrencyLimitLayer::new(limits.max_in_flight))
        // Translates only gRPC-Web requests; plain gRPC passes through untouched. Browsers
        // reach it over HTTP/1.1, which is accepted only when the `web` flag is on.
        .layer(tonic_web::GrpcWebLayer::new())
        .layer(telemetry::RequestInfoLayer { trust_proxy })
        .layer(telemetry::MeteredLayer)
        .layer(InterceptorLayer::new(AuthInterceptor(auth)))
        .layer(telemetry::AuditLayer)
        .layer(per_principal)
        // Innermost: after authentication, so the principal is known before any repository opens.
        .layer(hosting::HostingLayer(hosting))
        .add_routes(routes)
        .serve_with_incoming_shutdown(tokio_stream::wrappers::TcpListenerStream::new(listener), shutdown)
        .await;
    let _ = stop_tx.send(true);
    if let Some(w) = worker {
        let _ = w.await;
    }
    tracing::info!("stopped");
    served?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_principal_cannot_take_every_slot() {
        let limit = PerPrincipal { max: 2, slots: Default::default() };
        let (a, b) = (limit.acquire("agent/a").unwrap(), limit.acquire("agent/a").unwrap());
        assert!(limit.acquire("agent/a").is_none(), "a third call at once is refused");
        assert!(limit.acquire("mira").is_some(), "others are unaffected");
        drop((a, b));
        assert!(limit.acquire("agent/a").is_some(), "slots come back when calls finish");
    }

    #[test]
    fn readiness_needs_writable_data_directories() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ready_now(None, &[dir.path().join("repos")]).is_ok());
        let file = dir.path().join("file");
        std::fs::write(&file, "").unwrap();
        assert!(ready_now(None, &[file.join("below-a-file")]).is_err());
        let reg = hosting::Registry::open("memory").unwrap();
        assert!(ready_now(Some(&reg), &[]).is_ok());
    }
}
