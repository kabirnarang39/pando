//! Logs (tracing, optionally JSON) and Prometheus metrics.

use std::net::SocketAddr;
use std::task::{Context, Poll};
use std::time::Instant;

use futures::future::BoxFuture;
use tonic::codegen::http;

pub fn init_logging(json: bool) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if json {
        builder.json().init();
    } else {
        builder.init();
    }
}

/// Expose `/metrics` for Prometheus on `addr`.
pub fn init_metrics(addr: SocketAddr) -> anyhow::Result<()> {
    metrics_exporter_prometheus::PrometheusBuilder::new().with_http_listener(addr).install()?;
    Ok(())
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
