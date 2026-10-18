//! Logs (tracing, optionally JSON) and Prometheus metrics.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::task::{Context, Poll};
use std::time::Instant;

use futures::future::BoxFuture;
use tonic::codegen::http;

/// Target of audit events: who did what, to which path or branch, with what result. They
/// go to `--audit-log` as JSON lines and never into the normal log.
pub const AUDIT: &str = "pando::audit";

/// Logs, and the audit trail to `audit_log`: one file per UTC day (`PATH.YYYY-MM-DD`), the
/// oldest deleted beyond `retention_days` (0 keeps every file). Timestamps are UTC (RFC 3339).
pub fn init_logging(json: bool, audit_log: Option<&std::path::Path>, retention_days: usize) -> anyhow::Result<()> {
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info".into())
        .add_directive(format!("{AUDIT}=off").parse()?);
    let log = if json {
        tracing_subscriber::fmt::layer().json().with_filter(filter).boxed()
    } else {
        tracing_subscriber::fmt::layer().with_filter(filter).boxed()
    };
    let audit = match audit_log {
        Some(path) => {
            let mut files = tracing_appender::rolling::Builder::new()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix(path.file_name().and_then(|f| f.to_str()).unwrap_or("audit.log"));
            if retention_days > 0 {
                files = files.max_log_files(retention_days);
            }
            let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
            let file = files.build(dir)?;
            // ponytail: one mutex-guarded append per event; a buffered writer thread if audit
            // volume ever shows up in profiles.
            let layer = tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(true)
                .with_current_span(false)
                .with_span_list(false)
                .with_target(false)
                .with_writer(std::sync::Mutex::new(file))
                .with_filter(tracing_subscriber::filter::Targets::new().with_target(AUDIT, tracing::Level::INFO));
            Some(layer)
        }
        None => None,
    };
    tracing_subscriber::registry().with(log).with(audit).init();
    Ok(())
}

/// Whether the server can take traffic: [`crate::serve`] checks its storage every few seconds
/// and stops being ready when shutdown starts.
#[derive(Default)]
pub struct Health {
    ready: AtomicBool,
    stopping: AtomicBool,
}

impl Health {
    pub fn ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed) && !self.stopping.load(Ordering::Relaxed)
    }
    pub(crate) fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::Relaxed);
    }
    /// Shutdown has started: load balancers should stop sending new calls.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Relaxed);
    }
}

/// Serve `/metrics` (Prometheus), `/healthz` (liveness: the process answers) and `/readyz`
/// (readiness: 503 while storage or the registry fails, or once shutdown starts) on `addr`,
/// without authentication, for load balancers and uptime checks.
pub async fn init_metrics(addr: SocketAddr, health: Arc<Health>) -> anyhow::Result<()> {
    use axum::http::StatusCode;
    use axum::routing::get;
    let handle = metrics_exporter_prometheus::PrometheusBuilder::new().install_recorder()?;
    let upkeep = handle.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            upkeep.run_upkeep();
        }
    });
    let app =
        axum::Router::new()
            .route("/metrics", get(move || std::future::ready(handle.render())))
            .route("/healthz", get(|| async { "ok" }))
            .route(
                "/readyz",
                get(move || {
                    let ready = health.ready();
                    async move {
                        if ready { (StatusCode::OK, "ready") } else { (StatusCode::SERVICE_UNAVAILABLE, "not ready") }
                    }
                }),
            );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "metrics and health endpoint stopped");
        }
    });
    Ok(())
}

/// The caller's address, as [`RequestInfoLayer`] worked it out.
#[derive(Debug, Clone, Copy)]
pub struct ClientAddr(pub IpAddr);

/// Marks a gRPC health check, which needs no credentials.
#[derive(Debug, Clone, Copy)]
pub struct HealthCheck;

/// The client address of a call: the TCP peer, or with `trust_proxy` and a loopback peer (a
/// reverse proxy on the same host), the last address in `X-Forwarded-For`, the one that proxy
/// saw. Any other peer's header is ignored, so clients can't pick their address (L4).
pub fn client_addr(peer: Option<IpAddr>, forwarded: Option<&str>, trust_proxy: bool) -> Option<IpAddr> {
    let from_proxy = trust_proxy && peer.is_some_and(|p| p.is_loopback());
    match forwarded.and_then(|f| f.rsplit(',').next()).and_then(|a| a.trim().parse().ok()) {
        Some(a) if from_proxy => Some(a),
        _ => peer,
    }
}

/// Records [`ClientAddr`] and [`HealthCheck`] on each request before authentication.
#[derive(Clone, Copy)]
pub struct RequestInfoLayer {
    pub trust_proxy: bool,
}

impl<S> tower::Layer<S> for RequestInfoLayer {
    type Service = RequestInfo<S>;
    fn layer(&self, inner: S) -> RequestInfo<S> {
        RequestInfo(inner, self.trust_proxy)
    }
}

#[derive(Clone)]
pub struct RequestInfo<S>(S, bool);

