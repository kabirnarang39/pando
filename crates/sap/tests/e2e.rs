//! End-to-end tests: a real gRPC server (and cache + daemon tiers) on loopback ports,
//! driven through the client library exactly as the `sap` CLI drives it.

use std::path::Path;
use std::sync::Arc;

use sap::features::{browse, clone, commit, git, land, sync};
use sap::remote::{GrpcRemote, Remote};
use sap::workspace::{ChangeKind, Config, Workspace};
use sapwood_adapters::flags::SpecFlags;
use sapwood_core::Id;
use sapwood_core::acl::{AccessPolicy, AllowAllPolicy};
use sapwood_core::flags::FeatureFlags;
use sapwood_core::ports::{LandCheck, NoChecks};
use sapwood_server::auth::{AllowAll, Authenticator, StaticTokens};
use sapwood_server::{authority_routes, config, serve, tier_routes};
use tokio::net::TcpListener;

struct Cluster {
    server: String,
    _shutdown: Vec<tokio::sync::oneshot::Sender<()>>,
}

async fn spawn(
    routes: tonic::service::Routes,
    auth: Arc<dyn Authenticator>,
) -> (String, tokio::sync::oneshot::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(serve(listener, routes, auth, None, async {
        let _ = rx.await;
    }));
    (url, tx)
}

async fn start_with(flags: SpecFlags, auth: Arc<dyn Authenticator>, metadata: &str) -> Cluster {
    start_full(flags, auth, metadata, Arc::new(AllowAllPolicy)).await
}

async fn start_full(
    flags: SpecFlags,
    auth: Arc<dyn Authenticator>,
    metadata: &str,
    policy: Arc<dyn AccessPolicy>,
) -> Cluster {
    start_checked(flags, auth, metadata, policy, Arc::new(NoChecks)).await
}

async fn start_checked(
    flags: SpecFlags,
    auth: Arc<dyn Authenticator>,
    metadata: &str,
    policy: Arc<dyn AccessPolicy>,
    checks: Arc<dyn LandCheck>,
) -> Cluster {
    let repo = config::open_repo(metadata, "metadata").await.unwrap();
    let (server, tx) = spawn(authority_routes(repo, Arc::new(flags), policy, checks), auth).await;
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
    assert_eq!(
        st.iter().map(|c| (c.path.as_str(), c.kind)).collect::<Vec<_>>(),
        vec![("README.md", ChangeKind::Modified), ("src/deep/x/y.txt", ChangeKind::Deleted)]
    );
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
        assert_eq!(
            browse::cat(r.as_ref(), "main", &format!("worker/{i}.txt")).await.unwrap(),
            format!("{i}\n").as_bytes()
        );
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
    let (cache_url, _c) =
        spawn(tier_routes(config::cache_store(&c.server, None, 64 << 20).unwrap(), flags.clone()), Arc::new(AllowAll))
            .await;
    let ddir = tempfile::tempdir().unwrap();
    let daemon_store = config::daemon_store(&cache_url, None, ddir.path().to_str().unwrap(), 16 << 20).unwrap();
    let (daemon_url, _d) = spawn(tier_routes(daemon_store, flags), Arc::new(AllowAll)).await;

    for round in 0..2 {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("w");
        let cfg = Config {
            server: c.server.clone(),
            objects: Some(daemon_url.clone()),
            ref_name: "main".into(),
            sparse: vec![],
        };
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
    let c =
        start_with(SpecFlags::parse("land=off").unwrap(), Arc::new(StaticTokens::new(["s3cret".to_owned()])), "memory")
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
    // Quotes are illegal in Windows filenames; exercise git's path quoting elsewhere.
    #[cfg(unix)]
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
    // Case-only rename (regression: folded on case-insensitive filesystems).
    write(s, "Case.txt", "case\n");
    git_cmd(s, &["add", "."]);
    git_cmd(s, &["commit", "-qm", "case"]);
    git_cmd(s, &["mv", "Case.txt", "case.txt"]);
    git_cmd(s, &["commit", "-qm", "rename case"]);
    let want_tree = git_cmd(s, &["rev-parse", "HEAD^{tree}"]);

    let c = start().await;
    let r = remote(&c);
    let out = git::import(r.as_ref(), &flags(), s, "HEAD", "main", false).await.unwrap();
    assert_eq!(out.commits, 7);
    // Idempotent: importing again changes nothing.
    let again = git::import(r.as_ref(), &flags(), s, "HEAD", "main", false).await.unwrap();
    assert_eq!(again.tip, out.tip);
    assert_eq!(again.uploaded, 0, "no blobs re-uploaded");
    assert_eq!(browse::cat(r.as_ref(), "main", "a/nested").await.unwrap(), b"file became dir\n");
    #[cfg(unix)]
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

#[tokio::test(flavor = "multi_thread")]
async fn per_directory_write_and_ref_permissions() {
    let policy = sapwood_adapters::acl::RulesPolicy::parse(
        r#"{"default":{"refs":["user/*"]},
            "principals":{"api":{"write":["services/api"],"refs":["main"]},
                          "admin":{"write":["*"],"refs":["*"]}}}"#,
    )
    .unwrap();
    let tokens = StaticTokens::new(["api:api-token".to_owned(), "admin:admin-token".to_owned()]);
    let c = start_full(SpecFlags::all_on(), Arc::new(tokens), "memory", Arc::new(policy)).await;
    let as_ = |t: &str| -> Arc<dyn Remote> { Arc::new(GrpcRemote::connect(&c.server, None, Some(t)).unwrap()) };
    let (admin, api) = (as_("admin-token"), as_("api-token"));

    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("w");
    let cfg = |s: &str| Config { server: s.to_owned(), objects: None, ref_name: "main".into(), sparse: vec![] };
    let mut wa = clone::run(admin.as_ref(), &flags(), &p, cfg(&c.server)).await.unwrap();
    write(&p, "services/api/a.rs", "a");
    write(&p, "libs/core/b.rs", "b");
    commit_land(&mut wa, admin.as_ref(), "seed").await;

    let d2 = tempfile::tempdir().unwrap();
    let p2 = d2.path().join("w");
    let mut wb = clone::run(api.as_ref(), &flags(), &p2, cfg(&c.server)).await.unwrap();
    write(&p2, "libs/core/b.rs", "not yours");
    let err = commit::run(api.as_ref(), &mut wb, "x", "api", &[]).await.unwrap_err().to_string();
    assert!(err.contains("may not write libs/core/b.rs"), "{err}");
    std::fs::write(p2.join("libs/core/b.rs"), "b").unwrap();
    write(&p2, "services/api/a.rs", "mine");
    commit_land(&mut wb, api.as_ref(), "api change").await;
    // api may land on main but not create release refs.
    let base = wb.base.unwrap();
    let err = api.compare_and_swap("release/1", None, base).await.unwrap_err().to_string();
    assert!(err.contains("may not update ref"), "{err}");
    assert!(admin.compare_and_swap("release/1", None, base).await.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_server_speaks_json_rpc_and_reads_repo() {
    use rmcp::ServiceExt;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let c = start().await;
    let r = remote(&c);
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("w");
    let mut ws = clone_into(&c, &p, &[]).await;
    write(&p, "docs/hello.md", "hi from sapwood\n");
    commit_land(&mut ws, r.as_ref(), "doc").await;

    let (client, server) = tokio::io::duplex(1 << 16);
    let (sr, sw) = tokio::io::split(server);
    let mcp = sap::mcp::SapMcp::new(r.clone(), Some(p.clone()));
    tokio::spawn(async move { mcp.serve((sr, sw)).await.unwrap().waiting().await.unwrap() });
    let (cr, mut cw) = tokio::io::split(client);
    let mut lines = BufReader::new(cr).lines();
    let mut send = async |v: serde_json::Value| {
        cw.write_all(format!("{v}\n").as_bytes()).await.unwrap();
    };
    let mut recv =
        async || -> serde_json::Value { serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap() };

    send(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}))
    .await;
    let init = recv().await;
    assert!(init["result"]["capabilities"]["tools"].is_object(), "{init}");
    send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;

    send(serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await;
    let list = recv().await;
    let names: Vec<&str> =
        list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for want in [
        "sap_cat",
        "sap_ls",
        "sap_find",
        "sap_log",
        "sap_blame",
        "sap_diff",
        "sap_status",
        "sap_commit",
        "sap_land",
        "sap_sync",
    ] {
        assert!(names.contains(&want), "missing {want} in {names:?}");
    }

    send(serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"sap_cat","arguments":{"path":"docs/hello.md"}}})).await;
    let cat = recv().await;
    assert_eq!(cat["result"]["content"][0]["text"], "hi from sapwood\n", "{cat}");

    // An agent edits, commits and lands entirely through MCP.
    write(&p, "docs/hello.md", "edited by agent\n");
    send(serde_json::json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"sap_commit","arguments":{"message":"agent edit"}}})).await;
    let commit = recv().await;
    assert!(commit["result"]["content"][0]["text"].as_str().unwrap().starts_with("committed"), "{commit}");
    send(serde_json::json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"sap_land","arguments":{}}}))
        .await;
    let landed = recv().await;
    assert!(landed["result"]["content"][0]["text"].as_str().unwrap().starts_with("landed"), "{landed}");
    assert_eq!(browse::cat(r.as_ref(), "main", "docs/hello.md").await.unwrap(), b"edited by agent\n");

    // Errors surface as tool errors, not protocol failures.
    send(serde_json::json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"sap_cat","arguments":{"path":"nope"}}})).await;
    let missing = recv().await;
    assert_eq!(missing["result"]["isError"], true, "{missing}");
}

