//! Issues (ADR-027): work to do in a repository, assigned to people or agents. An issue and a
//! change request never share a number, so `#12` names one thing; a change request that says
//! "fixes #12" closes the issue when it lands.

use std::sync::Arc;

use pando_core::acl::Principal;
use pando_proto::issues_server::{self, IssuesServer};
use pando_proto::{
    AddIssueCommentRequest, ChangeRequestState, Comment, CreateIssueRequest, EventKind, Issue, IssueRef, IssueState,
    ListCommentsResponse, ListIssuesRequest, ListIssuesResponse, RepositoryRef, Role, UpdateIssueRequest,
};
use serde_json::json;
use tonic::{Request, Response, Status};

use crate::auth::{TokenScope, principal};
use crate::directory::check_login;
use crate::hosting::{Hosting, Registry, RepoKey, now_ms};
use crate::status::RpcResult;
use crate::webhooks;

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS issues (
  owner TEXT NOT NULL,
  name TEXT NOT NULL,
  number INTEGER NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL DEFAULT '',
  author TEXT NOT NULL,
  state INTEGER NOT NULL,
  labels TEXT NOT NULL DEFAULT '',
  assignees TEXT NOT NULL DEFAULT '',
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  closed_at_ms INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (owner, name, number)
);
CREATE INDEX IF NOT EXISTS issues_by_state ON issues (owner, name, state);
";

fn list(s: String) -> Vec<String> {
    s.split(',').filter(|t| !t.is_empty()).map(str::to_owned).collect()
}

fn clean(v: &[String]) -> Vec<String> {
    let mut out: Vec<String> =
        v.iter().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty() && !s.contains(',')).collect();
    out.sort();
    out.dedup();
    out
}

impl Registry {
    /// The next number in a repository, shared by issues and change requests. Call inside the
    /// transaction that uses it.
    pub(crate) fn next_number(tx: &rusqlite::Transaction<'_>, key: &RepoKey) -> rusqlite::Result<i64> {
        tx.query_row(
            "SELECT COALESCE(MAX(n), 0) + 1 FROM (
               SELECT MAX(number) AS n FROM change_requests WHERE owner = ?1 AND name = ?2
               UNION ALL SELECT MAX(number) FROM issues WHERE owner = ?1 AND name = ?2)",
            (&key.owner, &key.name),
            |r| r.get(0),
        )
    }

    pub fn create_issue(&self, key: &RepoKey, issue: &Issue) -> anyhow::Result<u64> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let n = Self::next_number(&tx, key)?;
        tx.execute(
            "INSERT INTO issues (owner, name, number, title, body, author, state, labels, assignees, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
            rusqlite::params![
                key.owner,
                key.name,
                n,
                issue.title,
                issue.body,
                issue.author,
                issue.state,
                issue.labels.join(","),
                issue.assignees.join(","),
                issue.created_at_ms
            ],
        )?;
        tx.commit()?;
        Ok(n as u64)
    }

    pub fn update_issue(&self, key: &RepoKey, i: &Issue) -> anyhow::Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "UPDATE issues SET title = ?4, body = ?5, state = ?6, labels = ?7, assignees = ?8, updated_at_ms = ?9, closed_at_ms = ?10
             WHERE owner = ?1 AND name = ?2 AND number = ?3",
            rusqlite::params![
                key.owner,
                key.name,
                i.number as i64,
                i.title,
                i.body,
                i.state,
                i.labels.join(","),
                i.assignees.join(","),
                i.updated_at_ms,
                i.closed_at_ms
            ],
        )?;
        Ok(())
    }

    fn issue_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Issue> {
        Ok(Issue {
            repository: Some(RepositoryRef { owner: r.get(0)?, name: r.get(1)? }),
            number: r.get::<_, i64>(2)? as u64,
            title: r.get(3)?,
            body: r.get(4)?,
            author: r.get(5)?,
            state: r.get(6)?,
            labels: list(r.get(7)?),
            assignees: list(r.get(8)?),
            created_at_ms: r.get(9)?,
            updated_at_ms: r.get(10)?,
            closed_at_ms: r.get(11)?,
            comments: r.get::<_, i64>(12)? as u32,
            fixed_by: vec![],
        })
    }

    const ISSUE_SELECT: &str =
        "SELECT i.owner, i.name, i.number, i.title, i.body, i.author, i.state, i.labels, i.assignees,
         i.created_at_ms, i.updated_at_ms, i.closed_at_ms,
         (SELECT COUNT(*) FROM comments c WHERE c.owner = i.owner AND c.name = i.name AND c.number = i.number)
       FROM issues i";

    pub fn issue(&self, key: &RepoKey, number: u64) -> anyhow::Result<Option<Issue>> {
        let db = self.db.lock().unwrap();
        let mut st =
            db.prepare_cached(&format!("{} WHERE i.owner = ?1 AND i.name = ?2 AND i.number = ?3", Self::ISSUE_SELECT))?;
        let mut rows = st.query_map((&key.owner, &key.name, number as i64), Self::issue_row)?;
        Ok(rows.next().transpose()?)
    }

    /// Issues of one repository, or of every repository, newest activity first; at most `scan`
    /// rows are read (M6).
    pub fn issues(&self, key: Option<&RepoKey>, states: &[i32], scan: usize) -> anyhow::Result<Vec<Issue>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(&format!(
            "{} WHERE (?1 IS NULL OR (i.owner = ?1 AND i.name = ?2)) ORDER BY i.updated_at_ms DESC LIMIT ?3",
            Self::ISSUE_SELECT
        ))?;
        let rows = st.query_map((key.map(|k| &k.owner), key.map(|k| &k.name), scan as i64), Self::issue_row)?;
        let mut out = Vec::new();
        for r in rows {
            let r = r?;
            if states.contains(&r.state) {
                out.push(r);
            }
        }
        Ok(out)
    }
}