impl<S, B> tower::Service<http::Request<B>> for RequestInfo<S>
where
    S: tower::Service<http::Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, mut req: http::Request<B>) -> Self::Future {
        let peer = req
            .extensions()
            .get::<tonic::transport::server::TcpConnectInfo>()
            .and_then(|c| c.remote_addr())
            .map(|a| a.ip());
        let forwarded = req.headers().get("x-forwarded-for").and_then(|v| v.to_str().ok());
        if let Some(a) = client_addr(peer, forwarded, self.1) {
            req.extensions_mut().insert(ClientAddr(a));
        }
        if req.uri().path().starts_with("/grpc.health.v1.Health/") {
            req.extensions_mut().insert(HealthCheck);
        }
        self.0.call(req)
    }
}

/// Tower layer writing one audit event per request: who (as authenticated), from where,
/// which method, the gRPC status and the duration. Sits inside authentication.
#[derive(Clone, Copy)]
pub struct AuditLayer;

impl<S> tower::Layer<S> for AuditLayer {
    type Service = Audited<S>;
    fn layer(&self, inner: S) -> Audited<S> {
        Audited(inner)
    }
}

#[derive(Clone)]
pub struct Audited<S>(S);

impl<S, B, RB> tower::Service<http::Request<B>> for Audited<S>
where
    S: tower::Service<http::Request<B>, Response = http::Response<RB>>,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<S::Response, S::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        if !tracing::enabled!(target: AUDIT, tracing::Level::INFO) {
            return Box::pin(self.0.call(req));
        }
        let method = req.uri().path().to_owned();
        let principal = req.extensions().get::<pando_core::acl::Principal>().map(|p| p.0.clone()).unwrap_or_default();
        let peer = req.extensions().get::<ClientAddr>().map(|a| a.0.to_string()).unwrap_or_default();
        let start = Instant::now();
        let fut = self.0.call(req);
        Box::pin(async move {
            let out = fut.await;
            // Errors returned before any message carry their status in the headers; a
            // success (or a failure later in a stream) carries it in the trailers.
            let status = match &out {
                Ok(resp) => resp.headers().get("grpc-status").and_then(|v| v.to_str().ok()).unwrap_or("0").to_owned(),
                Err(_) => "transport".to_owned(),
            };
            tracing::info!(target: AUDIT, kind = "rpc", principal, peer, method, status, ms = start.elapsed().as_millis() as u64);
            out
        })
    }
}

/// Every method the protocol defines (`/pando.v1.Service/Method`, from the descriptor set) plus
/// gRPC health checks: the only paths that become metric labels.
static KNOWN_METHODS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    let set: prost_types::FileDescriptorSet =
        prost::Message::decode(pando_proto::FILE_DESCRIPTOR_SET).expect("embedded descriptor set");
    let mut known: HashSet<String> = set
        .file
        .iter()
        .flat_map(|f| {
            let package = f.package().to_owned();
            f.service.iter().flat_map(move |s| {
                let service = format!("{package}.{}", s.name());
                s.method.iter().map(move |m| format!("/{service}/{}", m.name()))
            })
        })
        .collect();
    known.extend(["/grpc.health.v1.Health/Check".to_owned(), "/grpc.health.v1.Health/Watch".to_owned()]);
    known
});

/// The metric label for a request path: the method if the protocol defines it, else "unknown",
/// so made-up paths can't create unbounded series (M2).
pub fn method_label(path: &str) -> String {
    if KNOWN_METHODS.contains(path) { path.to_owned() } else { "unknown".to_owned() }
}

/// Tower layer recording per-RPC count and latency.
#[derive(Clone, Copy)]
pub struct MeteredLayer;

impl<S> tower::Layer<S> for MeteredLayer {
    type Service = Metered<S>;
    fn layer(&self, inner: S) -> Metered<S> {
        Metered(inner)
    }
}

#[derive(Clone)]
pub struct Metered<S>(S);

impl<S, B> tower::Service<http::Request<B>> for Metered<S>
where
    S: tower::Service<http::Request<B>>,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<S::Response, S::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        let method = method_label(req.uri().path());
        let start = Instant::now();
        let fut = self.0.call(req);
        Box::pin(async move {
            let out = fut.await;
            metrics::histogram!("pando_rpc_seconds", "method" => method.clone()).record(start.elapsed().as_secs_f64());
            metrics::counter!("pando_rpc_total", "method" => method).increment(1);
            out
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_defined_methods_become_labels() {
        assert_eq!(method_label("/pando.v1.Land/Land"), "/pando.v1.Land/Land");
        assert_eq!(method_label("/grpc.health.v1.Health/Check"), "/grpc.health.v1.Health/Check");
        for junk in ["/pando.v1.Land/x1234", "/pando.v1.Nope/Get", "/grpc.anything", "/", "/metrics"] {
            assert_eq!(method_label(junk), "unknown", "{junk}");
        }
    }

    #[test]
    fn forwarded_addresses_are_trusted_only_from_a_local_proxy() {
        let (local, remote) = ("127.0.0.1".parse().ok(), "203.0.113.9".parse().ok());
        let xff = Some("198.51.100.1, 192.0.2.7");
        assert_eq!(client_addr(local, xff, true), "192.0.2.7".parse().ok());
        assert_eq!(client_addr(local, xff, false), local, "off unless --trust-proxy");
        assert_eq!(client_addr(remote, xff, true), remote, "a remote client can't claim an address");
        assert_eq!(client_addr(local, Some("not an ip"), true), local);
    }
}
