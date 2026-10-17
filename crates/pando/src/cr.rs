//! `pando cr`: change requests from the command line, for people, agents and CI; and
//! `pando protect` for protected branches.

use anyhow::{Context, Result, anyhow};
use clap::{Subcommand, ValueEnum};
use pando::remote::Ch;
use pando_proto::reviews_client::ReviewsClient;
use pando_proto::{
    AddCommentRequest, ChangeRequest, ChangeRequestRef, ChangeRequestState, Check, CheckState,
    CreateChangeRequestRequest, ListChangeRequestsRequest, Protection, ProtectionRef, ReportCheckRequest,
    RepositoryRef, SubmitReviewRequest, UpdateChangeRequestRequest, Verdict,
};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ReviewArg {
    Approve,
    /// Request changes.
    Changes,
    Comment,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CheckArg {
    Pending,
    Passed,
    Failed,
    Skipped,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum StateArg {
    Open,
    Draft,
    Landed,
    Closed,
    All,
}

#[derive(Subcommand)]
pub enum CrCmd {
    /// Propose a branch for landing on another: `pando cr create --title "…"`.
    Create {
        #[arg(long)]
        title: String,
        #[arg(long, default_value = "")]
        body: String,
        /// The branch to land; defaults to the workspace's branch.
        #[arg(long)]
        source: Option<String>,
        /// Where to land it; defaults to the repository's default branch.
        #[arg(long, default_value = "")]
        target: String,
        #[arg(long)]
        draft: bool,
        /// Ask someone to review (repeatable).
        #[arg(long = "reviewer")]
        reviewers: Vec<String>,
    },
    /// List change requests.
    List {
        #[arg(long, value_enum, default_value = "open")]
        state: StateArg,
        /// Only this author; `agent/*` for every agent.
        #[arg(long, default_value = "")]
        author: String,
    },
    /// Show one, with what still blocks it.
    View {
        number: u64,
    },
    /// Comment, on the change or on a line of a file.
    Comment {
        number: u64,
        #[arg(short, long)]
        message: String,
        #[arg(long, requires = "line")]
        path: Option<String>,
        #[arg(long)]
        line: Option<u32>,
    },
    /// Approve, request changes, or comment.
    Review {
        number: u64,
        #[arg(value_enum)]
        verdict: ReviewArg,
        #[arg(short, long, default_value = "")]
        message: String,
    },
    /// Report a CI check on the current version: `pando cr check 12 build passed --url …`.
    Check {
        number: u64,
        name: String,
        #[arg(value_enum)]
        state: CheckArg,
        #[arg(long, default_value = "")]
        url: String,
        #[arg(long, default_value = "")]
        summary: String,
    },
    /// Land it, once nothing blocks it.
    Land {
        number: u64,
    },
    /// Replay it on the target's tip.
    Rebase {
        number: u64,
    },
    /// Mark a draft ready for review.
    Ready {
        number: u64,
    },
    Close {
        number: u64,
    },
    Reopen {
        number: u64,
    },
}

#[derive(Subcommand)]
pub enum ProtectCmd {
    /// List protected branches.
    List,
    /// Protect a branch or prefix (`main`, `release/*`).
    Set {
        pattern: String,
        /// Approvals needed before a change request lands.
        #[arg(long, default_value_t = 1)]
        approvals: u32,
        /// A check that must pass (repeatable).
        #[arg(long = "check")]
        checks: Vec<String>,
        /// Keep approvals when the branch changes after them.
        #[arg(long)]
        keep_stale: bool,
        /// Let agents land on it directly, without review.
        #[arg(long)]
        agents_direct: bool,
        /// Who may land without review: a login, a team `org/team`, or `admins` (repeatable).
        #[arg(long = "bypass")]
        bypass: Vec<String>,
    },
    /// Stop protecting a branch or prefix.
    Remove { pattern: String },
}

fn explain(s: tonic::Status) -> anyhow::Error {
    if s.code() == tonic::Code::Unimplemented {
        anyhow!("this server has no change requests; they come with `pandod server --platform`")
    } else {
        anyhow!("{}", s.message())
    }
}

fn state_name(s: i32) -> &'static str {
    match ChangeRequestState::try_from(s).unwrap_or(ChangeRequestState::Unspecified) {
        ChangeRequestState::Open => "open",
        ChangeRequestState::Draft => "draft",
        ChangeRequestState::Landed => "landed",
        ChangeRequestState::Closed => "closed",
        ChangeRequestState::Unspecified => "?",
    }
}

fn print(cr: &ChangeRequest) {
    println!("#{} {} [{}]", cr.number, cr.title, state_name(cr.state));
    println!(
        "  {} wants to land {} into {}: {} files, +{} -{}",
        cr.author, cr.source, cr.target, cr.files, cr.additions, cr.deletions
    );
    for r in &cr.reviews {
        let v = match Verdict::try_from(r.verdict).unwrap_or(Verdict::Unspecified) {
            Verdict::Approved => "approved",
            Verdict::ChangesRequested => "requested changes",
            _ => "commented",
        };
        let stale = if r.reviewed_tip != cr.source_tip { " (on an older version)" } else { "" };
        println!("  {} {v}{stale}", r.reviewer);
    }
    for c in &cr.checks {
        let s = match CheckState::try_from(c.state).unwrap_or(CheckState::Unspecified) {
            CheckState::Passed => "passed",
            CheckState::Failed => "failed",
            CheckState::Skipped => "skipped",
            _ => "pending",
        };
        println!("  check {}: {s}", c.name);
    }
    if cr.state == ChangeRequestState::Open as i32 || cr.state == ChangeRequestState::Draft as i32 {
        if cr.blockers.is_empty() {
            println!("  ready to land: pando cr land {}", cr.number);
        } else {
            for b in &cr.blockers {
                println!("  blocked: {b}");
            }
        }
    }
}

/// Opens a change request for the workspace's committed work on a protected branch: pushes it
/// to a new branch `review/ID` and proposes that branch for the protected one. Returns the
/// change request's number and the branch.
pub async fn propose(
    t: Target,
    remote: &dyn pando::remote::Remote,
    ws: &pando::workspace::Workspace,
) -> Result<(u64, String)> {
    let Target { mut client, repo, .. } = t;
    let tip = ws.base.context("nothing to propose: commit first")?;
    let branch = format!("review/{}", &tip.to_hex()[..12]);
    if !remote.compare_and_swap(&branch, None, tip).await? && remote.get_ref(&branch).await? != Some(tip) {
        anyhow::bail!("branch {branch} already exists with other work");
    }
    let message =
        remote.log(&tip.to_hex(), "", 1).await?.into_iter().next().map(|c| c.changeset.message).unwrap_or_default();
    let (title, body) = message.split_once('\n').unwrap_or((&message, ""));
    let title =
        if title.trim().is_empty() { format!("Changes for {}", ws.config.ref_name) } else { title.trim().to_owned() };
    let cr = client
        .create_change_request(CreateChangeRequestRequest {
            repository: Some(repo),
            source: branch.clone(),
            target: ws.config.ref_name.clone(),
            title,
            body: body.trim().to_owned(),
            ..Default::default()
        })
        .await
        .map_err(explain)?
        .into_inner();
    Ok((cr.number, branch))
}

pub struct Target {
    pub client: ReviewsClient<Ch>,
    pub issues: pando_proto::issues_client::IssuesClient<Ch>,
    pub repo: RepositoryRef,
    /// The workspace's branch, for `cr create` without `--source`.
    pub branch: Option<String>,
}

pub async fn cr(t: Target, cmd: CrCmd) -> Result<()> {
    let Target { mut client, repo, branch, .. } = t;
    let r = |number| ChangeRequestRef { repository: Some(repo.clone()), number };
    let update = |number, state: ChangeRequestState| UpdateChangeRequestRequest {
        r#ref: Some(ChangeRequestRef { repository: Some(repo.clone()), number }),
        fields: vec!["state".into()],
        state: state as i32,
        ..Default::default()
    };
    match cmd {
        CrCmd::Create { title, body, source, target, draft, reviewers } => {
            let source = source.or(branch).context("give --source BRANCH (or run this in a workspace)")?;
            let cr = client
                .create_change_request(CreateChangeRequestRequest {
                    repository: Some(repo.clone()),
                    source,
                    target,
                    title,
                    body,
                    draft,
                    reviewers,
                })
                .await
                .map_err(explain)?
                .into_inner();
            print(&cr);
        }
        CrCmd::List { state, author } => {
            let states = match state {
                StateArg::Open => vec![ChangeRequestState::Open as i32, ChangeRequestState::Draft as i32],
                StateArg::Draft => vec![ChangeRequestState::Draft as i32],
                StateArg::Landed => vec![ChangeRequestState::Landed as i32],
                StateArg::Closed => vec![ChangeRequestState::Closed as i32],
                StateArg::All => vec![],
            };
            let list = client
                .list_change_requests(ListChangeRequestsRequest {
                    repository: Some(repo.clone()),
                    states,
                    author,
                    ..Default::default()
                })
                .await
                .map_err(explain)?
                .into_inner();
            for c in list.change_requests {
                let open = c.state == ChangeRequestState::Open as i32 || c.state == ChangeRequestState::Draft as i32;
                let status = match (open, c.blockers.is_empty()) {
                    (false, _) => String::new(),
                    (true, true) => "ready to land".to_owned(),
                    (true, false) => c.blockers.join("; "),
                };
                println!("#{}\t{}\t{}\t{}\t{status}", c.number, state_name(c.state), c.author, c.title);
            }
        }
        CrCmd::View { number } => print(&client.get_change_request(r(number)).await.map_err(explain)?.into_inner()),
        CrCmd::Comment { number, message, path, line } => {
            client
                .add_comment(AddCommentRequest {
                    r#ref: Some(r(number)),
                    body: message,
                    path: path.unwrap_or_default(),
                    line: line.unwrap_or(0),
                    ..Default::default()
                })
                .await
                .map_err(explain)?;
            println!("commented on #{number}");
        }
        CrCmd::Review { number, verdict, message } => {
            let v = match verdict {
                ReviewArg::Approve => Verdict::Approved,
                ReviewArg::Changes => Verdict::ChangesRequested,
                ReviewArg::Comment => Verdict::Commented,
            };
            let cr = client
                .submit_review(SubmitReviewRequest { r#ref: Some(r(number)), verdict: v as i32, body: message })
                .await
                .map_err(explain)?
                .into_inner();
            print(&cr);
        }
        CrCmd::Check { number, name, state, url, summary } => {
            let s = match state {
                CheckArg::Pending => CheckState::Pending,
                CheckArg::Passed => CheckState::Passed,
                CheckArg::Failed => CheckState::Failed,
                CheckArg::Skipped => CheckState::Skipped,
            };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let check = Check {
                name,
                state: s as i32,
                summary,
                details_url: url,
                started_at_ms: now,
                finished_at_ms: if s == CheckState::Pending { 0 } else { now },
            };
            let cr = client
                .report_check(ReportCheckRequest { r#ref: Some(r(number)), check: Some(check) })
                .await
                .map_err(explain)?
                .into_inner();
            print(&cr);
        }
        CrCmd::Land { number } => {
            let out = client.land(r(number)).await.map_err(explain)?.into_inner();
            let id = out.landed_as.iter().map(|b| format!("{b:02x}")).collect::<String>();
            println!("landed #{number} as {}", &id[..id.len().min(12)]);
        }
        CrCmd::Rebase { number } => print(&client.rebase(r(number)).await.map_err(explain)?.into_inner()),
        CrCmd::Ready { number } => print(
            &client
                .update_change_request(update(number, ChangeRequestState::Open))
                .await
                .map_err(explain)?
                .into_inner(),
        ),
        CrCmd::Close { number } => print(
            &client
                .update_change_request(update(number, ChangeRequestState::Closed))
                .await
                .map_err(explain)?
                .into_inner(),
        ),
        CrCmd::Reopen { number } => print(
            &client
                .update_change_request(update(number, ChangeRequestState::Open))
                .await
                .map_err(explain)?
                .into_inner(),
        ),
    }
    Ok(())
}

pub async fn protect(t: Target, cmd: ProtectCmd) -> Result<()> {
    let Target { mut client, repo, .. } = t;
    match cmd {
        ProtectCmd::List => {
            for p in client.list_protections(repo).await.map_err(explain)?.into_inner().protections {
                let checks = if p.required_checks.is_empty() {
                    "no checks".to_owned()
                } else {
                    format!("checks: {}", p.required_checks.join(", "))
                };
                let bypass =
                    if p.bypass.is_empty() { String::new() } else { format!("; bypass: {}", p.bypass.join(", ")) };
                let agents = if p.agents_land_directly { "; agents land directly" } else { "" };
                println!("{}\t{} approvals, {checks}{agents}{bypass}", p.pattern, p.required_approvals);
            }
        }
        ProtectCmd::Set { pattern, approvals, checks, keep_stale, agents_direct, bypass } => {
            let p = client
                .set_protection(Protection {
                    repository: Some(repo),
                    pattern,
                    required_approvals: approvals,
                    required_checks: checks,
                    dismiss_stale: !keep_stale,
                    agents_land_directly: agents_direct,
                    bypass,
                })
                .await
                .map_err(explain)?
                .into_inner();
            let n = p.required_approvals;
            println!(
                "protected {}: changes land through a change request with {n} {}",
                p.pattern,
                if n == 1 { "approval" } else { "approvals" }
            );
        }
        ProtectCmd::Remove { pattern } => {
            client
                .delete_protection(ProtectionRef { repository: Some(repo), pattern: pattern.clone() })
                .await
                .map_err(explain)?;
            println!("{pattern} is no longer protected");
        }
    }
    Ok(())
}
