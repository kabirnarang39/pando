use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use pando_adapters::acl::RulesPolicy;
use pando_adapters::flags::SpecFlags;
use pando_core::acl::{AccessPolicy, AllowAllPolicy};
use pando_core::flags::{FeatureFlags, Flag};
use pando_core::ports::{LandCheck, NoChecks};
use pando_server::auth::{AllowAll, Authenticator, StaticTokens};
use pando_server::land_hook::WebhookCheck;
use pando_server::oidc::{FirstMatch, OidcAuthenticator, OidcConfig};
use pando_server::{authority_routes, config, serve, telemetry, tier_routes};
use tonic::transport::{Identity, ServerTlsConfig};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(name = "pandod", version, about = "Pando: source control server, regional cache and node daemon")]
struct Cli {
    #[command(subcommand)]
    role: Role,
    /// Prometheus metrics listen address, e.g. 0.0.0.0:9420.
    #[arg(long, global = true, env = "PANDO_METRICS")]
    metrics: Option<SocketAddr>,
    /// Emit JSON logs.
    #[arg(long, global = true, env = "PANDO_LOG_JSON")]
    log_json: bool,
    /// Append an audit trail (JSON lines: who read or changed what, from where, with what
    /// result) to this file. Off by default; it grows with every request.
    #[arg(long, global = true, env = "PANDO_AUDIT_LOG")]
    audit_log: Option<std::path::PathBuf>,
    /// Comma-separated origins allowed to call this server from a browser on another site,
    /// e.g. the web console's address; `*` allows any (development only).
    #[arg(long, global = true, env = "PANDO_WEB_ORIGINS", value_delimiter = ',')]
    web_origins: Vec<String>,
    /// Accept unauthenticated calls on a non-loopback address. Without tokens or OIDC, pandod
    /// otherwise listens only on localhost, so a server is never open to a network by accident.
    #[arg(long, global = true, env = "PANDO_ALLOW_ANONYMOUS", action = clap::ArgAction::SetTrue, value_parser = clap::builder::BoolishValueParser::new())]
    allow_anonymous: bool,
    /// Longest any one call may run, in seconds.
    #[arg(long, global = true, env = "PANDO_REQUEST_TIMEOUT", default_value_t = 600)]
    request_timeout: u64,
    /// Calls in progress at once across all clients; beyond this, calls are refused with
    /// Unavailable (clients retry) instead of queueing.
    #[arg(long, global = true, env = "PANDO_MAX_IN_FLIGHT", default_value_t = 4096)]
    max_in_flight: usize,
    /// Comma-separated bearer tokens accepted by this process. Unset = no auth (dev only).
    #[arg(long, global = true, env = "PANDO_TOKENS", hide_env_values = true)]
    tokens: Option<String>,
    /// OpenID Connect issuer URL; enables SSO bearer tokens (JWTs) alongside static tokens.
    #[arg(long, global = true, env = "PANDO_OIDC_ISSUER", requires = "oidc_audience")]
    oidc_issuer: Option<String>,
    /// Expected `aud` claim of OIDC tokens.
    #[arg(long, global = true, env = "PANDO_OIDC_AUDIENCE")]
    oidc_audience: Option<String>,
    /// Claim used as the principal for access rules (sub, email, preferred_username...).
    #[arg(long, global = true, env = "PANDO_OIDC_CLAIM", default_value = "sub")]
    oidc_claim: String,
    /// PEM certificate and key for TLS.
    #[arg(long, global = true, env = "PANDO_TLS_CERT", requires = "tls_key")]
    tls_cert: Option<std::path::PathBuf>,
    #[arg(long, global = true, env = "PANDO_TLS_KEY")]
    tls_key: Option<std::path::PathBuf>,
}

