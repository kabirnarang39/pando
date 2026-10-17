//! Review slice (platform phase 3, ADR-024): change requests from a branch into a target, line
//! comments and suggestions, reviews, CI checks, protected branches, an inbox and an activity
//! feed. A change request lands as one changeset through the same server-side rebase as every
//! other land; its review is the gate, in place of branch protection.

pub mod store;

use std::collections::BTreeSet;
use std::sync::Arc;

use pando_core::acl::Principal;
use pando_core::object::{Changeset, Kind};
use pando_core::{Id, Repo, paths_overlap, tree};
use pando_proto::reviews_server::{self, ReviewsServer};
use pando_proto::{
    AccountKind, AddCommentRequest, ChangeRequest, ChangeRequestRef, ChangeRequestState, CheckState, Comment,
    CreateChangeRequestRequest, Empty, Event, EventKind, InboxItem, InboxRequest, InboxResponse,
    LandChangeRequestResponse, ListChangeRequestsRequest, ListChangeRequestsResponse, ListCommentsResponse,
    ListEventsRequest, ListEventsResponse, ListProtectionsResponse, Protection, ProtectionRef, ReportCheckRequest,
    RepositoryRef, ResolveThreadRequest, ReviewSummary, Role, SubmitReviewRequest, UpdateChangeRequestRequest, Verdict,
};
use tonic::{Request, Response, Status};

use self::store::{CrRec, ReviewRec, check_state};
use crate::auth::{TokenScope, principal};
use crate::directory::{check_login, kind_of};
use crate::features::land::Land;
use crate::hosting::{Hosting, Registry, RepoKey, RepoRecord, now_ms};
use crate::status::{RpcResult, id_from, to_status};
use crate::webhooks;

/// Line counts are worked out for at most this many files of a change.
const STAT_FILES: usize = 300;

#[derive(Clone)]
pub struct Reviews {
    hosting: Arc<Hosting>,
}

pub fn service(s: Reviews) -> ReviewsServer<Reviews> {
    ReviewsServer::new(s)
}

fn internal(e: impl std::fmt::Display) -> Status {
    Status::internal(e.to_string())
}

fn caller<T>(req: &Request<T>) -> (Principal, Option<TokenScope>) {
    (principal(req), req.extensions().get::<TokenScope>().cloned())
}

fn audit(kind: &str, who: &Principal, target: &str) {
    tracing::info!(target: crate::telemetry::AUDIT, kind, principal = %who.0, target);
}

fn key_of(r: Option<&RepositoryRef>) -> Result<RepoKey, Status> {
    let r = r.ok_or_else(|| Status::invalid_argument("name the repository"))?;
    RepoKey::new(&r.owner, &r.name)
}

fn branch_ok(b: &str) -> Result<(), Status> {
    pando_core::validate_ref_name(b).map_err(to_status)
}

/// What a caller is allowed on one repository, with the repository opened.
struct Ctx {
    rec: RepoRecord,
    repo: Repo,
    land: Option<Land>,
    role: Role,
}

impl Reviews {
    pub fn new(hosting: Arc<Hosting>) -> Self {
        Self { hosting }
    }

    fn reg(&self) -> &Registry {
        &self.hosting.registry
    }

    async fn ctx(&self, who: &Principal, scope: Option<&TokenScope>, key: &RepoKey, need: Role) -> Result<Ctx, Status> {
        let rec = self.hosting.authorize(key, who, scope, need)?;
        let role = self.hosting.access.effective(self.reg(), who, scope, &rec)?;
        let (repo, land) = self.hosting.open_repo(key).await?;
        Ok(Ctx { rec, repo, land, role })
    }

    fn get_cr(&self, key: &RepoKey, number: u64) -> Result<CrRec, Status> {
        self.reg()
            .change_request(key, number)
            .map_err(internal)?
            .ok_or_else(|| Status::not_found(format!("no change request #{number} in {key}")))
    }

    /// Brings a change request up to date with its source branch: a new version resets its
    /// line counts (and, through the tip, which reviews and checks count).
    async fn refresh(&self, repo: &Repo, mut cr: CrRec) -> Result<CrRec, Status> {
        if !matches!(cr.state, ChangeRequestState::Open | ChangeRequestState::Draft) {
            return Ok(cr);
        }
        let Some(tip) = repo.refs.get(&cr.source).await.map_err(to_status)? else { return Ok(cr) };
        if tip.0.as_slice() != cr.source_tip.as_slice() {
            cr.source_tip = tip.0.to_vec();
            (cr.files, cr.additions, cr.deletions) = stats(repo, id_from(&cr.base)?, tip).await?;
            cr.updated_at_ms = now_ms();
            self.reg().update_change_request(&cr).map_err(internal)?;
        }
        Ok(cr)
    }

