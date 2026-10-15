use std::net::SocketAddr;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use sapwood_adapters::acl::RulesPolicy;
use sapwood_adapters::flags::SpecFlags;
use sapwood_core::acl::{AccessPolicy, AllowAllPolicy};
use sapwood_core::flags::{FeatureFlags, Flag};
use sapwood_server::auth::{AllowAll, Authenticator, StaticTokens};
use sapwood_server::oidc::{FirstMatch, OidcAuthenticator, OidcConfig};
use sapwood_server::{authority_routes, config, serve, telemetry, tier_routes};
use tonic::transport::{Identity, ServerTlsConfig};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(name = "sapwood", version, about = "Sapwood: source control server, regional cache and node daemon")]
struct Cli {
    #[command(subcommand)]
    role: Role,
    /// Prometheus metrics listen address, e.g. 0.0.0.0:9420.
    #[arg(long, global = true, env = "SAPWOOD_METRICS")]
    metrics: Option<SocketAddr>,
    /// Emit JSON logs.
    #[arg(long, global = true, env = "SAPWOOD_LOG_JSON")]
    log_json: bool,
    /// Comma-separated bearer tokens accepted by this process. Unset = no auth (dev only).
    #[arg(long, global = true, env = "SAPWOOD_TOKENS", hide_env_values = true)]
    tokens: Option<String>,
    /// OpenID Connect issuer URL; enables SSO bearer tokens (JWTs) alongside static tokens.
    #[arg(long, global = true, env = "SAPWOOD_OIDC_ISSUER", requires = "oidc_audience")]
    oidc_issuer: Option<String>,
    /// Expected `aud` claim of OIDC tokens.
    #[arg(long, global = true, env = "SAPWOOD_OIDC_AUDIENCE")]
    oidc_audience: Option<String>,
    /// Claim used as the principal for access rules (sub, email, preferred_username...).
    #[arg(long, global = true, env = "SAPWOOD_OIDC_CLAIM", default_value = "sub")]
    oidc_claim: String,
    /// PEM certificate and key for TLS.
    #[arg(long, global = true, env = "SAPWOOD_TLS_CERT", requires = "tls_key")]
    tls_cert: Option<std::path::PathBuf>,
    #[arg(long, global = true, env = "SAPWOOD_TLS_KEY")]
    tls_key: Option<std::path::PathBuf>,
}

