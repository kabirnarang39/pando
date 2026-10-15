//! Sapwood server library: feature slices plus the composition root that wires ports
//! to adapters. The binary (`main.rs`) is a thin CLI over [`serve`].

pub mod auth;
pub mod config;
pub mod features;
pub mod land_hook;
pub mod oidc;
mod status;
pub mod telemetry;

use std::future::Future;
use std::sync::Arc;

use sapwood_core::Repo;
use sapwood_core::acl::AccessPolicy;
use sapwood_core::flags::FeatureFlags;
use sapwood_core::ports::{LandCheck, ObjectStore};
use tokio::net::TcpListener;
use tonic::service::{InterceptorLayer, Routes};
use tonic::transport::{Server, ServerTlsConfig};

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
        .add_service(history::service(history::History::new(repo.clone(), f(), policy.clone())))
        .add_service(diff::service(diff::Diff::new(repo.clone(), f(), policy.clone())))
        .add_service(merge_base::service(merge_base::MergeBase::new(repo.clone(), f())))
        .add_service(browse::service(browse::Browse::new(repo, f(), policy)))
}

/// Objects slice only, served by caches and daemons over a tiered store. Tiers serve
/// objects by id and know no access policy: expose them only to principals with full read
/// access (e.g. build machines), never to path-restricted users.
pub fn tier_routes(store: Arc<dyn ObjectStore>, flags: Arc<dyn FeatureFlags>) -> Routes {
    Routes::new(objects::service(objects::Objects::new(store, flags, Arc::new(sapwood_core::acl::AllowAllPolicy))))
}

/// Serve `routes` with auth, health checks, reflection, metrics and graceful shutdown.
pub async fn serve(
    listener: TcpListener,
    routes: Routes,
    auth: Arc<dyn Authenticator>,
    tls: Option<ServerTlsConfig>,
    shutdown: impl Future<Output = ()> + Send,
) -> anyhow::Result<()> {
    let (health_reporter, health) = tonic_health::server::health_reporter();
    health_reporter.set_service_status("", tonic_health::ServingStatus::Serving).await;
    let reflection = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(sapwood_proto::FILE_DESCRIPTOR_SET)
        .build_v1()?;
    let routes = routes.add_service(health).add_service(reflection);

    let mut builder = Server::builder()
        .tcp_nodelay(true)
        .http2_adaptive_window(Some(true))
        .http2_keepalive_interval(Some(std::time::Duration::from_secs(30)));
    if let Some(tls) = tls {
        builder = builder.tls_config(tls)?;
    }
    builder
        .layer(telemetry::MeteredLayer)
        .layer(InterceptorLayer::new(AuthInterceptor(auth)))
        .add_routes(routes)
        .serve_with_incoming_shutdown(tokio_stream::wrappers::TcpListenerStream::new(listener), shutdown)
        .await?;
    Ok(())
}
