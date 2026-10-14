//! End-to-end tests: a real gRPC server (and cache + daemon tiers) on loopback ports,
//! driven through the client library exactly as the `sap` CLI drives it.

use std::path::Path;
use std::sync::Arc;

use sap::features::{browse, clone, commit, git, land, sync};
use sap::remote::{GrpcRemote, Remote};
use sap::workspace::{ChangeKind, Config, Workspace};
use sapwood_adapters::flags::SpecFlags;
use sapwood_core::flags::FeatureFlags;
use sapwood_server::auth::{AllowAll, Authenticator, StaticTokens};
use sapwood_server::{authority_routes, config, serve, tier_routes};
use tokio::net::TcpListener;

struct Cluster {
    server: String,
    _shutdown: Vec<tokio::sync::oneshot::Sender<()>>,
}

async fn spawn(routes: tonic::service::Routes, auth: Arc<dyn Authenticator>) -> (String, tokio::sync::oneshot::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(serve(listener, routes, auth, None, async {
        let _ = rx.await;
    }));
    (url, tx)
}

async fn start_with(flags: SpecFlags, auth: Arc<dyn Authenticator>, metadata: &str) -> Cluster {
    let repo = config::open_repo(metadata, "metadata").await.unwrap();
    let (server, tx) = spawn(authority_routes(repo, Arc::new(flags)), auth).await;
    Cluster { server, _shutdown: vec![tx] }
}

async fn start() -> Cluster {
    start_with(SpecFlags::all_on(), Arc::new(AllowAll), "memory").await
}

fn remote(c: &Cluster) -> Arc<dyn Remote> {
    Arc::new(GrpcRemote::connect(&c.server, None, None).unwrap())
}

fn flags() -> SpecFlags {
    SpecFlags::all_on()
}

async fn clone_into(c: &Cluster, dir: &Path, sparse: &[&str]) -> Workspace {
    let config = Config {
        server: c.server.clone(),
        objects: None,
        ref_name: "main".into(),
        sparse: sparse.iter().map(|s| s.to_string()).collect(),
    };
    clone::run(remote(c).as_ref(), &flags(), dir, config).await.unwrap()
}

fn write(dir: &Path, rel: &str, content: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap()
}