    /// The change request as clients see it, with reviews, checks, conflicts and what blocks it.
    async fn view(&self, repo: &Repo, cr: CrRec) -> Result<ChangeRequest, Status> {
        let reviews = self.reg().reviews(&cr.key, cr.number).map_err(internal)?;
        let checks = self.reg().checks(&cr.key, &cr.source_tip).map_err(internal)?;
        let comments = self.reg().comments(&cr.key, cr.number).map_err(internal)?.len() as u32;
        let open = matches!(cr.state, ChangeRequestState::Open | ChangeRequestState::Draft);
        let conflicts = if open { conflicts(repo, &cr).await? } else { vec![] };
        let protection = self.reg().protection_for(&cr.key, &cr.target).map_err(internal)?;
        let blockers = if open { blockers(&cr, &reviews, &checks, &conflicts, protection.as_ref()) } else { vec![] };
        Ok(ChangeRequest {
            repository: Some(RepositoryRef { owner: cr.key.owner.clone(), name: cr.key.name.clone() }),
            number: cr.number,
            title: cr.title,
            body: cr.body,
            author: cr.author,
            state: cr.state as i32,
            source: cr.source,
            target: cr.target,
            source_tip: cr.source_tip,
            base: cr.base,
            created_at_ms: cr.created_at_ms,
            updated_at_ms: cr.updated_at_ms,
            requested_reviewers: cr.reviewers,
            reviews: reviews
                .into_iter()
                .map(|r| ReviewSummary {
                    reviewer: r.reviewer,
                    verdict: r.verdict as i32,
                    at_ms: r.at_ms,
                    reviewed_tip: r.reviewed_tip,
                })
                .collect(),
            checks,
            comments,
            files: cr.files,
            additions: cr.additions,
            deletions: cr.deletions,
            conflicts,
            blockers,
            landed_as: cr.landed_as,
            labels: cr.labels,
            from_protected_land: cr.from_protected_land,
        })
    }

    fn event(&self, kind: EventKind, who: &Principal, cr: &CrRec, changeset: &[u8], summary: &str) {
        if let Err(e) = self.reg().record_event(kind, &who.0, &cr.key, cr.number, changeset, summary) {
            tracing::warn!(error = %e, "recording an event");
        }
        let (event, action) = match kind {
            EventKind::ChangeRequestOpened => ("change_request", "opened"),
            EventKind::ChangeRequestClosed => ("change_request", "closed"),
            EventKind::ChangeRequestLanded => ("change_request", "landed"),
            EventKind::Reviewed => ("review", summary),
            EventKind::CommentedOn => ("comment", "created"),
            _ => return,
        };
        let mut p = webhooks::envelope(event, action, &cr.key, &who.0);
        p["change_request"] = cr_json(cr);
        if !changeset.is_empty() {
            p["landed_as"] = serde_json::json!(hex::encode(changeset));
        }
        webhooks::emit(self.reg(), &cr.key, event, p);
    }

    /// May `who` change this change request: its author, the person an agent author answers
    /// to, or anyone with at least `role` on the repository.
    fn may_edit(&self, who: &Principal, ctx: &Ctx, cr: &CrRec, role: Role) -> bool {
        ctx.role >= role || who.0 == cr.author || self.answers_for(who, &cr.author)
    }

    fn answers_for(&self, who: &Principal, author: &str) -> bool {
        matches!(self.reg().account(author), Ok(Some(a)) if a.owner == who.0)
    }
}

/// A change request as webhook payloads carry it.
fn cr_json(cr: &CrRec) -> serde_json::Value {
    serde_json::json!({
        "number": cr.number,
        "title": cr.title,
        "author": cr.author,
        "state": format!("{:?}", cr.state).to_lowercase(),
        "source": cr.source,
        "target": cr.target,
        "source_tip": hex::encode(&cr.source_tip),
        "base": hex::encode(&cr.base),
        "draft": cr.state == ChangeRequestState::Draft,
    })
}

/// Files, added lines and removed lines between two versions.
async fn stats(repo: &Repo, base: Id, tip: Id) -> Result<(u32, u32, u32), Status> {
    let changes = changes_between(repo, base, tip).await?;
    let (mut add, mut del) = (0u32, 0u32);
    for c in changes.iter().take(STAT_FILES) {
        let old = match repo.lookup(base, &c.path).await.map_err(to_status)? {
            Some((_, id)) => tree::read_object(repo.objects.as_ref(), &id, Kind::Blob).await.map_err(to_status)?,
            None => vec![],
        };
        let new = if c.blob.is_empty() {
            vec![]
        } else {
            tree::read_object(repo.objects.as_ref(), &id_from(&c.blob)?, Kind::Blob).await.map_err(to_status)?
        };
        if old.contains(&0) || new.contains(&0) {
            continue; // binary
        }
        let (old, new) = (String::from_utf8_lossy(&old), String::from_utf8_lossy(&new));
        for op in similar::TextDiff::from_lines(old.as_ref(), new.as_ref()).ops() {
            for change in similar::TextDiff::from_lines(old.as_ref(), new.as_ref()).iter_changes(op) {
                match change.tag() {
                    similar::ChangeTag::Insert => add += 1,
                    similar::ChangeTag::Delete => del += 1,
                    similar::ChangeTag::Equal => {}
                }
            }
        }
    }
    Ok((changes.len() as u32, add, del))
}

async fn changes_between(repo: &Repo, from: Id, to: Id) -> Result<Vec<pando_core::object::FileChange>, Status> {
    let a = repo.root_tree(from).await.map_err(to_status)?;
    let b = repo.root_tree(to).await.map_err(to_status)?;
    tree::diff_trees(repo.objects.as_ref(), Some(a), Some(b)).await.map_err(to_status)
}