#[derive(Subcommand)]
enum Role {
    /// Authoritative server: all features, durable storage.
    Server {
        #[arg(long, default_value = "0.0.0.0:7420")]
        listen: SocketAddr,
        /// memory | sqlite://PATH | postgres://...
        #[arg(
            long,
            env = "SAPWOOD_METADATA",
            hide_env_values = true,
            default_value = "sqlite://./sapwood-data/meta.db"
        )]
        metadata: String,
        /// metadata | file:///PATH | s3://BUCKET/PREFIX | gs://BUCKET | az://CONTAINER | memory:///
        #[arg(long, env = "SAPWOOD_OBJECTS", hide_env_values = true, default_value = "metadata")]
        objects: String,
        /// JSON access rules (per-directory write and per-ref permissions). Unset = allow all.
        #[arg(long, env = "SAPWOOD_ACL")]
        acl: Option<std::path::PathBuf>,
    },
    /// Delta-compress stored blob versions (single writer; safe to run while serving).
    Compact {
        #[arg(
            long,
            env = "SAPWOOD_METADATA",
            hide_env_values = true,
            default_value = "sqlite://./sapwood-data/meta.db"
        )]
        metadata: String,
        #[arg(long, env = "SAPWOOD_OBJECTS", hide_env_values = true, default_value = "metadata")]
        objects: String,
        /// Take over the lease of a compaction that crashed.
        #[arg(long)]
        force_unlock: bool,
    },
    /// Regional cache: stateless, memory-bounded, read-through to an upstream tier.
    Cache {
        #[arg(long, default_value = "0.0.0.0:7421")]
        listen: SocketAddr,
        #[arg(long, env = "SAPWOOD_UPSTREAM", hide_env_values = true)]
        upstream: String,
        #[arg(long, default_value_t = 4096)]
        capacity_mb: u64,
    },
    /// Per-node daemon: memory + local disk cache shared by every job on the machine.
    Daemon {
        #[arg(long, default_value = "127.0.0.1:7422")]
        listen: SocketAddr,
        #[arg(long, env = "SAPWOOD_UPSTREAM", hide_env_values = true)]
        upstream: String,
        #[arg(long, default_value = "./sapwood-daemon")]
        dir: String,
        #[arg(long, default_value_t = 512)]
        memory_mb: u64,
    },
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! { _ = ctrl_c => {}, _ = term.recv() => {} }
    }
    #[cfg(not(unix))]
    let _ = ctrl_c.await;
    tracing::info!("shutting down");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    telemetry::init_logging(cli.log_json);
    if let Some(addr) = cli.metrics {
        telemetry::init_metrics(addr)?;
    }
    let flags = Arc::new(SpecFlags::from_env()?);
    let mut authenticators: Vec<Arc<dyn Authenticator>> = Vec::new();
    if let Some(t) = &cli.tokens {
        authenticators.push(Arc::new(StaticTokens::new(t.split(',').map(|s| s.trim().to_owned()))));
    }
    if let (Some(issuer), Some(audience)) = (&cli.oidc_issuer, &cli.oidc_audience) {
        if flags.enabled(Flag::Oidc) {
            let cfg = OidcConfig {
                issuer: issuer.clone(),
                audience: audience.clone(),
                principal_claim: cli.oidc_claim.clone(),
            };
            authenticators.push(Arc::new(OidcAuthenticator::discover(cfg).await?));
            tracing::info!(%issuer, "OIDC authentication enabled");
        } else {
            tracing::warn!("--oidc-issuer ignored: feature `oidc` is disabled");
        }
    }
    let auth: Arc<dyn Authenticator> = if authenticators.is_empty() {
        tracing::warn!("no SAPWOOD_TOKENS or OIDC issuer set: authentication disabled");
        Arc::new(AllowAll)
    } else {
        Arc::new(FirstMatch(authenticators))
    };
    let tls = match (&cli.tls_cert, &cli.tls_key) {
        (Some(c), Some(k)) => {
            Some(ServerTlsConfig::new().identity(Identity::from_pem(std::fs::read(c)?, std::fs::read(k)?)))
        }
        _ => None,
    };
    let upstream_token = std::env::var("SAPWOOD_UPSTREAM_TOKEN").ok();

    let (listen, routes) = match cli.role {
        Role::Server { listen, metadata, objects, acl } => {
            let repo = config::open_repo(&metadata, &objects).await?;
            let policy: Arc<dyn AccessPolicy> = match &acl {
                Some(path) => Arc::new(RulesPolicy::load(path)?),
                None => Arc::new(AllowAllPolicy),
            };
            tracing::info!(%listen, metadata = %config::redact(&metadata), objects = %config::redact(&objects), acl = ?acl, "sapwood server");
            (listen, authority_routes(repo, flags, policy))
        }
        Role::Compact { metadata, objects, force_unlock } => {
            let storage = config::open_storage(&metadata, &objects).await?;
            let started = std::time::Instant::now();
            let compactor = sapwood_server::features::compact::Compactor::new(storage.repo, storage.raw_objects, flags);
            let s = compactor.run(force_unlock).await?;
            if let Some(db) = storage.sqlite {
                db.vacuum().await?;
            }
            println!(
                "compacted {} of {} object versions ({} trees) in {:.1}s: {} -> {} bytes ({:.1}x); {} changesets scanned",
                s.converted,
                s.pairs,
                s.trees_converted,
                started.elapsed().as_secs_f64(),
                s.bytes_before,
                s.bytes_after,
                s.bytes_before as f64 / s.bytes_after.max(1) as f64,
                s.changesets
            );
            return Ok(());
        }
        Role::Cache { listen, upstream, capacity_mb } => {
            let store = config::cache_store(&upstream, upstream_token.as_deref(), capacity_mb << 20)?;
            tracing::info!(%listen, upstream = %config::redact(&upstream), capacity_mb, "sapwood cache");
            (listen, tier_routes(store, flags))
        }
        Role::Daemon { listen, upstream, dir, memory_mb } => {
            let store = config::daemon_store(&upstream, upstream_token.as_deref(), &dir, memory_mb << 20)?;
            tracing::info!(%listen, upstream = %config::redact(&upstream), %dir, "sapwood daemon");
            (listen, tier_routes(store, flags))
        }
    };
    let listener = tokio::net::TcpListener::bind(listen).await?;
    serve(listener, routes, auth, tls, shutdown_signal()).await
}
