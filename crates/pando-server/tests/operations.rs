//! Running pandod as a hosted service: health endpoints a load balancer can call without a
//! token, operator takedown and deletion of a repository, and a clean stop on SIGTERM.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use pando_proto::platform_client::PlatformClient;
use pando_proto::{CreateRepositoryRequest, RepositoryRef};
use tonic::transport::Channel;

const ADMIN: &str = "admin-0123456789abcdef";

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

struct Pandod {
    child: Child,
    grpc: String,
    http: String,
    platform: String,
    dir: tempfile::TempDir,
}

fn start() -> Pandod {
    let dir = tempfile::tempdir().unwrap();
    let (grpc, http) = (free_port(), free_port());
    let platform = format!("sqlite://{}/platform.db", dir.path().display());
    let child = Command::new(env!("CARGO_BIN_EXE_pandod"))
        .args(["server", "--listen", &format!("127.0.0.1:{grpc}"), "--metadata", "memory"])
        .args(["--platform", &platform, "--platform-admins", "admin"])
        .args(["--repos-metadata", &format!("sqlite://{}/repos/{{repo}}.db", dir.path().display())])
        .args(["--metrics", &format!("127.0.0.1:{http}")])
        .env("PANDO_TOKENS", format!("admin:{ADMIN}"))
        .env_remove("PANDO_WEB_ORIGINS")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    Pandod { child, grpc: format!("http://127.0.0.1:{grpc}"), http: format!("http://127.0.0.1:{http}"), platform, dir }
}

async fn get(url: &str) -> Option<u16> {
    reqwest::get(url).await.ok().map(|r| r.status().as_u16())
}

/// Polls until `f` holds, for up to 10 s.
async fn eventually<F: AsyncFnMut() -> bool>(what: &str, mut f: F) {
    let start = Instant::now();
    while !f().await {
        assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn platform(
    url: &str,
) -> PlatformClient<tonic::service::interceptor::InterceptedService<Channel, pando_proto::BearerAuth>> {
    let ch = Channel::from_shared(url.to_owned()).unwrap().connect().await.unwrap();
    PlatformClient::with_interceptor(ch, pando_proto::BearerAuth::new(Some(ADMIN)).unwrap())
}

fn repo_cmd(platform: &str, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pandod")).args(["repo", "--platform", platform]).args(args).output().unwrap()
}

#[tokio::test]
async fn health_takedown_delete_and_sigterm() {
    let mut s = start();
    eventually("readiness", async || get(&format!("{}/readyz", s.http)).await == Some(200)).await;
    assert_eq!(get(&format!("{}/healthz", s.http)).await, Some(200));

    // gRPC health needs no token and reports serving.
    let ch = Channel::from_shared(s.grpc.clone()).unwrap().connect().await.unwrap();
    let mut health = tonic_health::pb::health_client::HealthClient::new(ch);
    let r = health.check(tonic_health::pb::HealthCheckRequest { service: String::new() }).await.unwrap();
    assert_eq!(r.into_inner().status, tonic_health::pb::health_check_response::ServingStatus::Serving as i32);

    // A data directory that stops taking writes makes the server not ready, and back.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let repos = s.dir.path().join("repos");
        std::fs::create_dir_all(&repos).unwrap();
        std::fs::set_permissions(&repos, std::fs::Permissions::from_mode(0o500)).unwrap();
        let writable = std::fs::File::create(repos.join("probe")).is_ok();
        if !writable {
            eventually("not ready", async || get(&format!("{}/readyz", s.http)).await == Some(503)).await;
            let r = health.check(tonic_health::pb::HealthCheckRequest { service: String::new() }).await.unwrap();
            assert_eq!(
                r.into_inner().status,
                tonic_health::pb::health_check_response::ServingStatus::NotServing as i32
            );
        }
        std::fs::set_permissions(&repos, std::fs::Permissions::from_mode(0o700)).unwrap();
        eventually("ready again", async || get(&format!("{}/readyz", s.http)).await == Some(200)).await;
    }

    // Takedown: every call refused within seconds, nothing deleted; enable undoes it.
    let mut p = platform(&s.grpc).await;
    let create = CreateRepositoryRequest { owner: "lumen".into(), name: "fleet".into(), ..Default::default() };
    p.create_repository(create).await.unwrap();
    let fleet = RepositoryRef { owner: "lumen".into(), name: "fleet".into() };
    let out = repo_cmd(&s.platform, &["disable", "lumen/fleet", "--reason", "court order 7"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    eventually("takedown", async || {
        p.get_repository(fleet.clone()).await.is_err_and(|e| e.message().contains("disabled by the operator"))
    })
    .await;
    assert!(repo_cmd(&s.platform, &["enable", "lumen/fleet"]).status.success());
    eventually("enabled again", async || p.get_repository(fleet.clone()).await.is_ok()).await;

    // Deletion: the registry row and the storage files go. Windows refuses to delete a file the
    // running server holds open, so there the server must be stopped first (docs/deploy.md).
    #[cfg(windows)]
    let _ = s.dir.path();
    #[cfg(not(windows))]
    {
        let db = s.dir.path().join("repos/lumen/fleet.db");
        assert!(db.exists());
        let refused =
            repo_cmd(&s.platform, &["delete", "lumen/fleet", "--repos-metadata", "x/{repo}.db", "--confirm", "nope"]);
        assert!(!refused.status.success(), "the name must be confirmed");
        let tpl = format!("sqlite://{}/repos/{{repo}}.db", s.dir.path().display());
        let out =
            repo_cmd(&s.platform, &["delete", "lumen/fleet", "--repos-metadata", &tpl, "--confirm", "lumen/fleet"]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(!db.exists());
        eventually("gone", async || {
            p.get_repository(fleet.clone()).await.is_err_and(|e| e.code() == tonic::Code::NotFound)
        })
        .await;
    }

    // SIGTERM: stops cleanly.
    #[cfg(unix)]
    {
        Command::new("kill").args(["-TERM", &s.child.id().to_string()]).status().unwrap();
        let start = Instant::now();
        let status = loop {
            if let Some(st) = s.child.try_wait().unwrap() {
                break st;
            }
            assert!(start.elapsed() < Duration::from_secs(15), "pandod did not stop on SIGTERM");
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert!(status.success(), "{status:?}");
    }
    let _ = s.child.kill();
}
