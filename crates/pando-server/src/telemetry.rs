//! Logs (tracing, optionally JSON) and Prometheus metrics.

use std::net::SocketAddr;
use std::task::{Context, Poll};
use std::time::Instant;

use futures::future::BoxFuture;
use tonic::codegen::http;

/// Target of audit events: who did what, to which path or branch, with what result. They
/// go to `--audit-log` as JSON lines and never into the normal log.
pub const AUDIT: &str = "pando::audit";

pub fn init_logging(json: bool, audit_log: Option<&std::path::Path>) -> anyhow::Result<()> {
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
            let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
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

/// Expose `/metrics` for Prometheus on `addr`.
pub fn init_metrics(addr: SocketAddr) -> anyhow::Result<()> {
    metrics_exporter_prometheus::PrometheusBuilder::new().with_http_listener(addr).install()?;
    Ok(())
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
        let peer = req
            .extensions()
            .get::<tonic::transport::server::TcpConnectInfo>()
            .and_then(|c| c.remote_addr())
            .map(|a| a.ip().to_string())
            .unwrap_or_default();
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
        // Bound label cardinality: arbitrary client paths collapse into "other".
        let path = req.uri().path();
        let method = if path.starts_with("/pando.v1.") || path.starts_with("/grpc.") {
            path.to_owned()
        } else {
            "other".to_owned()
        };
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