/// Issue numbers a change request says it fixes: "fixes #12", "closes #3", "resolves #7".
pub fn fixes(text: &str) -> Vec<u64> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    for word in ["fix", "fixes", "fixed", "close", "closes", "closed", "resolve", "resolves", "resolved"] {
        let mut rest = lower.as_str();
        while let Some(i) = rest.find(word) {
            let before = rest[..i].chars().last();
            let after = &rest[i + word.len()..];
            rest = after;
            if before.is_some_and(|c| c.is_alphanumeric()) {
                continue;
            }
            let after = after.trim_start_matches([' ', ':']);
            if let Some(num) = after.strip_prefix('#') {
                let digits: String = num.chars().take_while(char::is_ascii_digit).collect();
                if let Ok(n) = digits.parse::<u64>()
                    && !out.contains(&n)
                {
                    out.push(n);
                }
            }
        }
    }
    out.sort();
    out
}

#[derive(Clone)]
pub struct Issues {
    hosting: Arc<Hosting>,
}

pub fn service(s: Issues) -> IssuesServer<Issues> {
    IssuesServer::new(s)
}

use crate::status::internal;

fn caller<T>(req: &Request<T>) -> (Principal, Option<TokenScope>) {
    (principal(req), req.extensions().get::<TokenScope>().cloned())
}

fn key_of(r: Option<&RepositoryRef>) -> Result<RepoKey, Status> {
    let r = r.ok_or_else(|| Status::invalid_argument("name the repository"))?;
    RepoKey::new(&r.owner, &r.name)
}

fn state_word(s: i32) -> &'static str {
    match IssueState::try_from(s).unwrap_or(IssueState::Unspecified) {
        IssueState::Open => "open",
        IssueState::Completed => "completed",
        IssueState::NotPlanned => "not planned",
        IssueState::Unspecified => "unknown",
    }
}

impl Issues {
    pub fn new(hosting: Arc<Hosting>) -> Self {
        Self { hosting }
    }

    fn reg(&self) -> &Registry {
        &self.hosting.registry
    }