async fn commit_land(ws: &mut Workspace, r: &dyn Remote, msg: &str) -> land::LandOutcome {
    commit::run(r, ws, msg, "tester <t@example.com>", &[]).await.unwrap().expect("something to commit");
    land::run(r, ws).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn clone_commit_land_sync_roundtrip() {
    let c = start().await;
    let r = remote(&c);
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());

    // Empty repo: clone, add files, land creates `main`.
    let mut wa = clone_into(&c, &a.path().join("w"), &[]).await;
    let a = a.path().join("w");
    write(&a, "README.md", "hello\n");
    write(&a, "src/lib.rs", "pub fn f() {}\n");
    write(&a, "src/deep/x/y.txt", "deep\n");
    let out = commit_land(&mut wa, r.as_ref(), "initial").await;
    assert!(!out.rebased);
    assert_eq!(r.get_ref("main").await.unwrap(), Some(out.landed));
    assert!(wa.status(&[]).unwrap().is_empty(), "clean after commit");

    // Second clone sees everything.
    let mut wb = clone_into(&c, &b.path().join("w"), &[]).await;
    let b = b.path().join("w");
    assert_eq!(read(&b, "src/deep/x/y.txt"), "deep\n");
    assert_eq!(wb.base, Some(out.landed));

    // B modifies + deletes; A syncs.
    write(&b, "README.md", "hello v2\n");
    std::fs::remove_file(b.join("src/deep/x/y.txt")).unwrap();
    let st = wb.status(&[]).unwrap();
    assert_eq!(st.iter().map(|c| (c.path.as_str(), c.kind)).collect::<Vec<_>>(), vec![
        ("README.md", ChangeKind::Modified),
        ("src/deep/x/y.txt", ChangeKind::Deleted)
    ]);
    commit_land(&mut wb, r.as_ref(), "edit").await;
    let s = sync::run(r.as_ref(), &mut wa, None).await.unwrap();
    assert_eq!(s.changed, 2);
    assert_eq!(read(&a, "README.md"), "hello v2\n");
    assert!(!a.join("src/deep").exists(), "empty dirs pruned");
    assert!(wa.status(&[]).unwrap().is_empty());

    // API-first reads without a workspace.
    assert_eq!(browse::cat(r.as_ref(), "main", "src/lib.rs").await.unwrap(), b"pub fn f() {}\n");
    let names: Vec<_> = browse::ls(r.as_ref(), "main", "").await.unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(names, vec!["README.md", "src"]);
    let log = browse::log(r.as_ref(), "main", "", 10).await.unwrap();
    assert_eq!(log.iter().map(|c| c.changeset.message.as_str()).collect::<Vec<_>>(), vec!["edit", "initial"]);
    assert_eq!(browse::log(r.as_ref(), "main", "src/lib.rs", 10).await.unwrap().len(), 1);
    assert_eq!(r.find("main", "lib", 0).await.unwrap(), vec!["src/lib.rs"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_lands_rebase_server_side() {
    let c = start().await;
    let r = remote(&c);
    let base = tempfile::tempdir().unwrap();
    let mut w0 = clone_into(&c, &base.path().join("w"), &[]).await;
    write(&base.path().join("w"), "shared.txt", "0\n");
    commit_land(&mut w0, r.as_ref(), "seed").await;

    // 8 workers each clone, edit their own file, and land concurrently.
    let mut tasks = Vec::new();
    for i in 0..8 {
        let server = c.server.clone();
        tasks.push(tokio::spawn(async move {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("w");
            let r: Arc<dyn Remote> = Arc::new(GrpcRemote::connect(&server, None, None).unwrap());
            let config = Config { server, objects: None, ref_name: "main".into(), sparse: vec![] };
            let mut ws = clone::run(r.as_ref(), &SpecFlags::all_on(), &path, config).await.unwrap();
            write(&path, &format!("worker/{i}.txt"), &format!("{i}\n"));
            commit::run(r.as_ref(), &mut ws, &format!("worker {i}"), "w", &[]).await.unwrap();
            let out = land::run(r.as_ref(), &mut ws).await.unwrap();
            assert!(out.behind.is_none());
            (dir, out.rebased)
        }));
    }
    let mut rebased = 0;
    let mut dirs = Vec::new();
    for t in tasks {
        let (d, rb) = t.await.unwrap();
        rebased += rb as usize;
        dirs.push(d);
    }
    // All 8 landed; history is linear with seed + 8 changesets; at least 7 were rebased
    // unless they happened to serialise perfectly (then 0) - both are valid, but every
    // file must be present at the head.
    let log = browse::log(r.as_ref(), "main", "", 100).await.unwrap();
    assert_eq!(log.len(), 9, "linear history, no lost lands");
    for i in 0..8 {
        assert_eq!(browse::cat(r.as_ref(), "main", &format!("worker/{i}.txt")).await.unwrap(), format!("{i}\n").as_bytes());
    }
    assert!(rebased <= 8);
}

#[tokio::test(flavor = "multi_thread")]
async fn conflicting_land_is_rejected_and_nothing_moves() {
    let c = start().await;
    let r = remote(&c);
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (ap, bp) = (a.path().join("w"), b.path().join("w"));
    let mut wa = clone_into(&c, &ap, &[]).await;
    write(&ap, "f.txt", "base\n");
    commit_land(&mut wa, r.as_ref(), "base").await;
    let mut wb = clone_into(&c, &bp, &[]).await;

    write(&ap, "f.txt", "from a\n");
    commit_land(&mut wa, r.as_ref(), "a").await;
    let head = r.get_ref("main").await.unwrap();

    write(&bp, "f.txt", "from b\n");
    commit::run(r.as_ref(), &mut wb, "b", "b", &[]).await.unwrap();
    let err = land::run(r.as_ref(), &mut wb).await.unwrap_err().to_string();
    assert!(err.contains("conflicting path") && err.contains("f.txt"), "{err}");
    assert_eq!(r.get_ref("main").await.unwrap(), head, "ref unchanged on conflict");
    // Sync is refused while B holds an unlanded changeset.
    assert!(sync::run(r.as_ref(), &mut wb, None).await.unwrap_err().to_string().contains("unlanded"));
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_refuses_to_clobber_uncommitted_edits() {
    let c = start().await;
    let r = remote(&c);
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (ap, bp) = (a.path().join("w"), b.path().join("w"));
    let mut wa = clone_into(&c, &ap, &[]).await;
    write(&ap, "f.txt", "1\n");
    commit_land(&mut wa, r.as_ref(), "1").await;
    let mut wb = clone_into(&c, &bp, &[]).await;
    write(&ap, "f.txt", "2\n");
    commit_land(&mut wa, r.as_ref(), "2").await;
    write(&bp, "f.txt", "local edit\n");
    let err = sync::run(r.as_ref(), &mut wb, None).await.unwrap_err().to_string();
    assert!(err.contains("uncommitted changes would be overwritten"), "{err}");
    assert_eq!(read(&bp, "f.txt"), "local edit\n", "local edit preserved");
}

#[tokio::test(flavor = "multi_thread")]
async fn sparse_checkout_materialises_only_requested_paths() {
    let c = start().await;
    let r = remote(&c);
    let a = tempfile::tempdir().unwrap();
    let ap = a.path().join("w");
    let mut wa = clone_into(&c, &ap, &[]).await;
    for p in ["services/api/main.rs", "services/web/app.ts", "libs/core/lib.rs", "README.md"] {
        write(&ap, p, p);
    }
    commit_land(&mut wa, r.as_ref(), "tree").await;

    let s = tempfile::tempdir().unwrap();
    let sp = s.path().join("w");
    let mut ws = clone_into(&c, &sp, &["services/api", "libs"]).await;
    assert!(sp.join("services/api/main.rs").exists());
    assert!(sp.join("libs/core/lib.rs").exists());
    assert!(!sp.join("services/web").exists());
    assert!(!sp.join("README.md").exists());
    assert_eq!(ws.index.len(), 2);

    // Commits from a sparse workspace never delete files outside the sparse set.
    write(&sp, "services/api/main.rs", "changed");
    commit_land(&mut ws, r.as_ref(), "sparse edit").await;
    assert_eq!(browse::cat(r.as_ref(), "main", "README.md").await.unwrap(), b"README.md");
    assert_eq!(browse::cat(r.as_ref(), "main", "services/api/main.rs").await.unwrap(), b"changed");

    // Sparse flag off => sparse clone refused.
    let off = SpecFlags::parse("sparse-checkout=off").unwrap();
    let cfg = Config { server: c.server.clone(), objects: None, ref_name: "main".into(), sparse: vec!["libs".into()] };
    let dir = tempfile::tempdir().unwrap();
    assert!(clone::run(r.as_ref(), &off, &dir.path().join("x"), cfg).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn tiers_serve_identical_objects_through_cache_and_daemon() {
    let c = start().await;
    let r = remote(&c);
    let a = tempfile::tempdir().unwrap();
    let ap = a.path().join("w");
    let mut wa = clone_into(&c, &ap, &[]).await;
    for i in 0..50 {
        write(&ap, &format!("d{}/f{i}.txt", i % 5), &format!("content {i}\n"));
    }
    commit_land(&mut wa, r.as_ref(), "fifty").await;

    // server <- cache <- daemon <- client
    let flags: Arc<dyn FeatureFlags> = Arc::new(SpecFlags::all_on());
    let (cache_url, _c) = spawn(tier_routes(config::cache_store(&c.server, None, 64 << 20).unwrap(), flags.clone()), Arc::new(AllowAll)).await;
    let ddir = tempfile::tempdir().unwrap();
    let daemon_store = config::daemon_store(&cache_url, None, ddir.path().to_str().unwrap(), 16 << 20).unwrap();
    let (daemon_url, _d) = spawn(tier_routes(daemon_store, flags), Arc::new(AllowAll)).await;

    for round in 0..2 {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("w");
        let cfg = Config { server: c.server.clone(), objects: Some(daemon_url.clone()), ref_name: "main".into(), sparse: vec![] };
        let via: Arc<dyn Remote> = Arc::new(GrpcRemote::connect(&c.server, Some(&daemon_url), None).unwrap());
        let ws = clone::run(via.as_ref(), &SpecFlags::all_on(), &p, cfg).await.unwrap();
        assert_eq!(ws.index.len(), 50, "round {round}");
        assert_eq!(read(&p, "d3/f13.txt"), "content 13\n");
    }
    // The daemon persisted objects to its disk tier.
    let persisted = walk_count(ddir.path());
    assert!(persisted >= 50, "daemon disk tier holds {persisted} objects");
}

fn walk_count(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() { walk_count(&e.path()) } else { 1 }
        })
        .sum()
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_tokens_and_feature_flags_enforced() {
    let c = start_with(
        SpecFlags::parse("land=off").unwrap(),
        Arc::new(StaticTokens::new(["s3cret".to_owned()])),
        "memory",
    )
    .await;
    let anon: Arc<dyn Remote> = Arc::new(GrpcRemote::connect(&c.server, None, None).unwrap());
    let err = anon.list_refs().await.unwrap_err().to_string();
    assert!(err.contains("missing bearer token") || err.contains("Unauthenticated"), "{err}");
    let bad: Arc<dyn Remote> = Arc::new(GrpcRemote::connect(&c.server, None, Some("nope")).unwrap());
    assert!(bad.list_refs().await.is_err());

    let ok: Arc<dyn Remote> = Arc::new(GrpcRemote::connect(&c.server, None, Some("s3cret")).unwrap());
    assert!(ok.list_refs().await.unwrap().is_empty());
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("w");
    let cfg = Config { server: c.server.clone(), objects: None, ref_name: "main".into(), sparse: vec![] };
    let mut ws = clone::run(ok.as_ref(), &flags(), &p, cfg).await.unwrap();
    write(&p, "x", "1");
    commit::run(ok.as_ref(), &mut ws, "m", "a", &[]).await.unwrap();
    let err = land::run(ok.as_ref(), &mut ws).await.unwrap_err().to_string();
    assert!(err.contains("feature `land` is disabled"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn sqlite_backend_persists_across_server_restart() {
    let data = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", data.path().join("meta.db").display());
    let head = {
        let c = start_with(SpecFlags::all_on(), Arc::new(AllowAll), &url).await;
        let r = remote(&c);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("w");
        let mut ws = clone_into(&c, &p, &[]).await;
        write(&p, "persist.txt", "durable\n");
        commit_land(&mut ws, r.as_ref(), "durable").await.landed
    };
    let c = start_with(SpecFlags::all_on(), Arc::new(AllowAll), &url).await;
    let r = remote(&c);
    assert_eq!(r.get_ref("main").await.unwrap(), Some(head));
    assert_eq!(browse::cat(r.as_ref(), "main", "persist.txt").await.unwrap(), b"durable\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn blame_attributes_each_line_to_its_changeset() {
    let c = start().await;
    let r = remote(&c);
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("w");
    let mut ws = clone_into(&c, &p, &[]).await;
    write(&p, "f.txt", "one\ntwo\nthree\n");
    let c1 = commit_land(&mut ws, r.as_ref(), "c1").await.landed;
    write(&p, "other.txt", "noise\n");
    commit_land(&mut ws, r.as_ref(), "unrelated").await;
    write(&p, "f.txt", "one\nTWO\nthree\nfour\n");
    let c3 = commit_land(&mut ws, r.as_ref(), "c3").await.landed;
    let blame = r.blame("main", "f.txt").await.unwrap();
    let owners: Vec<_> = blame.iter().map(|(id, l)| (l.as_str(), *id)).collect();
    assert_eq!(owners, vec![("one", c1), ("TWO", c3), ("three", c1), ("four", c3)]);
}

fn git_cmd(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn git_import_export_roundtrip_preserves_trees_exactly() {
    let src = tempfile::tempdir().unwrap();
    let s = src.path();
    git_cmd(s, &["init", "-q", "-b", "main"]);
    write(s, "a", "a\n");
    git_cmd(s, &["add", "."]);
    git_cmd(s, &["commit", "-qm", "one"]);
    write(s, "dir/x", "x\n");
    write(s, "sp ace \"q\".txt", "quoted\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(s.join("dir/x"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("dir/x", s.join("link")).unwrap();
    }
    git_cmd(s, &["add", "."]);
    git_cmd(s, &["commit", "-qm", "two"]);
    git_cmd(s, &["checkout", "-qb", "side"]);
    write(s, "side.txt", "s\n");
    git_cmd(s, &["add", "."]);
    git_cmd(s, &["commit", "-qm", "side"]);
    git_cmd(s, &["checkout", "-q", "main"]);
    git_cmd(s, &["rm", "-q", "a"]);
    write(s, "a/nested", "file became dir\n");
    git_cmd(s, &["add", "."]);
    git_cmd(s, &["commit", "-qm", "three"]);
    git_cmd(s, &["merge", "-q", "--no-edit", "side"]);
    let want_tree = git_cmd(s, &["rev-parse", "HEAD^{tree}"]);

    let c = start().await;
    let r = remote(&c);
    let out = git::import(r.as_ref(), &flags(), s, "HEAD", "main", false).await.unwrap();
    assert_eq!(out.commits, 5);
    // Idempotent: importing again changes nothing.
    let again = git::import(r.as_ref(), &flags(), s, "HEAD", "main", false).await.unwrap();
    assert_eq!(again.tip, out.tip);
    assert_eq!(again.uploaded, 0, "no blobs re-uploaded");
    assert_eq!(browse::cat(r.as_ref(), "main", "a/nested").await.unwrap(), b"file became dir\n");
    assert_eq!(browse::cat(r.as_ref(), "main", "sp ace \"q\".txt").await.unwrap(), b"quoted\n");

    // Incremental: a new git commit fast-forwards the ref.
    write(s, "later.txt", "later\n");
    git_cmd(s, &["add", "."]);
    git_cmd(s, &["commit", "-qm", "later"]);
    let inc = git::import(r.as_ref(), &flags(), s, "HEAD", "main", false).await.unwrap();
    assert_ne!(inc.tip, out.tip);
    let want_tree = {
        let _ = want_tree;
        git_cmd(s, &["rev-parse", "HEAD^{tree}"])
    };

    // Export to a fresh git repo: the tip tree hash is bit-for-bit identical.
    let dst = tempfile::tempdir().unwrap();
    let n = git::export(r.as_ref(), &flags(), "main", dst.path(), "main").await.unwrap();
    assert!(n >= 5);
    assert_eq!(git_cmd(dst.path(), &["rev-parse", "main^{tree}"]), want_tree);
}

#[tokio::test(flavor = "multi_thread")]
async fn checkout_branch_and_switch() {
    let c = start().await;
    let r = remote(&c);
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("w");
    let mut ws = clone_into(&c, &p, &[]).await;
    write(&p, "f", "main\n");
    let m1 = commit_land(&mut ws, r.as_ref(), "m1").await.landed;
    sync::create_branch(r.as_ref(), &mut ws, "feature/x").await.unwrap();
    write(&p, "f", "feature\n");
    commit_land(&mut ws, r.as_ref(), "on feature").await;
    assert_eq!(r.get_ref("main").await.unwrap(), Some(m1), "main untouched");
    sync::checkout(r.as_ref(), &mut ws, "main").await.unwrap();
    assert_eq!(read(&p, "f"), "main\n");
    assert_eq!(ws.config.ref_name, "main");
    assert_eq!(r.merge_base("main", "feature/x").await.unwrap(), Some(m1));
}