/// Files the change and the target have both changed since the change's base.
async fn conflicts(repo: &Repo, cr: &CrRec) -> Result<Vec<String>, Status> {
    let base = id_from(&cr.base)?;
    let Some(target) = repo.refs.get(&cr.target).await.map_err(to_status)? else { return Ok(vec![]) };
    if target == base {
        return Ok(vec![]);
    }
    // ponytail: two tree diffs per open change request per read; cache per (base, tips) if
    // listings of hundreds of open requests get slow.
    let landed = changes_between(repo, base, target).await?;
    let mine = changes_between(repo, base, id_from(&cr.source_tip)?).await?;
    let mut out: BTreeSet<String> = BTreeSet::new();
    for c in &mine {
        if landed.iter().any(|l| paths_overlap(&l.path, &c.path)) {
            out.insert(c.path.clone());
        }
    }
    Ok(out.into_iter().collect())
}

/// Why a change request can't land yet, in words; empty when it can.
pub fn blockers(
    cr: &CrRec,
    reviews: &[ReviewRec],
    checks: &[pando_proto::Check],
    conflicts: &[String],
    protection: Option<&Protection>,
) -> Vec<String> {
    let mut out = Vec::new();
    if cr.state == ChangeRequestState::Draft {
        out.push("Draft: mark it ready for review".to_owned());
    }
    let current = |r: &&ReviewRec| r.reviewed_tip == cr.source_tip;
    let dismiss = protection.is_some_and(|p| p.dismiss_stale);
    for r in reviews.iter().filter(|r| r.verdict == Verdict::ChangesRequested && (!dismiss || current(r))) {
        out.push(format!("Changes requested by {}", r.reviewer));
    }
    if let Some(p) = protection {
        let approvals = reviews
            .iter()
            .filter(|r| r.verdict == Verdict::Approved && r.reviewer != cr.author && (!p.dismiss_stale || current(r)))
            .count() as u32;
        if approvals < p.required_approvals {
            let n = p.required_approvals - approvals;
            out.push(format!("Needs {n} more {}", if n == 1 { "approval" } else { "approvals" }));
        }
        for name in &p.required_checks {
            match checks.iter().find(|c| &c.name == name).map(check_state) {
                Some(CheckState::Passed) | Some(CheckState::Skipped) => {}
                Some(CheckState::Failed) => out.push(format!("Check {name} failed")),
                _ => out.push(format!("Waiting for check {name}")),
            }
        }
    }
    if !conflicts.is_empty() {
        let n = conflicts.len();
        out.push(format!("Conflicts in {n} {}", if n == 1 { "file" } else { "files" }));
    }
    out
}