    /// The role `who` holds, after checking at least `need`.
    fn role(&self, who: &Principal, scope: Option<&TokenScope>, key: &RepoKey, need: Role) -> Result<Role, Status> {
        let rec = self.hosting.authorize(key, who, scope, need)?;
        self.hosting.access.effective(self.reg(), who, scope, &rec)
    }

    fn get(&self, key: &RepoKey, number: u64) -> Result<Issue, Status> {
        let mut i = self
            .reg()
            .issue(key, number)
            .map_err(internal)?
            .ok_or_else(|| Status::not_found(format!("no issue #{number} in {key}")))?;
        // ponytail: scans the repository's change requests per read; keep a link table if
        // repositories reach many thousands of change requests.
        i.fixed_by = self
            .reg()
            .change_requests(Some(key), &[], usize::MAX)
            .map_err(internal)?
            .into_iter()
            .filter(|c| {
                c.state != ChangeRequestState::Closed && fixes(&format!("{}\n{}", c.title, c.body)).contains(&number)
            })
            .map(|c| c.number)
            .collect();
        Ok(i)
    }

    fn emit(&self, key: &RepoKey, action: &str, who: &Principal, i: &Issue) {
        let mut p = webhooks::envelope("issue", action, key, &who.0);
        p["issue"] = json!({
            "number": i.number,
            "title": i.title,
            "author": i.author,
            "state": state_word(i.state),
            "labels": i.labels,
            "assignees": i.assignees,
        });
        webhooks::emit(self.reg(), key, "issue", p);
    }
}

/// Closes the issues a landed change request says it fixes. Called by the review slice.
pub fn close_fixed(reg: &Registry, key: &RepoKey, who: &Principal, cr_number: u64, text: &str) {
    for n in fixes(text) {
        let Ok(Some(mut i)) = reg.issue(key, n) else { continue };
        if i.state != IssueState::Open as i32 {
            continue;
        }
        i.state = IssueState::Completed as i32;
        i.closed_at_ms = now_ms();
        i.updated_at_ms = i.closed_at_ms;
        if let Err(e) = reg.update_issue(key, &i) {
            tracing::warn!(error = %e, "closing a fixed issue");
            continue;
        }
        let _ = reg.record_event(EventKind::IssueClosed, &who.0, key, n, &[], &format!("fixed by #{cr_number}"));
        let mut p = webhooks::envelope("issue", "closed", key, &who.0);
        p["issue"] = json!({ "number": n, "title": i.title, "state": "completed", "fixed_by": cr_number });
        webhooks::emit(reg, key, "issue", p);
    }
}