#[tokio::test(flavor = "multi_thread")]
async fn file_and_directory_swaps_commit_cleanly() {
    let c = start().await;
    let r = remote(&c);
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("w");
    let mut ws = clone_into(&c, &p, &[]).await;
    write(&p, "a", "file a\n");
    write(&p, "b/inner.txt", "inner b\n");
    commit_land(&mut ws, r.as_ref(), "seed").await;

    // File -> directory and directory -> file in one commit.
    std::fs::remove_file(p.join("a")).unwrap();
    write(&p, "a/now-a-dir.txt", "dir a\n");
    std::fs::remove_dir_all(p.join("b")).unwrap();
    write(&p, "b", "file b\n");
    commit_land(&mut ws, r.as_ref(), "swap").await;
    assert!(ws.status(&[]).unwrap().is_empty(), "clean after commit: {:?}", ws.status(&[]).unwrap());
    assert_eq!(browse::cat(r.as_ref(), "main", "a/now-a-dir.txt").await.unwrap(), b"dir a\n");
    assert_eq!(browse::cat(r.as_ref(), "main", "b").await.unwrap(), b"file b\n");

    // Another workspace syncs across the swap.
    let d2 = tempfile::tempdir().unwrap();
    let p2 = d2.path().join("w");
    let ws2 = clone_into(&c, &p2, &[]).await;
    assert_eq!(read(&p2, "a/now-a-dir.txt"), "dir a\n");
    assert_eq!(read(&p2, "b"), "file b\n");
    assert_eq!(ws2.index.len(), 2);
}

