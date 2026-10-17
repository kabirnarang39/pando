//! `pandod backup`, then the documented restore (copy back while stopped): the registry and
//! every repository come back whole.

use pando_core::object::{Changeset, FileChange, Mode};
use pando_core::tree;
use pando_server::backup::{MANIFEST_FILE, REGISTRY_FILE, backup};
use pando_server::config::open_repo;
use pando_server::directory::TokenRec;
use pando_server::hosting::{Registry, RepoKey, RepoRecord, Templates, now_ms};

#[tokio::test(flavor = "multi_thread")]
async fn backup_wipe_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let live = tmp.path().join("live");
    let platform = format!("sqlite://{}", live.join("platform.db").display());
    let templates =
        Templates { metadata: format!("sqlite://{}/repos/{{repo_id}}.db", live.display()), objects: "metadata".into() };
    let key = RepoKey::parse("lumen/fleet").unwrap();

    // A running platform: a repository with a commit, and a token.
    let reg = Registry::open(&platform).unwrap();
    reg.create(&RepoRecord {
        key: key.clone(),
        description: "fleet".into(),
        visibility: 0,
        default_branch: "main".into(),
        created_at_ms: now_ms(),
        archived: false,
        topics: vec![],
    })
    .unwrap();
    reg.ensure_account("dev", "").unwrap();
    let (_, secret) = reg
        .create_token(TokenRec {
            id: String::new(),
            account: "dev".into(),
            name: "t".into(),
            acts_as: "dev".into(),
            repositories: vec![],
            max_role: pando_proto::Role::Write,
            created_at_ms: now_ms(),
            expires_at_ms: now_ms() + 86_400_000,
            last_used_at_ms: 0,
        })
        .unwrap();
    let repo = open_repo(&templates.render(&key).0, "metadata").await.unwrap();
    let blob = tree::put_blob(repo.objects.as_ref(), b"rate = 20\n").await.unwrap();
    let id = repo
        .create_changeset(Changeset {
            parents: vec![],
            changes: vec![FileChange::upsert("tax.rs", Mode::File, blob)],
            author: "dev".into(),
            message: "m".into(),
            timestamp_ms: 1,
        })
        .await
        .unwrap();
    assert!(repo.refs.compare_and_swap("main", None, id).await.unwrap());

    // Backed up while serving.
    let bak = tmp.path().join("bak");
    let s = backup(&platform, &templates, None, bak.to_str().unwrap()).await.unwrap();
    assert_eq!(s.repositories, 1);
    assert!(backup(&platform, &templates, None, bak.to_str().unwrap()).await.is_err(), "never overwrites a backup");

    // The disk is lost.
    drop((reg, repo));
    std::fs::remove_dir_all(&live).unwrap();

    // Restore, as docs/deploy.md describes: with pandod stopped, copy platform.db to the
    // --platform path and each repository's `file` to its `metadata` path.
    std::fs::create_dir_all(live.join("repos")).unwrap();
    std::fs::copy(bak.join(REGISTRY_FILE), live.join("platform.db")).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(bak.join(MANIFEST_FILE)).unwrap()).unwrap();
    for r in manifest["repositories"].as_array().unwrap() {
        let dst = r["metadata"].as_str().unwrap().strip_prefix("sqlite://").unwrap();
        std::fs::copy(bak.join(r["file"].as_str().unwrap()), dst).unwrap();
    }

    let reg = Registry::open(&platform).unwrap();
    assert_eq!(reg.get(&key).unwrap().unwrap().description, "fleet");
    assert_eq!(reg.use_token(&secret).unwrap().unwrap().account, "dev", "tokens survive");
    let repo = open_repo(&templates.render(&key).0, "metadata").await.unwrap();
    let main = repo.refs.get("main").await.unwrap().unwrap();
    assert_eq!(main, id);
    let (_, file) = repo.lookup(main, "tax.rs").await.unwrap().unwrap();
    let body = tree::read_object(repo.objects.as_ref(), &file, pando_core::object::Kind::Blob).await.unwrap();
    assert_eq!(body, b"rate = 20\n");
}