#[derive(Subcommand)]
enum Role {
    /// Authoritative server: all features, durable storage.
    Server {
        #[arg(long, default_value = "127.0.0.1:7420")]
        listen: SocketAddr,
        /// memory | sqlite://PATH | postgres://...
        #[arg(long, env = "PANDO_METADATA", hide_env_values = true, default_value = "sqlite://./pando-data/meta.db")]
        metadata: String,
        /// metadata | file:///PATH | s3://BUCKET/PREFIX | gs://BUCKET | az://CONTAINER | memory:///
        #[arg(long, env = "PANDO_OBJECTS", hide_env_values = true, default_value = "metadata")]
        objects: String,
        /// JSON access rules (per-directory write and per-ref permissions). Unset = allow all.
        #[arg(long, env = "PANDO_ACL")]
        acl: Option<std::path::PathBuf>,
        /// Pre-land check: every land is POSTed here and proceeds only on a 2xx answer.
        #[arg(long, env = "PANDO_LAND_HOOK", hide_env_values = true)]
        land_hook: Option<String>,
        /// Bearer token sent to the land hook.
        #[arg(long, env = "PANDO_LAND_HOOK_TOKEN", hide_env_values = true, requires = "land_hook")]
        land_hook_token: Option<String>,
        /// Seconds to wait for the land hook before refusing the land.
        #[arg(long, default_value_t = 10)]
        land_hook_timeout: u64,
        /// Host many repositories: the registry of which exist (sqlite://PATH or memory).
        /// Without it the server hosts one repository, `--metadata` / `--objects`.
        #[arg(long, env = "PANDO_PLATFORM", hide_env_values = true)]
        platform: Option<String>,
        /// Storage for each hosted repository; `{repo}` becomes OWNER/NAME and `{repo_id}`
        /// OWNER--NAME, e.g. sqlite:///var/lib/pando/repos/{repo}.db
        #[arg(long, env = "PANDO_REPOS_METADATA", hide_env_values = true, requires = "platform")]
        repos_metadata: Option<String>,
        /// Objects for each hosted repository (same placeholders), or `metadata`.
        #[arg(long, env = "PANDO_REPOS_OBJECTS", hide_env_values = true, default_value = "metadata")]
        repos_objects: String,
        /// Principals that may create repositories.
        #[arg(long, env = "PANDO_PLATFORM_ADMINS", value_delimiter = ',')]
        platform_admins: Vec<String>,
    },
    /// Delta-compress stored blob versions (single writer; safe to run while serving).
    Compact {
        #[arg(long, env = "PANDO_METADATA", hide_env_values = true, default_value = "sqlite://./pando-data/meta.db")]
        metadata: String,
        #[arg(long, env = "PANDO_OBJECTS", hide_env_values = true, default_value = "metadata")]
        objects: String,
        /// Take over the lease of a compaction that crashed.
        #[arg(long)]
        force_unlock: bool,
    },
    /// Copy a repository to another store: a backup, a restore, or a move to another backend
    /// (e.g. SQLite to Postgres). Safe while the source serves; repeat it to copy only what
    /// is new.
    Copy {
        #[arg(long)]
        from_metadata: String,
        #[arg(long, default_value = "metadata")]
        from_objects: String,
        #[arg(long)]
        to_metadata: String,
        #[arg(long, default_value = "metadata")]
        to_objects: String,
    },
    /// Regional cache: stateless, memory-bounded, read-through to an upstream tier.
    Cache {
        #[arg(long, default_value = "127.0.0.1:7421")]
        listen: SocketAddr,
        #[arg(long, env = "PANDO_UPSTREAM", hide_env_values = true)]
        upstream: String,
        #[arg(long, default_value_t = 4096)]
        capacity_mb: u64,
    },
    /// Per-node daemon: memory + local disk cache shared by every job on the machine.
    Daemon {
        #[arg(long, default_value = "127.0.0.1:7422")]
        listen: SocketAddr,
        #[arg(long, env = "PANDO_UPSTREAM", hide_env_values = true)]
        upstream: String,
        #[arg(long, default_value = "./pando-daemon")]
        dir: String,
        #[arg(long, default_value_t = 512)]
        memory_mb: u64,
        /// Authoritative server, used to resolve `--prefetch` revisions.
        #[arg(long, env = "PANDO_SERVER", hide_env_values = true, requires = "prefetch")]
        server: Option<String>,
        /// Warm the cache with this revision (e.g. `main`) at startup.
        #[arg(long, requires = "server")]
        prefetch: Option<String>,
        /// Limit prefetch to these paths (repeatable).
        #[arg(long = "prefetch-path")]
        prefetch_paths: Vec<String>,
        /// Re-prefetch periodically (seconds) to follow a moving branch.
        #[arg(long, requires = "prefetch")]
        prefetch_interval: Option<u64>,
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
    telemetry::init_logging(cli.log_json, cli.audit_log.as_deref())?;
    if let Some(addr) = cli.metrics {
        telemetry::init_metrics(addr)?;
    }
    let flags = Arc::new(SpecFlags::from_env()?);
    let web = pando_server::WebAccess {
        enabled: flags.enabled(pando_core::flags::Flag::Web),
        origins: cli.web_origins.iter().map(|o| o.trim().to_owned()).filter(|o| !o.is_empty()).collect(),
    };
    if web.origins.iter().any(|o| o == "*") {
        tracing::warn!("PANDO_WEB_ORIGINS=*: any website can call this server from a browser; use only in development");
    }
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
    let anonymous = authenticators.is_empty();
    let auth: Arc<dyn Authenticator> = if anonymous {
        tracing::warn!("no PANDO_TOKENS or OIDC issuer set: authentication disabled");
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
    let upstream_token = std::env::var("PANDO_UPSTREAM_TOKEN").ok();

    let mut hosting: Option<Arc<pando_server::hosting::Hosting>> = None;
    let (listen, routes) = match cli.role {
        Role::Server {
            listen,
            metadata,
            objects,
            acl,
            land_hook,
            land_hook_token,
            land_hook_timeout,
            platform,
            repos_metadata,
            repos_objects,
            platform_admins,
        } => {
            let repo = config::open_repo(&metadata, &objects).await?;
            let policy: Arc<dyn AccessPolicy> = match &acl {
                Some(path) => Arc::new(RulesPolicy::load(path)?),
                None => Arc::new(AllowAllPolicy),
            };
            tracing::info!(%listen, metadata = %config::redact(&metadata), objects = %config::redact(&objects), acl = ?acl, "pandod server");
            let checks: Arc<dyn LandCheck> = match &land_hook {
                Some(url) => {
                    tracing::info!(hook = %config::redact(url), "pre-land checks enabled");
                    Arc::new(WebhookCheck::new(url, land_hook_token, Duration::from_secs(land_hook_timeout))?)
                }
                None => Arc::new(NoChecks),
            };
            if let Some(registry) = &platform {
                let templates = pando_server::hosting::Templates {
                    metadata: anyhow::Context::context(
                        repos_metadata.clone(),
                        "--platform needs --repos-metadata (e.g. sqlite:///var/lib/pando/repos/{repo}.db)",
                    )?,
                    objects: repos_objects.clone(),
                };
                let (f, p, c) = (flags.clone(), policy.clone(), checks.clone());
                let build: pando_server::hosting::BuildRoutes =
                    Arc::new(move |repo| authority_routes(repo, f.clone(), p.clone(), c.clone()));
                hosting = Some(Arc::new(pando_server::hosting::Hosting::new(
                    templates,
                    pando_server::hosting::Registry::open(registry)?,
                    build,
                    platform_admins.iter().map(|a| a.trim().to_owned()).filter(|a| !a.is_empty()).collect(),
                )?));
                tracing::info!(registry = %config::redact(registry), "hosting many repositories");
            }
            (listen, authority_routes(repo, flags, policy, checks))
        }
        Role::Copy { from_metadata, from_objects, to_metadata, to_objects } => {
            let from = config::open_storage(&from_metadata, &from_objects).await?;
            let to = config::open_storage(&to_metadata, &to_objects).await?;
            let started = std::time::Instant::now();
            let s = pando_server::features::copy::copy(&from.raw, &to.raw).await?;
            println!(
                "copied {} refs, {} changesets, {} objects ({} already there), {} derived records in {:.1}s",
                s.refs,
                s.changesets,
                s.objects_copied,
                s.objects_present,
                s.derived,
                started.elapsed().as_secs_f64()
            );
            return Ok(());
        }
        Role::Compact { metadata, objects, force_unlock } => {
            let storage = config::open_storage(&metadata, &objects).await?;
            let started = std::time::Instant::now();
            let compactor = pando_server::features::compact::Compactor::new(storage.repo, storage.raw_objects, flags);
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
            tracing::info!(%listen, upstream = %config::redact(&upstream), capacity_mb, "pandod cache");
            (listen, tier_routes(store, flags))
        }
        Role::Daemon { listen, upstream, dir, memory_mb, server, prefetch, prefetch_paths, prefetch_interval } => {
            let store = config::daemon_store(&upstream, upstream_token.as_deref(), &dir, memory_mb << 20)?;
            tracing::info!(%listen, upstream = %config::redact(&upstream), %dir, "pandod daemon");
            if let (Some(server), Some(rev)) = (server, prefetch) {
                let p = pando_server::features::prefetch::Prefetcher::new(
                    &server,
                    upstream_token.as_deref(),
                    store.clone(),
                    flags.clone(),
                )?;
                // Warm in the background: the daemon serves (and fetches on demand) meanwhile.
                tokio::spawn(p.run(rev, prefetch_paths, prefetch_interval.map(std::time::Duration::from_secs)));
            }
            (listen, tier_routes(store, flags))
        }
    };
    if anonymous && !listen.ip().is_loopback() && !cli.allow_anonymous {
        anyhow::bail!(
            "refusing to serve {listen} without authentication: anyone who can reach it could read and change the repository. \
             Set PANDO_TOKENS (or --oidc-issuer), listen on 127.0.0.1, or set PANDO_ALLOW_ANONYMOUS=1 to accept anonymous access on purpose"
        );
    }
    let limits = pando_server::Limits {
        request_timeout: std::time::Duration::from_secs(cli.request_timeout),
        max_in_flight: cli.max_in_flight.max(1),
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind(listen).await?;
    serve(listener, routes, auth, pando_server::ServeOptions { tls, web, limits, hosting }, shutdown_signal()).await
}
