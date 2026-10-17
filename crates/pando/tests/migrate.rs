//! `pando migrate` (ADR-028): full git history and branches, and from GitHub (here a fake
//! GitHub API), open pull requests as change requests and issues with comments and state.

use std::path::Path;
use std::process::Command;

use pando::remote::{GrpcRemote, Remote};
use pando_proto::{ChangeRequestRef, IssueRef, IssueState, ListChangeRequestsRequest, RepositoryRef};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod common;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Ada")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Answers GitHub API paths from a fixed table; unknown paths get 404.
async fn fake_github(routes: Vec<(&'static str, serde_json::Value)>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let routes = routes.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let path = path.split('?').next().unwrap();
                let page2 = req.split_whitespace().nth(1).unwrap_or("").contains("page=2");
                let body =
                    routes.iter().find(|(p, _)| *p == path).map(|(_, v)| if page2 { json!([]) } else { v.clone() });
                let (status, body) = match body {
                    Some(b) => ("200 OK", b.to_string()),
                    None => ("404 Not Found", "{}".to_owned()),
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    url
}

#[tokio::test(flavor = "multi_thread")]
async fn migrates_history_pull_requests_and_issues() {
    // A git repository with history on two branches, a merge, and a pull request head.
    let src = tempfile::tempdir().unwrap();
    let s = src.path().join("acme").join("fleet");
    std::fs::create_dir_all(&s).unwrap();
    git(&s, &["init", "-q", "-b", "main"]);
    write(&s, "billing/tax.rs", "rate = 19\n");
    git(&s, &["add", "."]);
    git(&s, &["commit", "-qm", "Start billing"]);
    git(&s, &["checkout", "-qb", "refunds"]);
    write(&s, "billing/refund.rs", "window = 30\n");
    git(&s, &["add", "."]);
    git(&s, &["commit", "-qm", "Refund window"]);
    git(&s, &["checkout", "-q", "main"]);
    write(&s, "README.md", "Fleet\n");
    git(&s, &["add", "."]);
    git(&s, &["commit", "-qm", "README"]);
    git(&s, &["merge", "-q", "--no-edit", "refunds"]);
    git(&s, &["checkout", "-qb", "pr-head"]);
    write(&s, "billing/tax.rs", "rate = 20\n");
    git(&s, &["commit", "-qam", "Raise the rate"]);
    let pr_sha = git(&s, &["rev-parse", "HEAD"]);
    git(&s, &["checkout", "-q", "main"]);
    git(&s, &["branch", "-qD", "pr-head"]);
    git(&s, &["update-ref", "refs/pull/7/head", &pr_sha]);
    let main_tree = git(&s, &["rev-parse", "main^{tree}"]);

    let api = fake_github(vec![
        ("/repos/acme/fleet", json!({"description": "The product monorepo"})),
        ("/repos/acme/fleet/pulls", json!([{"number": 7, "title": "Raise the rate to 20", "body": "Finance asked.", "draft": false,
            "base": {"ref": "main"}, "html_url": "https://github.example.com/acme/fleet/pull/7", "user": {"login": "ada"}}])),
        ("/repos/acme/fleet/issues", json!([
            {"number": 3, "title": "Refunds are too short", "body": "45 days please", "state": "open", "comments": 1, "labels": [{"name": "billing"}],
             "html_url": "https://github.example.com/acme/fleet/issues/3", "user": {"login": "grace"}},
            {"number": 4, "title": "Old idea", "body": "", "state": "closed", "state_reason": "not_planned", "comments": 0, "labels": [],
             "html_url": "https://github.example.com/acme/fleet/issues/4", "user": {"login": "grace"}},
            {"number": 7, "title": "Raise the rate to 20", "pull_request": {}, "user": {"login": "ada"}}
        ])),
        ("/repos/acme/fleet/issues/3/comments", json!([{"body": "Agreed with legal.", "user": {"login": "linus"}}])),
    ])
    .await;

    let server = common::start().await;
    let out = Command::new(env!("CARGO_BIN_EXE_pando"))
        .args(["migrate", s.to_str().unwrap(), "--to", &format!("{}/acme/fleet", server.url), "--github-api", &api])
        .env("PANDO_TOKEN", "admin-token")
        .env_remove("GITHUB_TOKEN")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));

    // History: every commit, identical trees, every branch.
    let r = GrpcRemote::connect(&format!("{}/acme/fleet", server.url), None, Some("admin-token")).unwrap();
    assert_eq!(pando::features::browse::cat(&r, "main", "billing/refund.rs").await.unwrap(), b"window = 30\n");
    assert!(r.get_ref("refunds").await.unwrap().is_some(), "every branch comes across");
    let log = r.log("main", "", 100).await.unwrap();
    assert!(
        log.len() >= 3
            && log
                .iter()
                .any(|c| c.changeset.message.starts_with("Start billing") && c.changeset.author.contains("Ada")),
        "{stdout}"
    );
    let dst = tempfile::tempdir().unwrap();
    pando::features::git::export(&r, &pando_adapters::flags::SpecFlags::all_on(), "main", dst.path(), "main")
        .await
        .unwrap();
    assert_eq!(git(dst.path(), &["rev-parse", "main^{tree}"]), main_tree, "the tip tree is byte-identical");

    // The open pull request is a change request from its own branch.
    let repo = || Some(RepositoryRef { owner: "acme".into(), name: "fleet".into() });
    let crs = r
        .reviews()
        .list_change_requests(ListChangeRequestsRequest { repository: repo(), ..Default::default() })
        .await
        .unwrap()
        .into_inner();
    let cr = crs.change_requests.iter().find(|c| c.title == "Raise the rate to 20").expect("the pull request moved");
    assert_eq!(cr.source, "github/pr-7");
    assert!(cr.body.contains("Finance asked.") && cr.body.contains("@ada"));
    let got = r
        .reviews()
        .get_change_request(ChangeRequestRef { repository: repo(), number: cr.number })
        .await
        .unwrap()
        .into_inner();
    assert_eq!((got.files, got.additions, got.deletions), (1, 1, 1));

    // Issues, with discussion, labels and state; the pull request is not an issue.
    let mut issues = r.issues();
    let all = issues
        .list_issues(pando_proto::ListIssuesRequest { repository: repo(), states: vec![1, 2, 3], ..Default::default() })
        .await
        .unwrap()
        .into_inner()
        .issues;
    assert_eq!(all.len(), 2, "{all:?}");
    let open = all.iter().find(|i| i.title == "Refunds are too short").unwrap();
    assert_eq!(open.labels, vec!["billing".to_string()]);
    assert!(open.body.contains("@grace"));
    let comments = issues
        .list_issue_comments(IssueRef { repository: repo(), number: open.number })
        .await
        .unwrap()
        .into_inner()
        .comments;
    assert!(comments[0].body.contains("@linus") && comments[0].body.contains("Agreed with legal."));
    let closed = all.iter().find(|i| i.title == "Old idea").unwrap();
    assert_eq!(closed.state, IssueState::NotPlanned as i32);
}
