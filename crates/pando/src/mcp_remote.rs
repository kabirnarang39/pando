//! `pando mcp --http`: a remote Model Context Protocol endpoint for a whole Pando server, so an
//! agent anywhere (Claude, ChatGPT, Gemini, Cursor, your own) can read, change, review and land
//! code with nothing but a URL and a token: no clone, no workspace (ADR-026).
//!
//! It is a stateless gateway. Each request's `Authorization: Bearer` token is passed through to
//! the server, so the agent can do exactly what the token allows: every permission check,
//! token limit and audit record of the server applies, and the gateway holds no credentials.
//! MCP 2026-07-28 is stateless too (an older client's `initialize` is answered without a
//! session), so any number of gateways can run behind a load balancer.

use std::sync::Arc;

use axum::http::request::Parts;
use pando_core::object::{Changeset, FileChange, Kind, Mode, encode_object};
use pando_proto::{
    AddCommentRequest, ChangeRequest, ChangeRequestRef, ChangeRequestState, Check, CheckState,
    CreateChangeRequestRequest, InboxRequest, ListChangeRequestsRequest, ListRepositoriesRequest, ReportCheckRequest,
    RepositoryRef, SubmitReviewRequest, Verdict,
};
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::features::{browse, diff};
use crate::remote::{GrpcRemote, Remote};

/// Largest file an agent may write in one call, and most files per call.
const MAX_FILE: usize = 4 << 20;
const MAX_FILES: usize = 200;