#[tonic::async_trait]
impl issues_server::Issues for Issues {
    async fn list_issues(&self, req: Request<ListIssuesRequest>) -> RpcResult<ListIssuesResponse> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let key = key_of(r.repository.as_ref())?;
        self.role(&who, scope.as_ref(), &key, Role::Read)?;
        let states = if r.states.is_empty() { vec![IssueState::Open as i32] } else { r.states };
        let q = r.query.to_lowercase();
        let limit = if r.limit == 0 { 100 } else { r.limit.min(500) } as usize;
        let issues = self
            .reg()
            .issues(Some(&key), &states, 5000)
            .map_err(internal)?
            .into_iter()
            .filter(|i| r.assignee.is_empty() || i.assignees.contains(&r.assignee))
            .filter(|i| r.label.is_empty() || i.labels.contains(&r.label))
            .filter(|i| q.is_empty() || i.title.to_lowercase().contains(&q) || i.body.to_lowercase().contains(&q))
            .take(limit)
            .collect();
        Ok(Response::new(ListIssuesResponse { issues }))
    }

    async fn get_issue(&self, req: Request<IssueRef>) -> RpcResult<Issue> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let key = key_of(r.repository.as_ref())?;
        self.role(&who, scope.as_ref(), &key, Role::Read)?;
        Ok(Response::new(self.get(&key, r.number)?))
    }

    async fn create_issue(&self, req: Request<CreateIssueRequest>) -> RpcResult<Issue> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let key = key_of(r.repository.as_ref())?;
        let role = self.role(&who, scope.as_ref(), &key, Role::Read)?;
        let title = r.title.trim().to_owned();
        if title.is_empty() || title.len() > 300 {
            return Err(Status::invalid_argument("give the issue a title of up to 300 characters"));
        }
        if r.body.len() > 65_536 {
            return Err(Status::invalid_argument("keep the description under 64 KB"));
        }
        let assignees = clean(&r.assignees);
        // Readers may open issues; assigning and labelling take triage, as on GitHub.
        if role < Role::Triage && (!assignees.is_empty() || !r.labels.is_empty()) {
            return Err(Status::permission_denied("assigning and labelling issues takes triage access"));
        }
        for a in &assignees {
            check_login(a)?;
        }
        let now = now_ms();
        let mut issue = Issue {
            repository: Some(RepositoryRef { owner: key.owner.clone(), name: key.name.clone() }),
            title,
            body: r.body,
            author: who.0.clone(),
            state: IssueState::Open as i32,
            labels: clean(&r.labels),
            assignees,
            created_at_ms: now,
            updated_at_ms: now,
            ..Default::default()
        };
        issue.number = self.reg().create_issue(&key, &issue).map_err(internal)?;
        let _ = self.reg().record_event(EventKind::IssueOpened, &who.0, &key, issue.number, &[], &issue.title);
        self.emit(&key, "opened", &who, &issue);
        if !issue.assignees.is_empty() {
            self.emit(&key, "assigned", &who, &issue);
        }
        Ok(Response::new(issue))
    }

    async fn update_issue(&self, req: Request<UpdateIssueRequest>) -> RpcResult<Issue> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let iref = r.r#ref.ok_or_else(|| Status::invalid_argument("name the issue"))?;
        let key = key_of(iref.repository.as_ref())?;
        let role = self.role(&who, scope.as_ref(), &key, Role::Read)?;
        let mut i = self.get(&key, iref.number)?;
        let author = i.author == who.0;
        let mut actions = Vec::new();
        for f in &r.fields {
            let triage = role >= Role::Triage;
            match f.as_str() {
                "title" | "body" if !author && !triage => {
                    return Err(Status::permission_denied("only its author or someone with triage access may edit it"));
                }
                "title" if !r.title.trim().is_empty() => i.title = r.title.trim().to_owned(),
                "body" => i.body = r.body.clone(),
                "state" if !author && !triage => {
                    return Err(Status::permission_denied(
                        "only its author or someone with triage access may close it",
                    ));
                }
                "state" => {
                    let to = IssueState::try_from(r.state).unwrap_or(IssueState::Unspecified);
                    if to == IssueState::Unspecified {
                        return Err(Status::invalid_argument("an issue is open, completed or not planned"));
                    }
                    if to as i32 != i.state {
                        actions.push(if to == IssueState::Open { "reopened" } else { "closed" });
                        i.closed_at_ms = if to == IssueState::Open { 0 } else { now_ms() };
                    }
                    i.state = to as i32;
                }
                "labels" | "assignees" if !triage => {
                    return Err(Status::permission_denied("assigning and labelling issues takes triage access"));
                }
                "labels" => i.labels = clean(&r.labels),
                "assignees" => {
                    let a = clean(&r.assignees);
                    for l in &a {
                        check_login(l)?;
                    }
                    if a.iter().any(|x| !i.assignees.contains(x)) {
                        actions.push("assigned");
                    }
                    i.assignees = a;
                }
                other => return Err(Status::invalid_argument(format!("`{other}` can't be changed"))),
            }
        }
        i.updated_at_ms = now_ms();
        self.reg().update_issue(&key, &i).map_err(internal)?;
        for a in actions {
            if a == "closed" {
                let _ =
                    self.reg().record_event(EventKind::IssueClosed, &who.0, &key, i.number, &[], state_word(i.state));
            }
            self.emit(&key, a, &who, &i);
        }
        Ok(Response::new(i))
    }

    async fn list_issue_comments(&self, req: Request<IssueRef>) -> RpcResult<ListCommentsResponse> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let key = key_of(r.repository.as_ref())?;
        self.role(&who, scope.as_ref(), &key, Role::Read)?;
        self.get(&key, r.number)?;
        Ok(Response::new(ListCommentsResponse { comments: self.reg().comments(&key, r.number).map_err(internal)? }))
    }

    async fn add_issue_comment(&self, req: Request<AddIssueCommentRequest>) -> RpcResult<Comment> {
        let (who, scope) = caller(&req);
        let r = req.into_inner();
        let iref = r.r#ref.ok_or_else(|| Status::invalid_argument("name the issue"))?;
        let key = key_of(iref.repository.as_ref())?;
        self.role(&who, scope.as_ref(), &key, Role::Read)?;
        let mut i = self.get(&key, iref.number)?;
        let body = r.body.trim().to_owned();
        if body.is_empty() || body.len() > 65_536 {
            return Err(Status::invalid_argument("a comment needs some text, up to 64 KB"));
        }
        let mut raw = [0u8; 8];
        getrandom::fill(&mut raw).map_err(internal)?;
        let id = hex::encode(raw);
        let c = Comment {
            thread: id.clone(),
            id,
            author: who.0.clone(),
            body,
            created_at_ms: now_ms(),
            ..Default::default()
        };
        self.reg().add_comment(&key, i.number, &c).map_err(internal)?;
        i.updated_at_ms = now_ms();
        self.reg().update_issue(&key, &i).map_err(internal)?;
        let mut p = webhooks::envelope("comment", "created", &key, &who.0);
        p["issue"] = json!({ "number": i.number, "title": i.title });
        p["comment"] = json!({ "id": c.id, "body": c.body });
        webhooks::emit(self.reg(), &key, "comment", p);
        Ok(Response::new(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_what_a_change_says_it_fixes() {
        assert_eq!(fixes("Fixes #12"), vec![12]);
        assert_eq!(fixes("This closes #3 and resolves: #7.\nAlso fixed #3"), vec![3, 7]);
        assert_eq!(fixes("prefixes #4, suffixes #5"), Vec::<u64>::new(), "only whole words");
        assert_eq!(fixes("fix #"), Vec::<u64>::new());
        assert_eq!(fixes("see #9"), Vec::<u64>::new());
    }

    #[test]
    fn issues_and_change_requests_share_numbers() {
        let reg = Registry::open("memory").unwrap();
        let key = RepoKey::parse("lumen/fleet").unwrap();
        let issue = |t: &str| Issue {
            title: t.into(),
            author: "kabir".into(),
            state: IssueState::Open as i32,
            created_at_ms: 1,
            ..Default::default()
        };
        assert_eq!(reg.create_issue(&key, &issue("one")).unwrap(), 1);
        let cr = crate::features::reviews::store::CrRec {
            key: key.clone(),
            number: 0,
            title: "Fixes #1".into(),
            body: String::new(),
            author: "dev".into(),
            state: ChangeRequestState::Open,
            source: "f".into(),
            target: "main".into(),
            source_tip: vec![1],
            base: vec![0],
            created_at_ms: 1,
            updated_at_ms: 1,
            reviewers: vec![],
            labels: vec![],
            landed_as: vec![],
            from_protected_land: false,
            files: 0,
            additions: 0,
            deletions: 0,
        };
        assert_eq!(reg.create_change_request(&cr).unwrap(), 2);
        assert_eq!(reg.create_issue(&key, &issue("three")).unwrap(), 3);
        close_fixed(&reg, &key, &Principal("dev".into()), 2, "Fixes #1, fixes #2");
        assert_eq!(reg.issue(&key, 1).unwrap().unwrap().state, IssueState::Completed as i32);
        assert_eq!(reg.issue(&key, 3).unwrap().unwrap().state, IssueState::Open as i32);
    }
}
