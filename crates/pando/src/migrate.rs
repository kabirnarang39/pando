//! `pando migrate`: move a repository from any git host to Pando in one command (ADR-028).
//! Full history comes from git itself; from GitHub, open pull requests become change requests
//! and issues come across with their discussion, labels and state. The old host can be kept
//! current afterwards with `pando mirror`, so the move is reversible.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use pando::features::git;
use pando::remote::GrpcRemote;
use pando_core::flags::FeatureFlags;
use pando_proto::{
    AddIssueCommentRequest, CreateChangeRequestRequest, CreateIssueRequest, CreateRepositoryRequest, IssueRef,
    IssueState, RepositoryRef, UpdateIssueRequest,
};
use serde_json::Value;

/// A scratch directory removed when dropped.
struct TempDir(std::path::PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn hex_of(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub struct Migration {
    /// Where the repository is now: any URL git can clone.
    pub from: String,
    /// Where it goes: SERVER/OWNER/NAME.
    pub to: String,
    /// A GitHub token, for private repositories and for pull requests and issues.
    pub github_token: Option<String>,
    pub issues: bool,
    pub pull_requests: bool,
    /// GitHub's API: https://api.github.com, or GitHub Enterprise Server's
    /// (https://github.example.com/api/v3). Setting it treats `from` as a GitHub repository.
    pub github_api: Option<String>,
}

/// `owner/repo` from the last two path segments of any repository URL.
fn last_two(url: &str) -> Option<(String, String)> {
    let path = url.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = path.rsplit(['/', ':', '\\']);
    let repo = parts.next()?;
    let owner = parts.next()?;
    (!owner.is_empty() && !repo.is_empty()).then(|| (owner.to_owned(), repo.to_owned()))
}

/// `owner/repo` if `url` is a GitHub repository.
pub fn github_repo(url: &str) -> Option<(String, String)> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    let (owner, repo) = rest.split_once('/')?;
    (!owner.is_empty() && !repo.is_empty() && !repo.contains('/')).then(|| (owner.to_owned(), repo.to_owned()))
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!("git {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

struct GitHub {
    http: reqwest::Client,
    api: String,
    repo: String,
    token: Option<String>,
}

impl GitHub {
    /// Every page of a list endpoint.
    async fn all(&self, path: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for page in 1.. {
            let sep = if path.contains('?') { '&' } else { '?' };
            let url = format!("{}/repos/{}/{path}{sep}per_page=100&page={page}", self.api, self.repo);
            let mut req = self
                .http
                .get(&url)
                .header("accept", "application/vnd.github+json")
                .header("user-agent", "pando-migrate");
            if let Some(t) = &self.token {
                req = req.bearer_auth(t);
            }
            let resp = req.send().await.with_context(|| format!("calling {url}"))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let hint = if status.as_u16() == 403 || status.as_u16() == 429 {
                    " (rate limited: pass --github-token or set GITHUB_TOKEN)"
                } else {
                    ""
                };
                bail!("GitHub answered {status} for {path}{hint}");
            }
            let items: Vec<Value> = resp.json().await?;
            let n = items.len();
            out.extend(items);
            if n < 100 {
                break;
            }
        }
        Ok(out)
    }

    async fn one(&self, path: &str) -> Result<Value> {
        let mut req =
            self.http.get(format!("{}/repos/{}{path}", self.api, self.repo)).header("user-agent", "pando-migrate");
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        Ok(req.send().await?.error_for_status()?.json().await?)
    }
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_owned()
}

fn attribution(item: &Value, what: &str) -> String {
    format!("\n\n---\n_Moved from {} where @{} opened this {what}._", s(item, "html_url"), s(&item["user"], "login"))
}

pub async fn run(m: Migration, token: Option<&str>, flags: &dyn FeatureFlags) -> Result<()> {
    let (base, repo) = pando_proto::split_repo_url(&m.to);
    let repo = repo.context("give the destination as SERVER/OWNER/NAME")?;
    let (owner, name) = repo.split_once('/').context("give the destination as SERVER/OWNER/NAME")?;
    let server = GrpcRemote::connect(&base, None, token)?;
    let found = match &m.github_api {
        Some(_) => last_two(&m.from),
        None => github_repo(&m.from),
    };
    let api = m.github_api.clone().unwrap_or_else(|| "https://api.github.com".into()).trim_end_matches('/').to_owned();
    let gh = found.map(|(o, r)| GitHub {
        api,
        http: {
            let _ = rustls::crypto::ring::default_provider().install_default();
            reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build().expect("an HTTP client")
        },
        repo: format!("{o}/{r}"),
        token: m.github_token.clone(),
    });

    // 1. The repository, described as it was.
    let description = match &gh {
        Some(g) => g.one("").await.map(|v| s(&v, "description")).unwrap_or_default(),
        None => String::new(),
    };
    match server
        .platform()
        .create_repository(CreateRepositoryRequest {
            owner: owner.into(),
            name: name.into(),
            description,
            ..Default::default()
        })
        .await
    {
        Ok(_) => println!("created {owner}/{name}"),
        Err(e) if e.code() == tonic::Code::AlreadyExists => println!("{owner}/{name} exists; adding to it"),
        Err(e) => bail!("creating {owner}/{name}: {}", e.message()),
    }

    // 2. History: a bare mirror clone, then every branch.
    let mut nonce = [0u8; 6];
    nonce
        .iter_mut()
        .zip(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos().to_le_bytes())
        .for_each(|(a, b)| *a = b);
    let work = TempDir(std::env::temp_dir().join(format!("pando-migrate-{}-{}", std::process::id(), hex_of(&nonce))));
    let dir = work.0.join("mirror.git");
    println!("cloning {} ...", m.from);
    let mut clone = Command::new("git");
    clone.env("GIT_TERMINAL_PROMPT", "0");
    if let (Some(_), Some(t)) = (&gh, &m.github_token)
        && let Some(host) = m.from.strip_prefix("https://").and_then(|r| r.split('/').next())
    {
        // The token goes only to the repository's own host, and through git's environment
        // rather than its command line, which other users of the machine can see.
        clone.env("GIT_CONFIG_COUNT", "1");
        clone.env("GIT_CONFIG_KEY_0", format!("http.https://{host}/.extraheader"));
        clone.env("GIT_CONFIG_VALUE_0", format!("AUTHORIZATION: bearer {t}"));
    }
    let status =
        clone.args(["clone", "--mirror", "--quiet", &m.from]).arg(&dir).status().context("running git clone")?;
    if !status.success() {
        bail!("git could not clone {}", m.from);
    }
    let default = git(&dir, &["symbolic-ref", "--short", "HEAD"]).unwrap_or_else(|_| "main".into());
    let remote = GrpcRemote::connect(&format!("{base}/{owner}/{name}"), None, token)?;
    let branches: Vec<String> =
        git(&dir, &["for-each-ref", "--format=%(refname:short)", "refs/heads"])?.lines().map(str::to_owned).collect();
    let mut ordered = vec![default.clone()];
    ordered.extend(branches.into_iter().filter(|b| b != &default));
    for b in &ordered {
        let out = git::import(&remote, flags, &dir, &format!("refs/heads/{b}"), b, false)
            .await
            .with_context(|| format!("importing {b}"))?;
        println!("  {b}: {} commits ({} files uploaded)", out.commits, out.uploaded);
    }

    let Some(gh) = gh else {
        println!("done. Clone it: pando clone {base}/{owner}/{name}");
        return Ok(());
    };
    let dest = || Some(RepositoryRef { owner: owner.into(), name: name.into() });

    // 3. Open pull requests become change requests from github/pr-N branches.
    if m.pull_requests {
        let prs = gh.all("pulls?state=open").await?;
        for pr in &prs {
            let n = pr["number"].as_u64().unwrap_or(0);
            let branch = format!("github/pr-{n}");
            let head = format!("refs/pull/{n}/head");
            if git(&dir, &["rev-parse", "--verify", "-q", &head]).is_err() {
                eprintln!("  pull request #{n}: its commits are not in the repository; skipped");
                continue;
            }
            if let Err(e) = git::import(&remote, flags, &dir, &head, &branch, false).await {
                eprintln!("  pull request #{n}: {e:#}; skipped");
                continue;
            }
            let target = s(&pr["base"], "ref");
            let target = if ordered.contains(&target) { target } else { default.clone() };
            let created = server
                .reviews()
                .create_change_request(CreateChangeRequestRequest {
                    repository: dest(),
                    source: branch,
                    target,
                    title: s(pr, "title"),
                    body: format!("{}{}", s(pr, "body"), attribution(pr, "pull request")),
                    draft: pr["draft"].as_bool().unwrap_or(false),
                    reviewers: vec![],
                })
                .await;
            match created {
                Ok(c) => println!("  pull request #{n} -> change request #{}", c.into_inner().number),
                Err(e) => eprintln!("  pull request #{n}: {}; skipped", e.message()),
            }
        }
    }

    // 4. Issues, oldest first, with their comments and state.
    if m.issues {
        let items = gh.all("issues?state=all&sort=created&direction=asc").await?;
        let issues: Vec<&Value> = items.iter().filter(|i| i.get("pull_request").is_none()).collect();
        println!("moving {} issues ...", issues.len());
        let mut issues_client = server.issues();
        for i in issues {
            let labels = i["labels"]
                .as_array()
                .map(|l| l.iter().map(|x| s(x, "name").replace(',', " ")).collect())
                .unwrap_or_default();
            let created = issues_client
                .create_issue(CreateIssueRequest {
                    repository: dest(),
                    title: s(i, "title"),
                    body: format!("{}{}", s(i, "body"), attribution(i, "issue")),
                    labels,
                    assignees: vec![],
                })
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.message()))?
                .into_inner();
            let r = IssueRef { repository: dest(), number: created.number };
            if i["comments"].as_u64().unwrap_or(0) > 0 {
                let n = i["number"].as_u64().unwrap_or(0);
                for c in gh.all(&format!("issues/{n}/comments")).await? {
                    let body = format!("**@{}** wrote on GitHub:\n\n{}", s(&c["user"], "login"), s(&c, "body"));
                    issues_client
                        .add_issue_comment(AddIssueCommentRequest { r#ref: Some(r.clone()), body })
                        .await
                        .map_err(|e| anyhow::anyhow!("{}", e.message()))?;
                }
            }
            if s(i, "state") == "closed" {
                let state =
                    if s(i, "state_reason") == "not_planned" { IssueState::NotPlanned } else { IssueState::Completed };
                issues_client
                    .update_issue(UpdateIssueRequest {
                        r#ref: Some(r),
                        fields: vec!["state".into()],
                        state: state as i32,
                        ..Default::default()
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("{}", e.message()))?;
            }
        }
    }

    println!("done. Clone it: pando clone {base}/{owner}/{name}");
    println!(
        "to keep {} current while you move over: pando mirror --to {} --server {base}/{owner}/{name} --every 10",
        gh.repo, m.from
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_github_urls() {
        for url in [
            "https://github.com/acme/fleet",
            "https://github.com/acme/fleet.git",
            "git@github.com:acme/fleet.git",
            "ssh://git@github.com/acme/fleet",
        ] {
            assert_eq!(github_repo(url), Some(("acme".into(), "fleet".into())), "{url}");
        }
        assert_eq!(github_repo("https://gitlab.com/acme/fleet"), None);
        assert_eq!(github_repo("https://github.com/acme"), None);
        assert_eq!(last_two("https://github.example.com/acme/fleet.git"), Some(("acme".into(), "fleet".into())));
        assert_eq!(last_two("/srv/git/acme/fleet"), Some(("acme".into(), "fleet".into())));
        assert_eq!(last_two(r"C:\git\acme\fleet"), Some(("acme".into(), "fleet".into())));
    }
}