fn default_rev() -> String {
    String::new()
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RepoArg {
    /// The repository, OWNER/NAME.
    pub repo: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListRepos {
    /// Only this owner's repositories.
    #[serde(default)]
    pub owner: String,
    /// Words to look for in names and descriptions.
    #[serde(default)]
    pub query: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RepoPath {
    /// The repository, OWNER/NAME.
    pub repo: String,
    /// A branch or changeset id; empty: the default branch.
    #[serde(default = "default_rev")]
    pub rev: String,
    /// Repository-relative path; empty for the root.
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindArgs {
    pub repo: String,
    #[serde(default = "default_rev")]
    pub rev: String,
    /// Substring matched against whole paths.
    pub pattern: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HistoryArgs {
    pub repo: String,
    #[serde(default = "default_rev")]
    pub rev: String,
    /// Only changes touching this path.
    #[serde(default)]
    pub path: String,
    /// At most this many (default 20, at most 200).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DiffArgs {
    pub repo: String,
    /// Older revision: a branch or changeset id.
    pub from: String,
    /// Newer revision.
    pub to: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileWrite {
    /// Repository-relative path.
    pub path: String,
    /// The file's new content; omit (null) to delete the file.
    pub content: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WriteArgs {
    pub repo: String,
    /// The branch to land on. A new branch starts from `from`.
    pub branch: String,
    /// Where a new branch starts; empty: the default branch.
    #[serde(default)]
    pub from: String,
    /// Files to write or delete, all landed as one change.
    pub files: Vec<FileWrite>,
    /// What the change does and why.
    pub message: String,
    /// If the branch is protected, open a change request for review instead of failing
    /// (default true).
    #[serde(default)]
    pub propose: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CrListArgs {
    pub repo: String,
    /// open (default), landed, closed or all.
    #[serde(default)]
    pub state: String,
    /// Only this author; `agent/*` for every agent.
    #[serde(default)]
    pub author: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CrArg {
    pub repo: String,
    /// The change request's number.
    pub number: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OpenCrArgs {
    pub repo: String,
    /// The branch with the work.
    pub source: String,
    /// Where it should land; empty: the default branch.
    #[serde(default)]
    pub target: String,
    pub title: String,
    /// Why, and how to check it (Markdown).
    #[serde(default)]
    pub body: String,
    /// Logins to ask for a review.
    #[serde(default)]
    pub reviewers: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CommentArgs {
    pub repo: String,
    pub number: u64,
    /// Markdown.
    pub body: String,
    /// For a line comment: the file.
    #[serde(default)]
    pub path: String,
    /// For a line comment: the line in the new version (1-based).
    #[serde(default)]
    pub line: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReviewArgs {
    pub repo: String,
    pub number: u64,
    /// approve, request_changes or comment.
    pub verdict: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckArgs {
    pub repo: String,
    pub number: u64,
    /// The check's name, e.g. `build`.
    pub name: String,
    /// pending, passed, failed or skipped.
    pub state: String,
    #[serde(default)]
    pub summary: String,
    /// A link to the run (http or https).
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Nothing {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct IssuesArgs {
    pub repo: String,
    /// open (default), closed or all.
    #[serde(default)]
    pub state: String,
    /// Only issues assigned to this login.
    #[serde(default)]
    pub assignee: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct IssueArg {
    pub repo: String,
    pub number: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OpenIssueArgs {
    pub repo: String,
    pub title: String,
    /// What and why (Markdown).
    #[serde(default)]
    pub body: String,
    /// People or agents to work on it.
    #[serde(default)]
    pub assignees: Vec<String>,
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct IssueCommentArgs {
    pub repo: String,
    pub number: u64,
    pub body: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct IssueUpdateArgs {
    pub repo: String,
    pub number: u64,
    /// open, completed or not_planned; empty to leave as is.
    #[serde(default)]
    pub state: String,
    /// Replace the assignees with these; omit to leave them.
    #[serde(default)]
    pub assignees: Option<Vec<String>>,
}

type ToolResult = Result<String, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn status(s: tonic::Status) -> String {
    s.message().to_owned()
}

/// The remote MCP server: one per gateway, shared by every request.
#[derive(Clone)]
pub struct RemoteMcp {
    server: Arc<String>,
}

fn bearer(parts: &Parts) -> Result<String, String> {
    parts
        .headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned)
        .ok_or_else(|| "send your Pando token as `Authorization: Bearer pando_…`".to_owned())
}

fn split(repo: &str) -> Result<RepositoryRef, String> {
    let (owner, name) = repo.split_once('/').ok_or("give the repository as OWNER/NAME")?;
    Ok(RepositoryRef { owner: owner.into(), name: name.into() })
}

fn state_name(s: i32) -> &'static str {
    match ChangeRequestState::try_from(s).unwrap_or(ChangeRequestState::Unspecified) {
        ChangeRequestState::Open => "open",
        ChangeRequestState::Draft => "draft",
        ChangeRequestState::Landed => "landed",
        ChangeRequestState::Closed => "closed",
        ChangeRequestState::Unspecified => "unknown",
    }
}

/// A change request as an agent reads it: what it is, who said what, and what blocks it.
fn describe(cr: &ChangeRequest) -> String {
    let mut out = format!(
        "#{} {} [{}]\n{} wants to land {} into {}: {} files, +{} -{}\n",
        cr.number,
        cr.title,
        state_name(cr.state),
        cr.author,
        cr.source,
        cr.target,
        cr.files,
        cr.additions,
        cr.deletions
    );
    if !cr.body.trim().is_empty() {
        out.push_str(&format!("\n{}\n\n", cr.body.trim()));
    }
    for r in &cr.reviews {
        let v = match Verdict::try_from(r.verdict).unwrap_or(Verdict::Unspecified) {
            Verdict::Approved => "approved",
            Verdict::ChangesRequested => "requested changes",
            _ => "commented",
        };
        let stale = if r.reviewed_tip != cr.source_tip { " (an older version)" } else { "" };
        out.push_str(&format!("review: {} {v}{stale}\n", r.reviewer));
    }
    for c in &cr.checks {
        out.push_str(&format!(
            "check: {} {:?}\n",
            c.name,
            CheckState::try_from(c.state).unwrap_or(CheckState::Pending)
        ));
    }
    if !cr.conflicts.is_empty() {
        out.push_str(&format!("conflicts: {}\n", cr.conflicts.join(", ")));
    }
    if cr.state == ChangeRequestState::Open as i32 || cr.state == ChangeRequestState::Draft as i32 {
        if cr.blockers.is_empty() {
            out.push_str("ready to land: call pando_land_change_request\n");
        } else {
            for b in &cr.blockers {
                out.push_str(&format!("blocked: {b}\n"));
            }
        }
    }
    out
}

impl RemoteMcp {
    pub fn new(server: impl Into<String>) -> Self {
        Self { server: Arc::new(server.into().trim_end_matches('/').to_owned()) }
    }

    /// The caller's view of one repository, with the caller's own token.
    fn repo(&self, parts: &Parts, repo: &str) -> Result<GrpcRemote, String> {
        split(repo)?;
        GrpcRemote::connect(&format!("{}/{repo}", self.server), None, Some(&bearer(parts)?)).map_err(err)
    }

    /// The server's platform and review services, with the caller's own token.
    fn server(&self, parts: &Parts) -> Result<GrpcRemote, String> {
        GrpcRemote::connect(&self.server, None, Some(&bearer(parts)?)).map_err(err)
    }

    /// `rev`, or the repository's default branch when empty.
    async fn rev(&self, parts: &Parts, repo: &str, rev: &str) -> Result<String, String> {
        if !rev.is_empty() {
            return Ok(rev.to_owned());
        }
        let r = self.server(parts)?.platform().get_repository(split(repo)?).await.map_err(status)?.into_inner();
        Ok(if r.default_branch.is_empty() { "main".into() } else { r.default_branch })
    }

    /// Serves MCP over streamable HTTP at `/mcp` until the process stops.
    /// Browsers are refused unless their `Origin` is in `allowed_origins` (agents send none), so
    /// a web page can't drive the gateway with a visitor's network access.
    pub async fn serve_http(
        self,
        listen: std::net::SocketAddr,
        allowed_hosts: Vec<String>,
        allowed_origins: Vec<String>,
    ) -> anyhow::Result<()> {
        use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
        use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
        let mut config = StreamableHttpServerConfig::default();
        config.json_response = true;
        // Older clients (before 2026-07-28) start with `initialize`; answer it statelessly too
        // instead of trying to open a session, which this gateway never keeps.
        config.legacy_session_mode = false;
        if !allowed_hosts.is_empty() {
            config.allowed_hosts = allowed_hosts;
        }
        config = if allowed_origins.is_empty() {
            config.enforce_origin_validation()
        } else {
            config.with_allowed_origins(allowed_origins)
        };
        let me = self.clone();
        let service =
            StreamableHttpService::new(move || Ok(me.clone()), Arc::new(NeverSessionManager::default()), config);
        let app =
            axum::Router::new().nest_service("/mcp", service).route("/healthz", axum::routing::get(|| async { "ok" }));
        let listener = tokio::net::TcpListener::bind(listen).await?;
        eprintln!("pando MCP on http://{listen}/mcp, for {}", self.server);
        axum::serve(listener, app).with_graceful_shutdown(crate::shutdown_signal()).await?;
        Ok(())
    }
}

#[tool_router(server_handler)]
impl RemoteMcp {
    #[tool(description = "List the repositories you can read on this Pando server, with what landed in the last day.")]
    async fn pando_repositories(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<ListRepos>,
    ) -> ToolResult {
        let list = self
            .server(&parts)?
            .platform()
            .list_repositories(ListRepositoriesRequest { owner: a.owner, query: a.query, ..Default::default() })
            .await
            .map_err(status)?
            .into_inner();
        Ok(list
            .repositories
            .iter()
            .map(|r| {
                format!(
                    "{}/{} ({} landed today, {} open reviews) {}",
                    r.owner, r.name, r.lands_24h, r.open_change_requests, r.description
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    #[tool(description = "Read a file from a repository at a branch or changeset, without a checkout.")]
    async fn pando_read_file(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<RepoPath>,
    ) -> ToolResult {
        let rev = self.rev(&parts, &a.repo, &a.rev).await?;
        let bytes = browse::cat(&self.repo(&parts, &a.repo)?, &rev, &a.path).await.map_err(|e| format!("{e:#}"))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tool(description = "List a directory (d = directory, f = file, x = executable, l = link).")]
    async fn pando_list_directory(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<RepoPath>,
    ) -> ToolResult {
        let rev = self.rev(&parts, &a.repo, &a.rev).await?;
        let entries = browse::ls(&self.repo(&parts, &a.repo)?, &rev, &a.path).await.map_err(|e| format!("{e:#}"))?;
        Ok(entries.iter().map(|e| format!("{} {}", browse::mode_char(e.mode), e.name)).collect::<Vec<_>>().join("\n"))
    }

    #[tool(description = "Find files whose path contains a substring.")]
    async fn pando_find_files(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<FindArgs>,
    ) -> ToolResult {
        let rev = self.rev(&parts, &a.repo, &a.rev).await?;
        Ok(self.repo(&parts, &a.repo)?.find(&rev, &a.pattern, 1000).await.map_err(|e| format!("{e:#}"))?.join("\n"))
    }

    #[tool(description = "What landed, newest first: id, author and summary, optionally for one path.")]
    async fn pando_history(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<HistoryArgs>,
    ) -> ToolResult {
        let rev = self.rev(&parts, &a.repo, &a.rev).await?;
        let log = browse::log(&self.repo(&parts, &a.repo)?, &rev, &a.path, a.limit.unwrap_or(20).min(200))
            .await
            .map_err(|e| format!("{e:#}"))?;
        Ok(log
            .iter()
            .map(|c| {
                format!(
                    "{} {} | {}",
                    c.id.short(),
                    c.changeset.author,
                    c.changeset.message.lines().next().unwrap_or("")
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    #[tool(description = "Who last changed each line of a file, and in which change.")]
    async fn pando_blame(&self, Extension(parts): Extension<Parts>, Parameters(a): Parameters<RepoPath>) -> ToolResult {
        let rev = self.rev(&parts, &a.repo, &a.rev).await?;
        let lines = self.repo(&parts, &a.repo)?.blame(&rev, &a.path).await.map_err(|e| format!("{e:#}"))?;
        Ok(lines
            .iter()
            .enumerate()
            .map(|(i, (id, l))| format!("{} {:>5} {l}", id.short(), i + 1))
            .collect::<Vec<_>>()
            .join("\n"))
    }

    #[tool(description = "Unified diff between two revisions (branches or changeset ids).")]
    async fn pando_diff(&self, Extension(parts): Extension<Parts>, Parameters(a): Parameters<DiffArgs>) -> ToolResult {
        diff::revisions(&self.repo(&parts, &a.repo)?, &a.from, &a.to, false).await.map_err(|e| format!("{e:#}"))
    }

    #[tool(description = "Write or delete files and land them on a branch as one change, without a checkout. \
        Concurrent changes to other files are merged on the server. If the branch is protected, the change \
        goes to a new review/ branch and a change request is opened instead (unless propose is false).")]
    async fn pando_write(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<WriteArgs>,
    ) -> ToolResult {
        if a.files.is_empty() || a.files.len() > MAX_FILES {
            return Err(format!("write between 1 and {MAX_FILES} files per change"));
        }
        if a.message.trim().is_empty() {
            return Err("say what the change does in `message`".into());
        }
        let r = self.repo(&parts, &a.repo)?;
        let from = self.rev(&parts, &a.repo, &a.from).await?;
        let tip = r.get_ref(&a.branch).await.map_err(|e| format!("{e:#}"))?;
        let parent = match tip {
            Some(t) => Some(t),
            None => match r.get_ref(&from).await.map_err(|e| format!("{e:#}"))? {
                Some(base) => Some(base),
                // The first change in an empty repository starts its default branch.
                None if a.branch == from => None,
                None => return Err(format!("no branch `{from}` to start from; write to `{from}` first")),
            },
        };
        let mut objects = Vec::new();
        let mut changes = Vec::new();
        for f in &a.files {
            match &f.content {
                Some(text) if text.len() > MAX_FILE => return Err(format!("{} is larger than 4 MB", f.path)),
                Some(text) => {
                    let (id, payload) = encode_object(Kind::Blob, text.as_bytes());
                    objects.push((id, payload));
                    changes.push(FileChange::upsert(f.path.trim_matches('/'), Mode::File, id));
                }
                None => changes.push(FileChange::delete(f.path.trim_matches('/'))),
            }
        }
        r.objects().put_many(objects).await.map_err(|e| format!("{e:#}"))?;
        let cs = r
            .create_changeset(Changeset {
                parents: parent.iter().map(|p| p.0.to_vec()).collect(),
                changes,
                author: String::new(), // the server records who landed it
                message: a.message.trim().to_owned(),
                timestamp_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0),
            })
            .await
            .map_err(|e| format!("{e:#}"))?
            .id;
        let landed = match tip {
            None => r.compare_and_swap(&a.branch, None, cs).await.map(|ok| ok.then_some(cs)),
            Some(_) => r.land(&a.branch, cs).await.map(|(id, _)| Some(id)),
        };
        match landed {
            Ok(Some(id)) => Ok(format!("landed {} on {} as {}", a.files.len(), a.branch, id.short())),
            Ok(None) => Err(format!("{} was created by someone else meanwhile; try again", a.branch)),
            Err(e) if format!("{e:#}").contains("is protected") && a.propose.unwrap_or(true) => {
                let branch = format!("review/{}", &cs.to_hex()[..12]);
                r.compare_and_swap(&branch, None, cs).await.map_err(|e| format!("{e:#}"))?;
                let (title, body) = a.message.trim().split_once('\n').unwrap_or((a.message.trim(), ""));
                let cr = self
                    .server(&parts)?
                    .reviews()
                    .create_change_request(CreateChangeRequestRequest {
                        repository: Some(split(&a.repo)?),
                        source: branch,
                        target: a.branch.clone(),
                        title: title.to_owned(),
                        body: body.trim().to_owned(),
                        ..Default::default()
                    })
                    .await
                    .map_err(status)?
                    .into_inner();
                Ok(format!("{} is protected, so this went for review.\n{}", a.branch, describe(&cr)))
            }
            Err(e) => Err(format!("{e:#}")),
        }
    }

    #[tool(description = "List change requests in a repository with what blocks each from landing.")]
    async fn pando_change_requests(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<CrListArgs>,
    ) -> ToolResult {
        let states = match a.state.as_str() {
            "" | "open" => vec![ChangeRequestState::Open as i32, ChangeRequestState::Draft as i32],
            "landed" => vec![ChangeRequestState::Landed as i32],
            "closed" => vec![ChangeRequestState::Closed as i32],
            "all" => vec![],
            other => return Err(format!("`{other}` is not a state: open, landed, closed or all")),
        };
        let list = self
            .server(&parts)?
            .reviews()
            .list_change_requests(ListChangeRequestsRequest {
                repository: Some(split(&a.repo)?),
                states,
                author: a.author,
                ..Default::default()
            })
            .await
            .map_err(status)?
            .into_inner();
        if list.change_requests.is_empty() {
            return Ok("no change requests".into());
        }
        Ok(list
            .change_requests
            .iter()
            .map(|c| {
                let why = if c.blockers.is_empty() { "ready".to_owned() } else { c.blockers.join("; ") };
                format!("#{} [{}] {} by {} ({why})", c.number, state_name(c.state), c.title, c.author)
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    #[tool(description = "One change request: description, reviews, checks, conflicts and what blocks it.")]
    async fn pando_change_request(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<CrArg>,
    ) -> ToolResult {
        let cr = self
            .server(&parts)?
            .reviews()
            .get_change_request(ChangeRequestRef { repository: Some(split(&a.repo)?), number: a.number })
            .await
            .map_err(status)?
            .into_inner();
        Ok(describe(&cr))
    }

    #[tool(description = "Open a change request proposing a branch for review and landing.")]
    async fn pando_open_change_request(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<OpenCrArgs>,
    ) -> ToolResult {
        let cr = self
            .server(&parts)?
            .reviews()
            .create_change_request(CreateChangeRequestRequest {
                repository: Some(split(&a.repo)?),
                source: a.source,
                target: a.target,
                title: a.title,
                body: a.body,
                draft: false,
                reviewers: a.reviewers,
            })
            .await
            .map_err(status)?
            .into_inner();
        Ok(describe(&cr))
    }

    #[tool(description = "Comment on a change request, or on one line of one of its files.")]
    async fn pando_comment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<CommentArgs>,
    ) -> ToolResult {
        self.server(&parts)?
            .reviews()
            .add_comment(AddCommentRequest {
                r#ref: Some(ChangeRequestRef { repository: Some(split(&a.repo)?), number: a.number }),
                body: a.body,
                path: a.path,
                line: a.line,
                ..Default::default()
            })
            .await
            .map_err(status)?;
        Ok(format!("commented on #{}", a.number))
    }

    #[tool(description = "Review a change request: approve, request_changes or comment. You can't approve your own.")]
    async fn pando_review(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<ReviewArgs>,
    ) -> ToolResult {
        let verdict = match a.verdict.as_str() {
            "approve" | "approved" => Verdict::Approved,
            "request_changes" | "changes" => Verdict::ChangesRequested,
            "comment" => Verdict::Commented,
            other => return Err(format!("`{other}` is not a verdict: approve, request_changes or comment")),
        };
        let cr = self
            .server(&parts)?
            .reviews()
            .submit_review(SubmitReviewRequest {
                r#ref: Some(ChangeRequestRef { repository: Some(split(&a.repo)?), number: a.number }),
                verdict: verdict as i32,
                body: a.body,
            })
            .await
            .map_err(status)?
            .into_inner();
        Ok(describe(&cr))
    }

    #[tool(description = "Land a change request once nothing blocks it.")]
    async fn pando_land_change_request(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<CrArg>,
    ) -> ToolResult {
        let out = self
            .server(&parts)?
            .reviews()
            .land(ChangeRequestRef { repository: Some(split(&a.repo)?), number: a.number })
            .await
            .map_err(status)?
            .into_inner();
        Ok(format!("landed #{} as {}", a.number, hex_short(&out.landed_as)))
    }

    #[tool(description = "Report a CI check (pending, passed, failed, skipped) on a change request's current version.")]
    async fn pando_report_check(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<CheckArgs>,
    ) -> ToolResult {
        let state = match a.state.as_str() {
            "pending" => CheckState::Pending,
            "passed" => CheckState::Passed,
            "failed" => CheckState::Failed,
            "skipped" => CheckState::Skipped,
            other => return Err(format!("`{other}` is not a check state: pending, passed, failed or skipped")),
        };
        let cr = self
            .server(&parts)?
            .reviews()
            .report_check(ReportCheckRequest {
                r#ref: Some(ChangeRequestRef { repository: Some(split(&a.repo)?), number: a.number }),
                check: Some(Check {
                    name: a.name,
                    state: state as i32,
                    summary: a.summary,
                    details_url: a.url,
                    ..Default::default()
                }),
            })
            .await
            .map_err(status)?
            .into_inner();
        Ok(describe(&cr))
    }

    #[tool(description = "List issues in a repository: work to do, and who it is assigned to.")]
    async fn pando_issues(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<IssuesArgs>,
    ) -> ToolResult {
        use pando_proto::IssueState as S;
        let states = match a.state.as_str() {
            "" | "open" => vec![S::Open as i32],
            "closed" => vec![S::Completed as i32, S::NotPlanned as i32],
            "all" => vec![S::Open as i32, S::Completed as i32, S::NotPlanned as i32],
            other => return Err(format!("`{other}` is not a state: open, closed or all")),
        };
        let list = self
            .server(&parts)?
            .issues()
            .list_issues(pando_proto::ListIssuesRequest {
                repository: Some(split(&a.repo)?),
                states,
                assignee: a.assignee,
                ..Default::default()
            })
            .await
            .map_err(status)?
            .into_inner();
        if list.issues.is_empty() {
            return Ok("no issues".into());
        }
        Ok(list
            .issues
            .iter()
            .map(|i| {
                format!(
                    "#{} {}{}",
                    i.number,
                    i.title,
                    if i.assignees.is_empty() {
                        String::new()
                    } else {
                        format!(" (assigned to {})", i.assignees.join(", "))
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    #[tool(description = "Read an issue with its discussion, to start work on it.")]
    async fn pando_issue(&self, Extension(parts): Extension<Parts>, Parameters(a): Parameters<IssueArg>) -> ToolResult {
        let mut c = self.server(&parts)?.issues();
        let r = pando_proto::IssueRef { repository: Some(split(&a.repo)?), number: a.number };
        let i = c.get_issue(r.clone()).await.map_err(status)?.into_inner();
        let comments = c.list_issue_comments(r).await.map_err(status)?.into_inner().comments;
        Ok(issue_text(&i, &comments))
    }

    #[tool(description = "Open an issue: work to do, optionally assigned to people or agents.")]
    async fn pando_open_issue(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<OpenIssueArgs>,
    ) -> ToolResult {
        let i = self
            .server(&parts)?
            .issues()
            .create_issue(pando_proto::CreateIssueRequest {
                repository: Some(split(&a.repo)?),
                title: a.title,
                body: a.body,
                labels: a.labels,
                assignees: a.assignees,
            })
            .await
            .map_err(status)?
            .into_inner();
        Ok(issue_text(&i, &[]))
    }

    #[tool(description = "Comment on an issue.")]
    async fn pando_comment_on_issue(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<IssueCommentArgs>,
    ) -> ToolResult {
        self.server(&parts)?
            .issues()
            .add_issue_comment(pando_proto::AddIssueCommentRequest {
                r#ref: Some(pando_proto::IssueRef { repository: Some(split(&a.repo)?), number: a.number }),
                body: a.body,
            })
            .await
            .map_err(status)?;
        Ok(format!("commented on #{}", a.number))
    }

    #[tool(description = "Close, reopen or reassign an issue.")]
    async fn pando_update_issue(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<IssueUpdateArgs>,
    ) -> ToolResult {
        use pando_proto::IssueState as S;
        let mut fields = Vec::new();
        let state = match a.state.as_str() {
            "" => S::Unspecified,
            "open" => S::Open,
            "completed" | "done" => S::Completed,
            "not_planned" => S::NotPlanned,
            other => return Err(format!("`{other}` is not a state: open, completed or not_planned")),
        };
        if state != S::Unspecified {
            fields.push("state".to_owned());
        }
        if a.assignees.is_some() {
            fields.push("assignees".to_owned());
        }
        let i = self
            .server(&parts)?
            .issues()
            .update_issue(pando_proto::UpdateIssueRequest {
                r#ref: Some(pando_proto::IssueRef { repository: Some(split(&a.repo)?), number: a.number }),
                fields,
                state: state as i32,
                assignees: a.assignees.unwrap_or_default(),
                ..Default::default()
            })
            .await
            .map_err(status)?
            .into_inner();
        Ok(issue_text(&i, &[]))
    }

    #[tool(
        description = "What needs you: issues assigned to you, reviews requested from you, changes requested on yours, your agents' work ready to land."
    )]
    async fn pando_inbox(&self, Extension(parts): Extension<Parts>, Parameters(_): Parameters<Nothing>) -> ToolResult {
        let inbox = self.server(&parts)?.reviews().inbox(InboxRequest::default()).await.map_err(status)?.into_inner();
        if inbox.items.is_empty() {
            return Ok("nothing needs you".into());
        }
        Ok(inbox
            .items
            .iter()
            .filter_map(|i| {
                if let Some(c) = &i.change_request {
                    let repo = c.repository.as_ref().map(|r| format!("{}/{}", r.owner, r.name)).unwrap_or_default();
                    return Some(format!("{}: {repo}#{} {} by {}", i.reason, c.number, c.title, c.author));
                }
                let x = i.issue.as_ref()?;
                let repo = x.repository.as_ref().map(|r| format!("{}/{}", r.owner, r.name)).unwrap_or_default();
                Some(format!("{}: issue {repo}#{} {}", i.reason, x.number, x.title))
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn issue_text(i: &pando_proto::Issue, comments: &[pando_proto::Comment]) -> String {
    let state = match pando_proto::IssueState::try_from(i.state).unwrap_or(pando_proto::IssueState::Unspecified) {
        pando_proto::IssueState::Open => "open",
        pando_proto::IssueState::Completed => "done",
        pando_proto::IssueState::NotPlanned => "not planned",
        pando_proto::IssueState::Unspecified => "unknown",
    };
    let mut out = format!("#{} {} [{state}]\nopened by {}", i.number, i.title, i.author);
    if !i.assignees.is_empty() {
        out.push_str(&format!(", assigned to {}", i.assignees.join(", ")));
    }
    if !i.labels.is_empty() {
        out.push_str(&format!("\nlabels: {}", i.labels.join(", ")));
    }
    if !i.fixed_by.is_empty() {
        out.push_str(&format!(
            "\nfixed by change requests: {}",
            i.fixed_by.iter().map(|n| format!("#{n}")).collect::<Vec<_>>().join(", ")
        ));
    }
    if !i.body.trim().is_empty() {
        out.push_str(&format!("\n\n{}", i.body.trim()));
    }
    for c in comments {
        out.push_str(&format!("\n\n{} commented:\n{}", c.author, c.body));
    }
    out.push_str(&format!(
        "\n\nTo fix it, open a change request whose description says \"Fixes #{}\": landing it closes the issue.",
        i.number
    ));
    out
}

fn hex_short(bytes: &[u8]) -> String {
    bytes.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_a_bearer_token_and_owner_name() {
        let req = axum::http::Request::builder().header("authorization", "Bearer pando_abc").body(()).unwrap();
        let (parts, _) = req.into_parts();
        assert_eq!(bearer(&parts).unwrap(), "pando_abc");
        let (none, _) = axum::http::Request::builder().body(()).unwrap().into_parts();
        assert!(bearer(&none).is_err());
        assert!(split("lumen/fleet").is_ok());
        assert!(split("fleet").is_err());
    }
}