/// Run git with the remote helper on PATH. Returns (success, stdout, stderr).
fn git_bridge(dir: &Path, args: &[&str], extra: &[(&str, &str)]) -> (bool, String, String) {
    let helper_dir = Path::new(env!("CARGO_BIN_EXE_git-remote-sapwood")).parent().unwrap().to_owned();
    let mut dirs = vec![helper_dir];
    dirs.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    let path = std::env::join_paths(dirs).unwrap();
    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(dir)
        .args(args)
        .env("PATH", path)
        .env("GIT_AUTHOR_NAME", "g")
        .env("GIT_AUTHOR_EMAIL", "g@example.com")
        .env("GIT_COMMITTER_NAME", "g")
        .env("GIT_COMMITTER_EMAIL", "g@example.com")
        // Byte-exact checkouts on every OS (Windows runners default to autocrlf=true).
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.autocrlf")
        .env("GIT_CONFIG_VALUE_0", "false");
    for (k, v) in extra {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn git_ok(dir: &Path, args: &[&str]) -> String {
    let (ok, out, err) = git_bridge(dir, args, &[]);
    assert!(ok, "git {args:?} failed: {err}");
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn git_bridge_clone_push_pull_and_concurrent_push() {
    let c = start().await;
    let r = remote(&c);
    let d = tempfile::tempdir().unwrap();
    let sp = d.path().join("sap-ws");
    let mut ws = clone_into(&c, &sp, &[]).await;
    write(&sp, "README.md", "hello\n");
    write(&sp, "src/lib.rs", "pub fn a() {}\n");
    commit_land(&mut ws, r.as_ref(), "initial from sap").await;
    let url = format!("sapwood::{}", c.server);

    // Plain git clone through the helper.
    git_ok(d.path(), &["clone", "-q", &url, "g1"]);
    let g1 = d.path().join("g1");
    assert_eq!(read(&g1, "src/lib.rs"), "pub fn a() {}\n");
    assert_eq!(git_ok(&g1, &["log", "--format=%s"]), "initial from sap");

    // git push lands on the server.
    write(&g1, "src/lib.rs", "pub fn a() {}\npub fn b() {}\n");
    git_ok(&g1, &["commit", "-qam", "add b from git"]);
    git_ok(&g1, &["push", "-q", "origin", "main"]);
    assert_eq!(browse::cat(r.as_ref(), "main", "src/lib.rs").await.unwrap(), b"pub fn a() {}\npub fn b() {}\n");

    // A sap client lands; git pull fetches only the new changeset.
    sync::run(r.as_ref(), &mut ws, None).await.unwrap();
    write(&sp, "docs/new.md", "from sap\n");
    commit_land(&mut ws, r.as_ref(), "docs from sap").await;
    git_ok(&g1, &["pull", "-q", "--rebase", "origin", "main"]);
    assert_eq!(read(&g1, "docs/new.md"), "from sap\n");
    assert_eq!(git_ok(&g1, &["log", "--format=%s"]).lines().count(), 3);

    // Two git clones push different files at the same time: both land, no rejection.
    git_ok(d.path(), &["clone", "-q", &url, "g2"]);
    let g2 = d.path().join("g2");
    write(&g1, "one.txt", "1\n");
    git_ok(&g1, &["add", "."]);
    git_ok(&g1, &["commit", "-qm", "one from g1"]);
    write(&g2, "two.txt", "2\n");
    git_ok(&g2, &["add", "."]);
    git_ok(&g2, &["commit", "-qm", "two from g2"]);
    git_ok(&g1, &["push", "-q", "origin", "main"]);
    let (ok, _, err) = git_bridge(&g2, &["push", "origin", "main"], &[]);
    assert!(ok, "behind push should be rebased, not rejected: {err}");
    assert!(err.contains("rebased your push"), "{err}");
    assert_eq!(browse::cat(r.as_ref(), "main", "one.txt").await.unwrap(), b"1\n");
    assert_eq!(browse::cat(r.as_ref(), "main", "two.txt").await.unwrap(), b"2\n");
    // The pusher catches up cleanly: git drops its now-duplicate local commit.
    git_ok(&g2, &["pull", "-q", "--rebase", "origin", "main"]);
    assert_eq!(read(&g2, "one.txt"), "1\n");
    assert_eq!(git_ok(&g2, &["status", "--porcelain"]), "");
    let subjects = git_ok(&g2, &["log", "--format=%s"]);
    assert_eq!(subjects.lines().filter(|s| *s == "two from g2").count(), 1, "{subjects}");
    assert_eq!(git_ok(&g2, &["rev-list", "--count", "HEAD"]), "5");

    // Strict mode keeps git's reject-if-behind behaviour.
    write(&g1, "three.txt", "3\n");
    git_ok(&g1, &["add", "."]);
    git_ok(&g1, &["commit", "-qm", "three, behind"]);
    let (ok, _, err) = git_bridge(&g1, &["push", "origin", "main"], &[("SAPWOOD_PUSH", "strict")]);
    assert!(!ok && err.contains("non-fast-forward"), "{err}");

    // New branches can be pushed.
    git_ok(&g2, &["checkout", "-qb", "feature/x"]);
    write(&g2, "feature.txt", "f\n");
    git_ok(&g2, &["add", "."]);
    git_ok(&g2, &["commit", "-qm", "feature"]);
    git_ok(&g2, &["push", "-q", "origin", "feature/x"]);
    assert!(r.get_ref("feature/x").await.unwrap().is_some());
    assert_eq!(browse::cat(r.as_ref(), "feature/x", "feature.txt").await.unwrap(), b"f\n");

    // Case-only rename landed by sap, fetched by git (regression: on case-insensitive
    // filesystems git's fast-import kept the old name).
    sync::run(r.as_ref(), &mut ws, None).await.unwrap();
    write(&sp, "Case.txt", "case\n");
    commit_land(&mut ws, r.as_ref(), "add Case.txt").await;
    std::fs::rename(sp.join("Case.txt"), sp.join("tmp-name")).unwrap();
    std::fs::rename(sp.join("tmp-name"), sp.join("case.txt")).unwrap();
    commit_land(&mut ws, r.as_ref(), "rename to case.txt").await;
    git_ok(d.path(), &["clone", "-q", &url, "g4"]);
    let names = git_ok(&d.path().join("g4"), &["ls-tree", "--name-only", "HEAD"]);
    assert!(names.lines().any(|n| n == "case.txt") && !names.lines().any(|n| n == "Case.txt"), "{names}");

    // Feature flag off: the helper refuses.
    let (ok, _, err) = git_bridge(d.path(), &["clone", "-q", &url, "g3"], &[("SAPWOOD_FLAGS", "git-bridge=off")]);
    assert!(!ok && err.contains("git-bridge"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn oidc_tokens_authenticate_and_drive_permissions() {
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    use sapwood_server::oidc::{FirstMatch, OidcAuthenticator, OidcConfig};

    let jwks = include_str!("../../sapwood-server/tests/fixtures/test-oidc-jwks.json");
    let key = include_bytes!("../../sapwood-server/tests/fixtures/test-oidc-key.pem");
    let oidc = OidcAuthenticator::with_jwks(
        OidcConfig {
            issuer: "https://idp.example.com".into(),
            audience: "sapwood".into(),
            principal_claim: "email".into(),
        },
        serde_json::from_str(jwks).unwrap(),
    );
    let exp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 300;
    let token = |email: &str| {
        let claims =
            serde_json::json!({ "iss": "https://idp.example.com", "aud": "sapwood", "email": email, "exp": exp });
        let mut h = Header::new(Algorithm::RS256);
        h.kid = Some("test-key-1".into());
        jsonwebtoken::encode(&h, &claims, &EncodingKey::from_rsa_pem(key).unwrap()).unwrap()
    };
    let policy = sapwood_adapters::acl::RulesPolicy::parse(
        r#"{"principals":{"alice@example.com":{"write":["*"],"refs":["*"]},"bob@example.com":{"write":["docs"],"refs":["main"]}}}"#,
    )
    .unwrap();
    let auth = FirstMatch(vec![Arc::new(StaticTokens::new(["ci:ci-token".to_owned()])), Arc::new(oidc)]);
    let c = start_full(SpecFlags::all_on(), Arc::new(auth), "memory", Arc::new(policy)).await;
    let as_ = |t: &str| -> Arc<dyn Remote> { Arc::new(GrpcRemote::connect(&c.server, None, Some(t)).unwrap()) };
    let (alice, bob) = (as_(&token("alice@example.com")), as_(&token("bob@example.com")));

    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("w");
    let cfg = Config { server: c.server.clone(), objects: None, ref_name: "main".into(), sparse: vec![] };
    let mut wa = clone::run(alice.as_ref(), &flags(), &p, cfg.clone()).await.unwrap();
    write(&p, "src/main.rs", "fn main() {}\n");
    commit_land(&mut wa, alice.as_ref(), "alice via SSO").await;

    let d2 = tempfile::tempdir().unwrap();
    let p2 = d2.path().join("w");
    let mut wb = clone::run(bob.as_ref(), &flags(), &p2, cfg).await.unwrap();
    write(&p2, "src/main.rs", "fn main() { evil() }\n");
    let err = commit::run(bob.as_ref(), &mut wb, "x", "bob", &[]).await.unwrap_err().to_string();
    assert!(err.contains("bob@example.com may not write src/main.rs"), "{err}");

    // Static CI tokens still work next to SSO; garbage tokens do not.
    assert!(as_("ci-token").list_refs().await.is_ok());
    assert!(as_("not-a-token").list_refs().await.is_err());
}

fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (x >> 56) as u8
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn large_files_are_chunked_streamed_and_deduplicated() {
    let c = start().await;
    let r = remote(&c);
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("w");
    let mut ws = clone_into(&c, &p, &[]).await;
    let big = noise(40 << 20, 42); // incompressible, well above the chunking threshold
    std::fs::create_dir_all(p.join("assets")).unwrap();
    std::fs::write(p.join("assets/model.bin"), &big).unwrap();
    write(&p, "small.txt", "small\n");
    let first = commit::run(r.as_ref(), &mut ws, "add model", "t", &[]).await.unwrap().unwrap();
    assert!(first.uploaded >= 12, "chunks + manifest + small file: {}", first.uploaded);
    land::run(r.as_ref(), &mut ws).await.unwrap();
    assert!(ws.status(&[]).unwrap().is_empty());

    // Server-side read reassembles and verifies.
    assert_eq!(browse::cat(r.as_ref(), "main", "assets/model.bin").await.unwrap(), big);

    // Fresh clone, directly and through cache + daemon tiers.
    let d2 = tempfile::tempdir().unwrap();
    let p2 = d2.path().join("w");
    let ws2 = clone_into(&c, &p2, &[]).await;
    assert!(std::fs::read(p2.join("assets/model.bin")).unwrap() == big);
    assert!(ws2.status(&[]).unwrap().is_empty(), "streamed checkout matches its index");
    let tier_flags: Arc<dyn FeatureFlags> = Arc::new(SpecFlags::all_on());
    let (cache_url, _c1) = spawn(
        tier_routes(config::cache_store(&c.server, None, 256 << 20).unwrap(), tier_flags.clone()),
        Arc::new(AllowAll),
    )
    .await;
    let ddir = tempfile::tempdir().unwrap();
    let daemon = config::daemon_store(&cache_url, None, ddir.path().to_str().unwrap(), 64 << 20).unwrap();
    let (daemon_url, _c2) = spawn(tier_routes(daemon, tier_flags), Arc::new(AllowAll)).await;
    let d3 = tempfile::tempdir().unwrap();
    let via: Arc<dyn Remote> = Arc::new(GrpcRemote::connect(&c.server, Some(&daemon_url), None).unwrap());
    let cfg =
        Config { server: c.server.clone(), objects: Some(daemon_url.clone()), ref_name: "main".into(), sparse: vec![] };
    clone::run(via.as_ref(), &flags(), &d3.path().join("w"), cfg).await.unwrap();
    assert!(std::fs::read(d3.path().join("w/assets/model.bin")).unwrap() == big);

    // A small edit in the middle re-uploads only the chunks it touches.
    let mut edited = big.clone();
    edited.splice(20 << 20..20 << 20, b"patched weights".iter().copied());
    std::fs::write(p.join("assets/model.bin"), &edited).unwrap();
    let second = commit::run(r.as_ref(), &mut ws, "patch model", "t", &[]).await.unwrap().unwrap();
    assert!(second.uploaded <= 4, "only changed chunks + manifest uploaded, got {}", second.uploaded);
    land::run(r.as_ref(), &mut ws).await.unwrap();
    let mut ws2 = ws2;
    sync::run(r.as_ref(), &mut ws2, None).await.unwrap();
    assert!(std::fs::read(p2.join("assets/model.bin")).unwrap() == edited);

    // A manifest that lies about its id is refused by the server.
    let mut chunks = Vec::new();
    let (_, manifest) = sapwood_core::chunked::chunk_reader(&noise(9 << 20, 7)[..], |id, p| {
        chunks.push((id, p));
        Ok(())
    })
    .unwrap();
    let objects = r.objects();
    objects.put_many(chunks).await.unwrap();
    let err = objects.put_many(vec![(sapwood_core::Id([9; 32]), manifest)]).await.unwrap_err().to_string();
    assert!(err.contains("does not match"), "{err}");

    // git export carries the large file byte-for-byte.
    let g = tempfile::tempdir().unwrap();
    git::export(r.as_ref(), &flags(), "main", g.path(), "main").await.unwrap();
    git_cmd(g.path(), &["checkout", "-q", "main"]);
    assert!(std::fs::read(g.path().join("assets/model.bin")).unwrap() == edited);
}

#[tokio::test(flavor = "multi_thread")]
async fn path_scoped_read_permissions() {
    let policy = sapwood_adapters::acl::RulesPolicy::parse(
        r#"{"principals":{
            "alice":{"read":["*"],"write":["*"],"refs":["*"]},
            "bob":{"read":["services/api","docs"],"write":["services/api"],"refs":["main"]}}}"#,
    )
    .unwrap();
    let tokens = StaticTokens::new(["alice:a-token".to_owned(), "bob:b-token".to_owned()]);
    let c = start_full(SpecFlags::all_on(), Arc::new(tokens), "memory", Arc::new(policy)).await;
    let as_ = |t: &str| -> Arc<dyn Remote> { Arc::new(GrpcRemote::connect(&c.server, None, Some(t)).unwrap()) };
    let (alice, bob) = (as_("a-token"), as_("b-token"));
    let cfg = |_who: &str| Config { server: c.server.clone(), objects: None, ref_name: "main".into(), sparse: vec![] };

    let da = tempfile::tempdir().unwrap();
    let pa = da.path().join("w");
    let mut wa = clone::run(alice.as_ref(), &flags(), &pa, cfg("a")).await.unwrap();
    write(&pa, "services/api/main.rs", "api v1\n");
    write(&pa, "docs/guide.md", "guide\n");
    write(&pa, "secrets/prod.key", "TOP SECRET\n");
    write(&pa, "services/billing/ledger.rs", "ledger\n");
    let seed = commit_land(&mut wa, alice.as_ref(), "seed").await.landed;

    // Bob's clone contains only what he may read.
    let db = tempfile::tempdir().unwrap();
    let pb = db.path().join("w");
    let mut wb = clone::run(bob.as_ref(), &flags(), &pb, cfg("b")).await.unwrap();
    assert_eq!(read(&pb, "services/api/main.rs"), "api v1\n");
    assert_eq!(read(&pb, "docs/guide.md"), "guide\n");
    assert!(!pb.join("secrets").exists() && !pb.join("services/billing").exists());
    assert!(wb.status(&[]).unwrap().is_empty());

    // Every read path refuses or hides secrets.
    assert!(bob.read_file("main", "secrets/prod.key").await.unwrap_err().to_string().contains("may not read"));
    let root: Vec<String> = bob.list_dir("main", "").await.unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(root, vec!["docs", "services"]);
    let services: Vec<String> = bob.list_dir("main", "services").await.unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(services, vec!["api"]);
    assert!(bob.list_dir("main", "secrets").await.is_err());
    assert_eq!(bob.find("main", ".", 100).await.unwrap(), vec!["docs/guide.md", "services/api/main.rs"]);
    let seed_cs = bob.changeset(&seed.to_hex()).await.unwrap();
    assert!(seed_cs.redacted, "seed touched unreadable paths");
    assert!(seed_cs.changeset.changes.iter().all(|c| !c.path.starts_with("secrets") && !c.path.contains("billing")));
    let diff = bob.diff(&seed.to_hex(), "main").await.unwrap();
    assert!(diff.is_empty());
    assert!(bob.log("main", "secrets", 10).await.is_err());
    // Raw object reads by id (e.g. the root tree, which lists secret ids) are denied.
    let root_tree = seed_cs.root_tree;
    let err = bob.objects().get(&root_tree).await.unwrap_err().to_string();
    assert!(err.contains("path-scoped"), "{err}");
    assert!(alice.objects().get(&root_tree).await.unwrap().is_some());

    // Bob still works normally inside his scope: commit (uploads without existence
    // probes), land, and sync alice's changes (secrets stay out).
    write(&pb, "services/api/main.rs", "api v2 by bob\n");
    commit_land(&mut wb, bob.as_ref(), "bob edits api").await;
    sync::run(alice.as_ref(), &mut wa, None).await.unwrap();
    assert_eq!(read(&pa, "services/api/main.rs"), "api v2 by bob\n");
    write(&pa, "docs/guide.md", "guide v2\n");
    write(&pa, "secrets/prod.key", "ROTATED SECRET\n");
    commit_land(&mut wa, alice.as_ref(), "alice edits docs and secrets").await;
    sync::run(bob.as_ref(), &mut wb, None).await.unwrap();
    assert_eq!(read(&pb, "docs/guide.md"), "guide v2\n");
    assert!(!pb.join("secrets").exists());
    let log = bob.log("main", "", 10).await.unwrap();
    assert!(log.iter().all(|c| c.changeset.changes.iter().all(|ch| !ch.path.starts_with("secrets"))));
    // Bob's log omits changesets that only touched what he cannot read.
    write(&pa, "secrets/other.key", "x\n");
    let hidden = commit_land(&mut wa, alice.as_ref(), "secrets only").await.landed;
    assert!(bob.log("main", "", 10).await.unwrap().iter().all(|c| c.id != hidden));

    // Without read rules everyone keeps full access (backward compatible).
    let open = sapwood_adapters::acl::RulesPolicy::parse(r#"{"default":{"write":["*"],"refs":["*"]}}"#).unwrap();
    assert!(sapwood_core::acl::AccessPolicy::read_scope(&open, &sapwood_core::acl::Principal::anonymous()).is_full());
}

#[tokio::test(flavor = "multi_thread")]
async fn daemon_prefetch_serves_without_upstream() {
    use sapwood_server::features::prefetch::Prefetcher;

    let c = start().await;
    let r = remote(&c);
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("w");
    let mut ws = clone_into(&c, &p, &[]).await;
    for i in 0..20 {
        write(&p, &format!("services/api/f{i}.rs"), &format!("api {i}\n"));
        write(&p, &format!("services/web/f{i}.ts"), &format!("web {i}\n"));
    }
    std::fs::write(p.join("services/api/model.bin"), noise(10 << 20, 9)).unwrap(); // chunked
    commit_land(&mut ws, r.as_ref(), "seed").await;

    let tier_flags: Arc<dyn FeatureFlags> = Arc::new(SpecFlags::all_on());
    let (cache_url, stop_cache) = spawn(
        tier_routes(config::cache_store(&c.server, None, 64 << 20).unwrap(), tier_flags.clone()),
        Arc::new(AllowAll),
    )
    .await;
    let ddir = tempfile::tempdir().unwrap();
    let daemon = config::daemon_store(&cache_url, None, ddir.path().to_str().unwrap(), 16 << 20).unwrap();
    let prefetcher = Prefetcher::new(&c.server, None, daemon.clone(), tier_flags.clone()).unwrap();
    let stats = prefetcher.run_once("main", &["services/api".into()]).await.unwrap();
    assert_eq!(stats.files, 21, "{stats:?}");
    assert!(stats.objects > 21 + 3, "trees, files and large-file chunks: {stats:?}");

    // Cut the daemon off from its upstream: prefetched content is served from its disk.
    let _ = stop_cache.send(());
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let (daemon_url, _d) = spawn(tier_routes(daemon, tier_flags.clone()), Arc::new(AllowAll)).await;
    let via: Arc<dyn Remote> = Arc::new(GrpcRemote::connect(&c.server, Some(&daemon_url), None).unwrap());
    let cfg = Config {
        server: c.server.clone(),
        objects: Some(daemon_url.clone()),
        ref_name: "main".into(),
        sparse: vec!["services/api".into()],
    };
    let d2 = tempfile::tempdir().unwrap();
    let ws2 = clone::run(via.as_ref(), &flags(), &d2.path().join("w"), cfg).await.unwrap();
    assert_eq!(ws2.index.len(), 21);
    assert!(std::fs::read(d2.path().join("w/services/api/model.bin")).unwrap() == noise(10 << 20, 9));
    // Paths outside the prefetch were never fetched, so they are unavailable offline.
    let web_cfg = Config { sparse: vec!["services/web".into()], ..ws2.config.clone() };
    let d3 = tempfile::tempdir().unwrap();
    assert!(clone::run(via.as_ref(), &flags(), &d3.path().join("w"), web_cfg).await.is_err());

    // Flag off: prefetch refuses.
    let off = Prefetcher::new(
        &c.server,
        None,
        Arc::new(sapwood_adapters::memory::MemoryStore::new()),
        Arc::new(SpecFlags::parse("daemon-prefetch=off").unwrap()),
    )
    .unwrap();
    assert!(off.run_once("main", &[]).await.unwrap_err().to_string().contains("daemon-prefetch"));
}

/// Needs /dev/fuse and fusermount3 (or root); CI sets SAPWOOD_FUSE_TEST=1 on Linux.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn mounted_revision_reads_lazily() {
    use std::os::unix::fs::PermissionsExt;

    if std::env::var_os("SAPWOOD_FUSE_TEST").is_none() {
        eprintln!("skipping: set SAPWOOD_FUSE_TEST=1 to run the FUSE test");
        return;
    }
    let c = start().await;
    let r = remote(&c);
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("w");
    let mut ws = clone_into(&c, &p, &[]).await;
    write(&p, "src/lib.rs", "pub fn answer() -> u32 { 42 }\n");
    write(&p, "tools/run.sh", "#!/bin/sh\necho hi\n");
    std::fs::set_permissions(p.join("tools/run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("../src/lib.rs", p.join("tools/lib")).unwrap();
    let big = noise(20 << 20, 4);
    std::fs::create_dir_all(p.join("assets")).unwrap();
    std::fs::write(p.join("assets/model.bin"), &big).unwrap();
    commit_land(&mut ws, r.as_ref(), "seed").await;

    let root = r.changeset("main").await.unwrap().root_tree;
    let tree = Arc::new(sap::mount::VirtualTree::new(r.objects(), tokio::runtime::Handle::current(), root, 64 << 20));
    let mnt = tempfile::tempdir().unwrap();
    let session = sap::mount::fuse::mount(tree, mnt.path()).unwrap();
    let m = mnt.path().to_owned();
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Seek, SeekFrom};
        let mut top: Vec<_> = std::fs::read_dir(&m).unwrap().map(|e| e.unwrap().file_name()).collect();
        top.sort();
        assert_eq!(top, ["assets", "src", "tools"]);
        assert_eq!(std::fs::read_to_string(m.join("src/lib.rs")).unwrap(), "pub fn answer() -> u32 { 42 }\n");
        assert_eq!(std::fs::metadata(m.join("tools/run.sh")).unwrap().permissions().mode() & 0o111, 0o111);
        assert_eq!(std::fs::read_link(m.join("tools/lib")).unwrap(), std::path::Path::new("../src/lib.rs"));
        assert_eq!(std::fs::read_to_string(m.join("tools/lib")).unwrap(), "pub fn answer() -> u32 { 42 }\n");
        assert_eq!(std::fs::metadata(m.join("assets/model.bin")).unwrap().len(), big.len() as u64);
        let mut f = std::fs::File::open(m.join("assets/model.bin")).unwrap();
        f.seek(SeekFrom::Start(13_000_000)).unwrap();
        let mut buf = vec![0u8; 100_000];
        f.read_exact(&mut buf).unwrap();
        assert_eq!(buf, &big[13_000_000..13_100_000]);
        assert_eq!(std::fs::read(m.join("assets/model.bin")).unwrap(), big);
        assert!(std::fs::write(m.join("src/new.rs"), "x").is_err(), "mount is read-only");
        assert!(std::fs::metadata(m.join("missing")).is_err());
    })
    .await
    .unwrap();
    session.umount_and_join().unwrap();
}

/// Minimal HTTP land hook: approves only changesets in `approved`, records every request.
struct Hook {
    url: String,
    approved: Arc<std::sync::Mutex<Vec<String>>>,
    seen: Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>,
}

async fn land_hook() -> Hook {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/check", listener.local_addr().unwrap());
    let approved = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (a, s) = (approved.clone(), seen.clone());
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let (head, body) = loop {
                let n = sock.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..i]).to_lowercase();
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .map_or(0, |v| v.trim().parse().unwrap());
                    while buf.len() < i + 4 + len {
                        let n = sock.read(&mut chunk).await.unwrap();
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    break (head, buf[i + 4..i + 4 + len].to_vec());
                }
            };
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let auth = head.lines().find_map(|l| l.strip_prefix("authorization:")).unwrap_or("").trim().to_owned();
            s.lock().unwrap().push((auth.clone(), json.clone()));
            let (status, msg) = if auth != "bearer hook-secret" {
                ("401 Unauthorized", "")
            } else if a.lock().unwrap().contains(&json["changeset"].as_str().unwrap().to_owned()) {
                ("200 OK", "ok")
            } else {
                ("403 Forbidden", "needs an approving review from @owners")
            };
            let resp = format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{msg}", msg.len());
            sock.write_all(resp.as_bytes()).await.unwrap();
        }
    });
    Hook { url, approved, seen }
}