#[tonic::async_trait]
impl reviews_server::Reviews for Reviews {
    async fn list_change_requests(
        &self,
        req: Request<ListChangeRequestsRequest>,
    ) -> RpcResult<ListChangeRequestsResponse> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let key = key_of(r.repository.as_ref())?;
        let ctx = self.ctx(&who, scope.as_ref(), &key, Role::Read).await?;
        let states: Vec<ChangeRequestState> =
            r.states.iter().filter_map(|s| ChangeRequestState::try_from(*s).ok()).collect();
        let limit = if r.limit == 0 { 100 } else { r.limit.min(500) } as usize;
        let author_glob = |a: &str| match r.author.strip_suffix('*') {
            Some(prefix) => a.starts_with(prefix),
            None => r.author.is_empty() || a == r.author,
        };
        let q = r.query.to_lowercase();
        let mut out = Vec::new();
        for cr in self.reg().change_requests(Some(&key), &states, usize::MAX).map_err(internal)? {
            if !author_glob(&cr.author)
                || (!r.reviewer.is_empty() && !cr.reviewers.contains(&r.reviewer))
                || (!q.is_empty() && !cr.title.to_lowercase().contains(&q))
            {
                continue;
            }
            let cr = self.refresh(&ctx.repo, cr).await?;
            out.push(self.view(&ctx.repo, cr).await?);
            if out.len() == limit {
                break;
            }
        }
        Ok(Response::new(ListChangeRequestsResponse { change_requests: out, next_page_token: String::new() }))
    }

    async fn get_change_request(&self, req: Request<ChangeRequestRef>) -> RpcResult<ChangeRequest> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let key = key_of(r.repository.as_ref())?;
        let ctx = self.ctx(&who, scope.as_ref(), &key, Role::Read).await?;
        let cr = self.refresh(&ctx.repo, self.get_cr(&key, r.number)?).await?;
        Ok(Response::new(self.view(&ctx.repo, cr).await?))
    }

    async fn create_change_request(&self, req: Request<CreateChangeRequestRequest>) -> RpcResult<ChangeRequest> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let key = key_of(r.repository.as_ref())?;
        let ctx = self.ctx(&who, scope.as_ref(), &key, Role::Triage).await?;
        let target = if r.target.is_empty() { ctx.rec.default_branch.clone() } else { r.target };
        branch_ok(&r.source)?;
        branch_ok(&target)?;
        if r.source == target {
            return Err(Status::invalid_argument("a change request goes from one branch into another"));
        }
        let title = r.title.trim().to_owned();
        if title.is_empty() || title.len() > 300 {
            return Err(Status::invalid_argument("give it a title of up to 300 characters"));
        }
        for reviewer in &r.reviewers {
            check_login(reviewer)?;
        }
        let missing = |b: &str| Status::failed_precondition(format!("there is no branch `{b}` in {key}"));
        let source_tip = ctx.repo.refs.get(&r.source).await.map_err(to_status)?.ok_or_else(|| missing(&r.source))?;
        let target_tip = ctx.repo.refs.get(&target).await.map_err(to_status)?.ok_or_else(|| missing(&target))?;
        let base =
            ctx.repo.merge_base(source_tip, target_tip).await.map_err(to_status)?.ok_or_else(|| {
                Status::failed_precondition(format!("`{}` and `{target}` share no history", r.source))
            })?;
        if base == source_tip {
            return Err(Status::failed_precondition(format!(
                "`{}` has nothing that `{target}` doesn't already have",
                r.source
            )));
        }
        let open = self
            .reg()
            .change_requests(Some(&key), &[ChangeRequestState::Open, ChangeRequestState::Draft], usize::MAX)
            .map_err(internal)?;
        if let Some(dup) = open.iter().find(|c| c.source == r.source && c.target == target) {
            return Err(Status::already_exists(format!(
                "#{} already proposes `{}` into `{target}`",
                dup.number, r.source
            )));
        }
        let (files, additions, deletions) = stats(&ctx.repo, base, source_tip).await?;
        // An agent's change goes to the person who answers for it, unless someone else was asked.
        let mut reviewers = r.reviewers;
        if reviewers.is_empty()
            && let Some(owner) =
                self.reg().account(&who.0).map_err(internal)?.map(|a| a.owner).filter(|o| !o.is_empty())
        {
            reviewers.push(owner);
        }
        // `pando land` on a protected branch proposes the work from a `review/` branch.
        let from_protected_land =
            r.source.starts_with("review/") && self.reg().protection_for(&key, &target).map_err(internal)?.is_some();
        let now = now_ms();
        let mut cr = CrRec {
            key: key.clone(),
            number: 0,
            title,
            body: r.body,
            author: who.0.clone(),
            state: if r.draft { ChangeRequestState::Draft } else { ChangeRequestState::Open },
            source: r.source,
            target,
            source_tip: source_tip.0.to_vec(),
            base: base.0.to_vec(),
            created_at_ms: now,
            updated_at_ms: now,
            reviewers,
            labels: vec![],
            landed_as: vec![],
            from_protected_land,
            files,
            additions,
            deletions,
        };
        cr.number = self.reg().create_change_request(&cr).map_err(internal)?;
        self.event(EventKind::ChangeRequestOpened, &who, &cr, &[], &cr.title.clone());
        audit("create_change_request", &who, &format!("{key}#{}", cr.number));
        Ok(Response::new(self.view(&ctx.repo, cr).await?))
    }

    async fn update_change_request(&self, req: Request<UpdateChangeRequestRequest>) -> RpcResult<ChangeRequest> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let cref = r.r#ref.ok_or_else(|| Status::invalid_argument("name the change request"))?;
        let key = key_of(cref.repository.as_ref())?;
        let ctx = self.ctx(&who, scope.as_ref(), &key, Role::Read).await?;
        let mut cr = self.refresh(&ctx.repo, self.get_cr(&key, cref.number)?).await?;
        if !self.may_edit(&who, &ctx, &cr, Role::Triage) {
            return Err(Status::permission_denied(format!(
                "only its author or someone with triage access may change #{}",
                cr.number
            )));
        }
        for f in &r.fields {
            match f.as_str() {
                "title" if !r.title.trim().is_empty() => cr.title = r.title.trim().to_owned(),
                "body" => cr.body = r.body.clone(),
                "reviewers" => {
                    for l in &r.reviewers {
                        check_login(l)?;
                    }
                    cr.reviewers = r.reviewers.clone()
                }
                "labels" => {
                    cr.labels = r
                        .labels
                        .iter()
                        .map(|l| l.trim().to_owned())
                        .filter(|l| !l.is_empty() && !l.contains(','))
                        .collect()
                }
                "state" => {
                    let to = ChangeRequestState::try_from(r.state).unwrap_or(ChangeRequestState::Unspecified);
                    match (cr.state, to) {
                        (ChangeRequestState::Landed, _) => return Err(Status::failed_precondition("it has landed")),
                        (_, ChangeRequestState::Open | ChangeRequestState::Draft | ChangeRequestState::Closed) => {
                            if to == ChangeRequestState::Closed && cr.state != ChangeRequestState::Closed {
                                self.event(EventKind::ChangeRequestClosed, &who, &cr, &[], "");
                            }
                            cr.state = to
                        }
                        _ => return Err(Status::invalid_argument("a change request can be open, draft or closed")),
                    }
                }
                other => return Err(Status::invalid_argument(format!("`{other}` can't be changed"))),
            }
        }
        cr.updated_at_ms = now_ms();
        self.reg().update_change_request(&cr).map_err(internal)?;
        Ok(Response::new(self.view(&ctx.repo, cr).await?))
    }

    async fn list_comments(&self, req: Request<ChangeRequestRef>) -> RpcResult<ListCommentsResponse> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let key = key_of(r.repository.as_ref())?;
        self.ctx(&who, scope.as_ref(), &key, Role::Read).await?;
        self.get_cr(&key, r.number)?;
        Ok(Response::new(ListCommentsResponse { comments: self.reg().comments(&key, r.number).map_err(internal)? }))
    }

    async fn add_comment(&self, req: Request<AddCommentRequest>) -> RpcResult<Comment> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let cref = r.r#ref.ok_or_else(|| Status::invalid_argument("name the change request"))?;
        let key = key_of(cref.repository.as_ref())?;
        let ctx = self.ctx(&who, scope.as_ref(), &key, Role::Read).await?;
        let mut cr = self.refresh(&ctx.repo, self.get_cr(&key, cref.number)?).await?;
        let body = r.body.trim().to_owned();
        if body.is_empty() || body.len() > 65_536 {
            return Err(Status::invalid_argument("a comment needs some text, up to 64 KB"));
        }
        if !r.path.is_empty() && (r.line == 0 || (r.end_line != 0 && r.end_line < r.line)) {
            return Err(Status::invalid_argument("a line comment needs a line, and an end line after it"));
        }
        if !r.thread.is_empty()
            && !self.reg().comments(&key, cr.number).map_err(internal)?.iter().any(|c| c.id == r.thread)
        {
            return Err(Status::not_found(format!("no thread `{}`", r.thread)));
        }
        let mut raw = [0u8; 8];
        getrandom::fill(&mut raw).map_err(internal)?;
        let id = hex::encode(raw);
        let c = Comment {
            thread: if r.thread.is_empty() { id.clone() } else { r.thread },
            id,
            author: who.0.clone(),
            body,
            created_at_ms: now_ms(),
            edited_at_ms: 0,
            path: r.path,
            line: r.line,
            on_base: r.on_base,
            at_tip: cr.source_tip.clone(),
            resolved: false,
            suggestion: r.suggestion,
            end_line: r.end_line,
        };
        self.reg().add_comment(&key, cr.number, &c).map_err(internal)?;
        cr.updated_at_ms = now_ms();
        self.reg().update_change_request(&cr).map_err(internal)?;
        self.event(EventKind::CommentedOn, &who, &cr, &[], &c.path);
        Ok(Response::new(c))
    }

    async fn resolve_thread(&self, req: Request<ResolveThreadRequest>) -> RpcResult<Comment> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let cref = r.r#ref.ok_or_else(|| Status::invalid_argument("name the change request"))?;
        let key = key_of(cref.repository.as_ref())?;
        let ctx = self.ctx(&who, scope.as_ref(), &key, Role::Read).await?;
        let cr = self.get_cr(&key, cref.number)?;
        let comments = self.reg().comments(&key, cr.number).map_err(internal)?;
        let head = comments
            .iter()
            .find(|c| c.id == r.thread)
            .ok_or_else(|| Status::not_found(format!("no thread `{}`", r.thread)))?;
        if head.author != who.0 && !self.may_edit(&who, &ctx, &cr, Role::Triage) {
            return Err(Status::permission_denied(
                "only the change's author, the thread's author or someone with triage access may resolve it",
            ));
        }
        self.reg().resolve_thread(&key, cr.number, &r.thread, r.resolved).map_err(internal)?;
        Ok(Response::new(Comment { resolved: r.resolved, ..head.clone() }))
    }

    async fn submit_review(&self, req: Request<SubmitReviewRequest>) -> RpcResult<ChangeRequest> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let cref = r.r#ref.ok_or_else(|| Status::invalid_argument("name the change request"))?;
        let key = key_of(cref.repository.as_ref())?;
        let verdict = Verdict::try_from(r.verdict).unwrap_or(Verdict::Unspecified);
        let need = if verdict == Verdict::Commented { Role::Read } else { Role::Write };
        let ctx = self.ctx(&who, scope.as_ref(), &key, need).await?;
        let mut cr = self.refresh(&ctx.repo, self.get_cr(&key, cref.number)?).await?;
        if !matches!(cr.state, ChangeRequestState::Open | ChangeRequestState::Draft) {
            return Err(Status::failed_precondition(format!("#{} is no longer open", cr.number)));
        }
        match verdict {
            Verdict::Unspecified => return Err(Status::invalid_argument("approve, request changes or comment")),
            Verdict::Approved | Verdict::ChangesRequested if who.0 == cr.author => {
                return Err(Status::failed_precondition("you can't review your own change; ask someone else"));
            }
            _ => {}
        }
        if !r.body.trim().is_empty() {
            let mut raw = [0u8; 8];
            getrandom::fill(&mut raw).map_err(internal)?;
            let id = hex::encode(raw);
            let c = Comment {
                thread: id.clone(),
                id,
                author: who.0.clone(),
                body: r.body.trim().to_owned(),
                created_at_ms: now_ms(),
                at_tip: cr.source_tip.clone(),
                ..Default::default()
            };
            self.reg().add_comment(&key, cr.number, &c).map_err(internal)?;
        }
        if verdict != Verdict::Commented {
            self.reg()
                .set_review(
                    &key,
                    cr.number,
                    &ReviewRec {
                        reviewer: who.0.clone(),
                        verdict,
                        at_ms: now_ms(),
                        reviewed_tip: cr.source_tip.clone(),
                    },
                )
                .map_err(internal)?;
        }
        cr.updated_at_ms = now_ms();
        self.reg().update_change_request(&cr).map_err(internal)?;
        let word = match verdict {
            Verdict::Approved => "approved",
            Verdict::ChangesRequested => "requested changes",
            _ => "commented",
        };
        self.event(EventKind::Reviewed, &who, &cr, &[], word);
        audit("review", &who, &format!("{key}#{} {word}", cr.number));
        Ok(Response::new(self.view(&ctx.repo, cr).await?))
    }

    async fn land(&self, req: Request<ChangeRequestRef>) -> RpcResult<LandChangeRequestResponse> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let key = key_of(r.repository.as_ref())?;
        let ctx = self.ctx(&who, scope.as_ref(), &key, Role::Write).await?;
        let mut cr = self.refresh(&ctx.repo, self.get_cr(&key, r.number)?).await?;
        if cr.state != ChangeRequestState::Open {
            return Err(Status::failed_precondition(match cr.state {
                ChangeRequestState::Draft => format!("#{} is a draft; mark it ready first", cr.number),
                _ => format!("#{} is not open", cr.number),
            }));
        }
        let view = self.view(&ctx.repo, cr.clone()).await?;
        if !view.blockers.is_empty() {
            return Err(Status::failed_precondition(format!(
                "#{} can't land yet: {}",
                cr.number,
                view.blockers.join("; ")
            )));
        }
        let land = ctx.land.as_ref().ok_or_else(|| Status::unimplemented("this server can't land change requests"))?;
        // One changeset with everything the branch changed since its base, landed like any
        // other: the server rebases it onto the target's tip, file by file.
        let base = id_from(&cr.base)?;
        let changes = changes_between(&ctx.repo, base, id_from(&cr.source_tip)?).await?;
        let approvers: Vec<String> =
            view.reviews.iter().filter(|r| r.verdict == Verdict::Approved as i32).map(|r| r.reviewer.clone()).collect();
        let mut message = format!("{} (#{})", cr.title, cr.number);
        if !cr.body.trim().is_empty() {
            message.push_str(&format!("\n\n{}", cr.body.trim()));
        }
        if !approvers.is_empty() {
            message.push_str(&format!("\n\nApproved by {}.", approvers.join(", ")));
        }
        let cs = Changeset {
            parents: vec![base.0.to_vec()],
            changes,
            author: cr.author.clone(),
            message,
            timestamp_ms: now_ms(),
        };
        let id = ctx.repo.create_changeset(cs).await.map_err(to_status)?;
        let landed = land.land(&who, &cr.target, id).await.map_err(to_status)?;
        cr.state = ChangeRequestState::Landed;
        cr.landed_as = landed.id.0.to_vec();
        cr.updated_at_ms = now_ms();
        self.reg().update_change_request(&cr).map_err(internal)?;
        self.event(EventKind::ChangeRequestLanded, &who, &cr, &cr.landed_as.clone(), &cr.target.clone());
        audit("land_change_request", &who, &format!("{key}#{} as {}", cr.number, landed.id));
        let landed_as = cr.landed_as.clone();
        Ok(Response::new(LandChangeRequestResponse {
            change_request: Some(self.view(&ctx.repo, cr).await?),
            landed_as,
        }))
    }

    async fn rebase(&self, req: Request<ChangeRequestRef>) -> RpcResult<ChangeRequest> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let key = key_of(r.repository.as_ref())?;
        // Rebasing rewrites the source branch, so it takes write access like any push.
        let ctx = self.ctx(&who, scope.as_ref(), &key, Role::Write).await?;
        let mut cr = self.refresh(&ctx.repo, self.get_cr(&key, r.number)?).await?;
        if self.reg().protection_for(&key, &cr.source).map_err(internal)?.is_some() {
            return Err(Status::failed_precondition(format!(
                "`{}` is protected; rebase a branch of your own instead",
                cr.source
            )));
        }
        if !matches!(cr.state, ChangeRequestState::Open | ChangeRequestState::Draft) {
            return Err(Status::failed_precondition(format!("#{} is not open", cr.number)));
        }
        let clashes = conflicts(&ctx.repo, &cr).await?;
        if !clashes.is_empty() {
            return Err(Status::failed_precondition(format!(
                "the target changed the same files: {}. Update the branch by hand, then push it",
                clashes.join(", ")
            )));
        }
        let target = ctx
            .repo
            .refs
            .get(&cr.target)
            .await
            .map_err(to_status)?
            .ok_or_else(|| Status::failed_precondition(format!("there is no branch `{}`", cr.target)))?;
        if target.0.as_slice() == cr.base.as_slice() {
            return Ok(Response::new(self.view(&ctx.repo, cr).await?));
        }
        // The branch's changes, replayed as one changeset on the target's tip.
        let old_tip = id_from(&cr.source_tip)?;
        let changes = changes_between(&ctx.repo, id_from(&cr.base)?, old_tip).await?;
        let cs = Changeset {
            parents: vec![target.0.to_vec()],
            changes,
            author: cr.author.clone(),
            message: format!("{} (#{}, rebased)", cr.title, cr.number),
            timestamp_ms: now_ms(),
        };
        let id = ctx.repo.create_changeset(cs).await.map_err(to_status)?;
        if !ctx.repo.refs.compare_and_swap(&cr.source, Some(old_tip), id).await.map_err(to_status)? {
            return Err(Status::aborted(format!("`{}` moved while rebasing; try again", cr.source)));
        }
        cr.source_tip = id.0.to_vec();
        cr.base = target.0.to_vec();
        cr.updated_at_ms = now_ms();
        self.reg().update_change_request(&cr).map_err(internal)?;
        audit("rebase_change_request", &who, &format!("{key}#{}", cr.number));
        Ok(Response::new(self.view(&ctx.repo, cr).await?))
    }

    async fn report_check(&self, req: Request<ReportCheckRequest>) -> RpcResult<ChangeRequest> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let cref = r.r#ref.ok_or_else(|| Status::invalid_argument("name the change request"))?;
        let key = key_of(cref.repository.as_ref())?;
        let ctx = self.ctx(&who, scope.as_ref(), &key, Role::Write).await?;
        let cr = self.refresh(&ctx.repo, self.get_cr(&key, cref.number)?).await?;
        let mut check = r.check.ok_or_else(|| Status::invalid_argument("give the check"))?;
        check.name = check.name.trim().to_owned();
        if check.name.is_empty() || check.name.len() > 100 {
            return Err(Status::invalid_argument("give the check a name of up to 100 characters"));
        }
        if CheckState::try_from(check.state).unwrap_or(CheckState::Unspecified) == CheckState::Unspecified {
            return Err(Status::invalid_argument("a check is pending, passed, failed or skipped"));
        }
        if !check.details_url.is_empty()
            && !check.details_url.starts_with("https://")
            && !check.details_url.starts_with("http://")
        {
            return Err(Status::invalid_argument("a details link must be an http(s) URL"));
        }
        self.reg().set_check(&key, &cr.source_tip, &check).map_err(internal)?;
        let mut p = webhooks::envelope("check", "reported", &key, &who.0);
        p["change_request"] = cr_json(&cr);
        p["check"] = serde_json::json!({ "name": check.name, "state": format!("{:?}", CheckState::try_from(check.state).unwrap_or(CheckState::Pending)).to_lowercase(), "summary": check.summary, "details_url": check.details_url });
        webhooks::emit(self.reg(), &key, "check", p);
        Ok(Response::new(self.view(&ctx.repo, cr).await?))
    }

    async fn list_protections(&self, req: Request<RepositoryRef>) -> RpcResult<ListProtectionsResponse> {
        let (who, scope) = caller(&req);
        let key = key_of(Some(req.get_ref()))?;
        self.hosting.authorize(&key, &who, scope.as_ref(), Role::Read)?;
        Ok(Response::new(ListProtectionsResponse { protections: self.reg().protections(&key).map_err(internal)? }))
    }

    async fn set_protection(&self, req: Request<Protection>) -> RpcResult<Protection> {
        let (who, scope) = caller(&req);
        let p = req.into_inner();
        let key = key_of(p.repository.as_ref())?;
        self.hosting.authorize(&key, &who, scope.as_ref(), Role::Admin)?;
        let pattern = p.pattern.trim();
        let plain = pattern.strip_suffix('*').unwrap_or(pattern);
        if pattern.is_empty()
            || plain.contains('*')
            || (!plain.is_empty() && !plain.ends_with('/') && pando_core::validate_ref_name(plain).is_err())
        {
            return Err(Status::invalid_argument("protect a branch (`main`) or a prefix (`release/*`, `*`)"));
        }
        if p.required_approvals > 10 {
            return Err(Status::invalid_argument("require at most 10 approvals"));
        }
        let clean = |v: &[String]| {
            v.iter().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty() && !s.contains(',')).collect::<Vec<_>>()
        };
        let p = Protection {
            pattern: pattern.to_owned(),
            required_checks: clean(&p.required_checks),
            bypass: clean(&p.bypass),
            ..p
        };
        self.reg().set_protection(&key, &p).map_err(internal)?;
        audit("set_protection", &who, &format!("{key}:{}", p.pattern));
        Ok(Response::new(p))
    }

    async fn delete_protection(&self, req: Request<ProtectionRef>) -> RpcResult<Empty> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let key = key_of(r.repository.as_ref())?;
        self.hosting.authorize(&key, &who, scope.as_ref(), Role::Admin)?;
        if !self.reg().delete_protection(&key, &r.pattern).map_err(internal)? {
            return Err(Status::not_found(format!("`{}` is not protected", r.pattern)));
        }
        audit("delete_protection", &who, &format!("{key}:{}", r.pattern));
        Ok(Response::new(Empty {}))
    }

    async fn inbox(&self, req: Request<InboxRequest>) -> RpcResult<InboxResponse> {
        let (who, scope) = caller(&req);
        let limit = if req.get_ref().limit == 0 { 50 } else { req.get_ref().limit.min(200) } as usize;
        let mut items = Vec::new();
        let open = self.reg().change_requests(None, &[ChangeRequestState::Open], usize::MAX).map_err(internal)?;
        for cr in open {
            let mine = cr.author == who.0 || self.answers_for(&who, &cr.author);
            let asked = cr.reviewers.contains(&who.0);
            // Nobody asked: anyone who may approve it sees it.
            let unassigned = cr.reviewers.is_empty() && !mine;
            if !mine && !asked && !unassigned {
                continue;
            }
            let Ok(ctx) = self.ctx(&who, scope.as_ref(), &cr.key, Role::Read).await else { continue };
            if unassigned && ctx.role < Role::Write {
                continue;
            }
            let cr = self.refresh(&ctx.repo, cr).await?;
            let view = self.view(&ctx.repo, cr.clone()).await?;
            let reviewed = view.reviews.iter().any(|r| r.reviewer == who.0 && r.reviewed_tip == view.source_tip);
            let needs_approval = view.blockers.iter().any(|b| b.contains("approval"));
            let reason = if asked && !reviewed {
                "Review requested"
            } else if unassigned && needs_approval && !reviewed {
                "Needs a reviewer"
            } else if mine && view.reviews.iter().any(|r| r.verdict == Verdict::ChangesRequested as i32) {
                "Changes requested"
            } else if mine && view.blockers.is_empty() && kind_of(&cr.author) == AccountKind::Agent {
                "Ready to land"
            } else {
                continue;
            };
            items.push(InboxItem { change_request: Some(view), reason: reason.to_owned(), unread: false });
            if items.len() == limit {
                break;
            }
        }
        Ok(Response::new(InboxResponse { items }))
    }

    async fn list_events(&self, req: Request<ListEventsRequest>) -> RpcResult<ListEventsResponse> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let limit = if r.limit == 0 { 50 } else { r.limit.min(200) } as usize;
        let before: i64 = r.page_token.parse().unwrap_or(0);
        let key = match &r.repository {
            Some(repo) if !repo.owner.is_empty() => {
                let key = RepoKey::new(&repo.owner, &repo.name)?;
                self.hosting.authorize(&key, &who, scope.as_ref(), Role::Read)?;
                Some(key)
            }
            _ => None,
        };
        let mut events: Vec<Event> = Vec::new();
        let mut cursor = before;
        // Across repositories, keep only what the caller may read.
        while events.len() < limit {
            let page = self.reg().events(key.as_ref(), &r.actor, cursor, limit * 2).map_err(internal)?;
            let Some((last, _)) = page.last() else { break };
            cursor = *last;
            for (_, e) in page {
                let k = e.repository.as_ref().and_then(|x| RepoKey::new(&x.owner, &x.name).ok());
                if key.is_some()
                    || k.is_some_and(|k| self.hosting.authorize(&k, &who, scope.as_ref(), Role::Read).is_ok())
                {
                    events.push(e);
                }
            }
        }
        events.truncate(limit);
        let next =
            if events.len() == limit { events.last().map(|e| e.id.clone()).unwrap_or_default() } else { String::new() };
        Ok(Response::new(ListEventsResponse { events, next_page_token: next }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pando_proto::Check;

    fn cr(state: ChangeRequestState) -> CrRec {
        CrRec {
            key: RepoKey::parse("lumen/fleet").unwrap(),
            number: 1,
            title: "t".into(),
            body: String::new(),
            author: "agent/x".into(),
            state,
            source: "f".into(),
            target: "main".into(),
            source_tip: vec![2],
            base: vec![1],
            created_at_ms: 0,
            updated_at_ms: 0,
            reviewers: vec![],
            labels: vec![],
            landed_as: vec![],
            from_protected_land: false,
            files: 1,
            additions: 1,
            deletions: 0,
        }
    }

    fn review(who: &str, verdict: Verdict, tip: u8) -> ReviewRec {
        ReviewRec { reviewer: who.into(), verdict, at_ms: 0, reviewed_tip: vec![tip] }
    }

    #[test]
    fn blockers_say_what_is_missing() {
        let p = Protection {
            pattern: "main".into(),
            required_approvals: 2,
            required_checks: vec!["build".into(), "test".into()],
            dismiss_stale: true,
            ..Default::default()
        };
        let checks = vec![
            Check { name: "build".into(), state: CheckState::Passed as i32, ..Default::default() },
            Check { name: "test".into(), state: CheckState::Failed as i32, ..Default::default() },
        ];
        // One current approval, one stale; the author's own approval never counts.
        let reviews = vec![
            review("mira", Verdict::Approved, 2),
            review("dev", Verdict::Approved, 1),
            review("agent/x", Verdict::Approved, 2),
        ];
        let b = blockers(&cr(ChangeRequestState::Open), &reviews, &checks, &["a.rs".into()], Some(&p));
        assert_eq!(b, vec!["Needs 1 more approval", "Check test failed", "Conflicts in 1 file"]);

        let b =
            blockers(&cr(ChangeRequestState::Draft), &[review("mira", Verdict::ChangesRequested, 2)], &[], &[], None);
        assert_eq!(b, vec!["Draft: mark it ready for review", "Changes requested by mira"]);

        let ok = vec![review("mira", Verdict::Approved, 2), review("dev", Verdict::Approved, 2)];
        let green = vec![
            Check { name: "build".into(), state: CheckState::Passed as i32, ..Default::default() },
            Check { name: "test".into(), state: CheckState::Skipped as i32, ..Default::default() },
        ];
        assert!(blockers(&cr(ChangeRequestState::Open), &ok, &green, &[], Some(&p)).is_empty());
    }
}