#[tokio::test(flavor = "multi_thread")]
async fn land_hook_gates_landing() {
    use std::time::Duration;

    use sapwood_server::land_hook::WebhookCheck;

    let hook = land_hook().await;
    let check = WebhookCheck::new(&hook.url, Some("hook-secret".into()), Duration::from_secs(5)).unwrap();
    let c = start_checked(SpecFlags::all_on(), Arc::new(AllowAll), "memory", Arc::new(AllowAllPolicy), Arc::new(check))
        .await;
    let r = remote(&c);
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("w");
    let mut ws = clone_into(&c, &p, &[]).await;
    write(&p, "src/lib.rs", "pub fn f() {}\n");
    write(&p, "README.md", "hi\n");
    let committed = commit::run(r.as_ref(), &mut ws, "add lib", "Ada <ada@example.com>", &[]).await.unwrap().unwrap();

    // Not yet approved: refused with the hook's reason, and the ref does not move.
    let err = land::run(r.as_ref(), &mut ws).await.unwrap_err().to_string();
    assert!(err.contains("needs an approving review from @owners"), "{err}");
    assert!(r.changeset("main").await.is_err());
    let (auth, body) = hook.seen.lock().unwrap()[0].clone();
    assert_eq!(auth, "bearer hook-secret");
    assert_eq!(body["ref"], "main");
    assert_eq!(body["author"], "Ada <ada@example.com>");
    assert_eq!(body["message"], "add lib");
    assert_eq!(body["principal"], "anonymous");
    assert_eq!(body["parent"], serde_json::Value::Null);
    assert_eq!(body["paths"], serde_json::json!(["README.md", "src/lib.rs"]));
    assert_eq!(body["paths_truncated"], false);

    // The review approves this exact changeset: the same land now succeeds.
    let id = body["changeset"].as_str().unwrap().to_owned();
    assert_eq!(id, committed.id.to_hex());
    hook.approved.lock().unwrap().push(id);
    land::run(r.as_ref(), &mut ws).await.unwrap();
    assert_eq!(r.changeset("main").await.unwrap().id, committed.id);

    // Wrong token: the hook answers 401 with no body.
    let bad = WebhookCheck::new(&hook.url, Some("wrong".into()), Duration::from_secs(5)).unwrap();
    let proposal = sapwood_core::ports::LandProposal {
        principal: "p".into(),
        ref_name: "main".into(),
        changeset: committed.id,
        parent: None,
        author: String::new(),
        message: String::new(),
        paths: vec![],
    };
    let e = bad.check(&proposal).await.unwrap_err().to_string();
    assert!(e.contains("HTTP 401"), "{e}");

    // Unreachable hook: fail closed, without leaking the URL.
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead = format!("http://{}/check", closed.local_addr().unwrap());
    drop(closed);
    let e =
        WebhookCheck::new(&dead, None, Duration::from_secs(2)).unwrap().check(&proposal).await.unwrap_err().to_string();
    assert!(e.contains("land check unavailable") && !e.contains("127.0.0.1"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn land_hook_skipped_when_flag_off() {
    use std::time::Duration;

    use sapwood_server::land_hook::WebhookCheck;

    let hook = land_hook().await;
    let check = WebhookCheck::new(&hook.url, Some("hook-secret".into()), Duration::from_secs(5)).unwrap();
    let flags = SpecFlags::parse("land-checks=off").unwrap();
    let c = start_checked(flags, Arc::new(AllowAll), "memory", Arc::new(AllowAllPolicy), Arc::new(check)).await;
    let r = remote(&c);
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("w");
    let mut ws = clone_into(&c, &p, &[]).await;
    write(&p, "a.txt", "a\n");
    commit_land(&mut ws, r.as_ref(), "unchecked").await;
    assert!(hook.seen.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn compaction_leaves_chunked_large_files_intact() {
    use sapwood_server::features::compact::Compactor;

    let storage = config::open_storage("memory", "metadata").await.unwrap();
    let flags_on: Arc<dyn FeatureFlags> = Arc::new(SpecFlags::all_on());
    let (server, _stop) = spawn(
        authority_routes(storage.repo.clone(), flags_on.clone(), Arc::new(AllowAllPolicy), Arc::new(NoChecks)),
        Arc::new(AllowAll),
    )
    .await;
    let c = Cluster { server, _shutdown: vec![] };
    let r = remote(&c);
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("w");
    let mut ws = clone_into(&c, &p, &[]).await;
    let v1 = noise(12 << 20, 5);
    let mut v2 = v1.clone();
    v2.extend_from_slice(b"tail");
    std::fs::write(p.join("big.bin"), &v1).unwrap();
    commit_land(&mut ws, r.as_ref(), "v1").await;
    std::fs::write(p.join("big.bin"), &v2).unwrap();
    let head = commit_land(&mut ws, r.as_ref(), "v2").await.landed;
    let stats = Compactor::new(storage.repo.clone(), storage.raw_objects.clone(), flags_on).run(false).await.unwrap();
    assert!(stats.skipped_chunked >= 1, "{stats:?}");
    assert!(browse::cat(r.as_ref(), &head.to_hex(), "big.bin").await.unwrap() == v2);
}

#[tokio::test(flavor = "multi_thread")]
async fn compaction_never_converts_a_base_that_others_depend_on() {
    use sapwood_core::delta;
    use sapwood_server::features::compact::Compactor;

    // X is first a brand-new file at a/, then the base for Y at a/; later X reappears as
    // the *new* version of b/ (a copy/revert). Converting X then would lengthen Y's chain.
    let storage = config::open_storage("memory", "metadata").await.unwrap();
    let flags: Arc<dyn FeatureFlags> = Arc::new(SpecFlags::all_on());
    let (server, _stop) = spawn(
        authority_routes(storage.repo.clone(), flags.clone(), Arc::new(AllowAllPolicy), Arc::new(NoChecks)),
        Arc::new(AllowAll),
    )
    .await;
    let c = Cluster { server, _shutdown: vec![] };
    let r = remote(&c);
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("w");
    let mut ws = clone_into(&c, &p, &[]).await;
    let x: String = (0..2000).map(|i| format!("line {i} of x\n")).collect();
    let y = x.replace("line 7 of x", "line 7 of y");
    let w: String = x.replace("line 9 of x", "line 9 of w");
    write(&p, "a/f.txt", &x);
    write(&p, "b/f.txt", &w);
    commit_land(&mut ws, r.as_ref(), "x and w").await;
    write(&p, "a/f.txt", &y);
    commit_land(&mut ws, r.as_ref(), "y based on x").await;
    write(&p, "b/f.txt", &x);
    commit_land(&mut ws, r.as_ref(), "b becomes x").await;

    let stats = Compactor::new(storage.repo.clone(), storage.raw_objects.clone(), flags).run(false).await.unwrap();
    assert_eq!(stats.converted, 1, "{stats:?}");
    assert_eq!(stats.skipped_pinned, 1, "{stats:?}");
    let x_id = sapwood_core::Id::hash(sapwood_core::object::Kind::Blob as u8, x.as_bytes());
    let x_payload = storage.raw_objects.get(&x_id).await.unwrap().unwrap();
    assert!(!delta::is_delta(&x_payload), "pinned base must stay whole");
    assert_eq!(browse::cat(r.as_ref(), "main", "a/f.txt").await.unwrap(), y.as_bytes());
    assert_eq!(browse::cat(r.as_ref(), "main", "b/f.txt").await.unwrap(), x.as_bytes());
}

#[tokio::test(flavor = "multi_thread")]
async fn compaction_preserves_every_version_and_shrinks_storage() {
    use sapwood_server::features::compact::{Compactor, LEASE_REF};

    let data = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", data.path().join("meta.db").display());
    let storage = config::open_storage(&url, "metadata").await.unwrap();
    let flags: Arc<dyn FeatureFlags> = Arc::new(SpecFlags::all_on());
    let (server, _stop) = spawn(
        authority_routes(storage.repo.clone(), flags.clone(), Arc::new(AllowAllPolicy), Arc::new(NoChecks)),
        Arc::new(AllowAll),
    )
    .await;
    let c = Cluster { server, _shutdown: vec![] };
    let r = remote(&c);
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("w");
    let mut ws = clone_into(&c, &p, &[]).await;

    // Realistic directory sizes so tree objects are worth delta-encoding.
    for i in 0..300 {
        write(&p, &format!("src/module_{i:03}.rs"), &format!("pub fn f{i}() {{}}\n"));
        write(&p, &format!("top_level_{i:03}.md"), &format!("doc {i}\n"));
    }
    // 60 versions (more than the chain cap of 50) of a ~60 KB source file, each a small
    // edit; then a revert to v1 and a
    // file -> directory flip, to exercise the cycle and replacement edge cases.
    let base_text: String = (0..1500).map(|i| format!("fn function_{i}(x: u64) -> u64 {{ x * {i} }}\n")).collect();
    let mut expected = Vec::new();
    for v in 0..60 {
        let text = base_text.replacen(&format!("function_{v}("), &format!("renamed_{v}("), 1) + &format!("// v{v}\n");
        write(&p, "src/big.rs", &text);
        let id = commit_land(&mut ws, r.as_ref(), &format!("v{v}")).await.landed;
        expected.push((id, "src/big.rs".to_owned(), text));
    }
    let v1 = expected[1].2.clone();
    write(&p, "src/big.rs", &v1);
    let id = commit_land(&mut ws, r.as_ref(), "revert to v1").await.landed;
    expected.push((id, "src/big.rs".into(), v1.clone()));
    std::fs::remove_file(p.join("src/big.rs")).unwrap();
    write(&p, "src/big.rs/inner.rs", &v1);
    let id = commit_land(&mut ws, r.as_ref(), "file becomes dir").await.landed;
    expected.push((id, "src/big.rs/inner.rs".into(), v1.clone()));

    let compactor = Compactor::new(storage.repo.clone(), storage.raw_objects.clone(), flags.clone());
    // A held lease blocks a second compactor.
    let lease = Id::hash(0xEE, b"someone else");
    assert!(storage.repo.refs.compare_and_swap(LEASE_REF, None, lease).await.unwrap());
    assert!(compactor.run(false).await.unwrap_err().to_string().contains("lease"));
    // System refs are invisible to users.
    assert!(r.list_refs().await.unwrap().iter().all(|(n, _)| n == "main"));

    let stats = compactor.run(true).await.unwrap(); // take over the stale lease
    assert!(stats.converted >= 55, "{stats:?}");
    assert!(stats.trees_converted >= 30, "root and src/ trees are delta-encoded too: {stats:?}");
    assert!(stats.bytes_after * 20 < stats.bytes_before, "expected >20x: {stats:?}");
    assert!(stats.skipped_depth >= 1, "chains are capped: {stats:?}");

    // Every historical version reads back byte-identical, through the live server.
    for (rev, path, text) in &expected {
        let got = browse::cat(r.as_ref(), &rev.to_hex(), path).await.unwrap();
        assert_eq!(&got, text.as_bytes(), "{path} at {}", rev.short());
    }
    // Fresh clone and blame after compaction.
    let d2 = tempfile::tempdir().unwrap();
    let ws2 = clone_into(&c, &d2.path().join("w"), &[]).await;
    assert_eq!(ws2.index.len(), 601);
    assert_eq!(r.blame("main", "src/big.rs/inner.rs").await.unwrap().len(), 1501);

    // Idempotent; lease released.
    let again = compactor.run(false).await.unwrap();
    assert_eq!(again.converted, 0, "{again:?}");

    // Flag off => compaction refused.
    let off = Compactor::new(
        storage.repo.clone(),
        storage.raw_objects.clone(),
        Arc::new(SpecFlags::parse("delta-storage=off").unwrap()),
    );
    assert!(off.run(false).await.unwrap_err().to_string().contains("delta-storage"));
}
