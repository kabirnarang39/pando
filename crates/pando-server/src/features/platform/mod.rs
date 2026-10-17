//! Platform slice, phase 1: the repositories a server hosts, and the organisations that own
//! them. Accounts, teams, tokens and access grants come with phase 2 and answer Unimplemented
//! until then, which the web console understands.

use std::collections::BTreeMap;
use std::sync::Arc;

use pando_proto::platform_server::{self, PlatformServer};
use pando_proto::{
    Account, AccountKind, CreateOrganizationRequest, CreateRepositoryRequest, CreateTokenRequest, CreateTokenResponse,
    Empty, Grant, ListGrantsResponse, ListMembersResponse, ListOrganizationsResponse, ListRepositoriesRequest,
    ListRepositoriesResponse, ListTeamsResponse, ListTokensResponse, Member, OrgName, Organization, Repository,
    RepositoryRef, RevokeTokenRequest, Role, SetGrantRequest, SetMemberRequest, UpdateRepositoryRequest,
    ViewerResponse, Visibility,
};
use tonic::{Request, Response, Status};

use crate::auth::principal;
use crate::hosting::{Hosting, RepoKey, RepoRecord, now_ms};
use crate::status::RpcResult;

#[derive(Clone)]
pub struct Platform {
    hosting: Arc<Hosting>,
}

impl Platform {
    pub fn new(hosting: Arc<Hosting>) -> Self {
        Self { hosting }
    }

    async fn repository(&self, rec: &RepoRecord) -> Repository {
        let s = self.hosting.stats(rec).await;
        Repository {
            owner: rec.key.owner.clone(),
            name: rec.key.name.clone(),
            description: rec.description.clone(),
            visibility: rec.visibility,
            default_branch: rec.default_branch.clone(),
            created_at_ms: rec.created_at_ms,
            // Phase 1: every authenticated principal reads every repository (the server's access
            // rules still apply to paths); per-repository roles arrive with accounts.
            viewer_role: Role::Write as i32,
            archived: rec.archived,
            lands_24h: s.lands_24h,
            open_change_requests: 0,
            tip: s.tip.map(|t| t.0.to_vec()).unwrap_or_default(),
            tip_at_ms: s.tip_at_ms,
            topics: rec.topics.clone(),
        }
    }

    fn records(&self, owner: Option<&str>) -> Result<Vec<RepoRecord>, Status> {
        self.hosting.registry.list(owner).map_err(|e| Status::internal(e.to_string()))
    }

    fn organizations(&self) -> Result<Vec<Organization>, Status> {
        let mut by_owner: BTreeMap<String, u32> = BTreeMap::new();
        for r in self.records(None)? {
            *by_owner.entry(r.key.owner).or_default() += 1;
        }
        Ok(by_owner
            .into_iter()
            .map(|(login, repositories)| Organization {
                display_name: login.clone(),
                login,
                description: String::new(),
                created_at_ms: 0,
                viewer_role: Role::Write as i32,
                repositories,
                members: 0,
                agents: 0,
            })
            .collect())
    }
}

pub fn service(s: Platform) -> PlatformServer<Platform> {
    PlatformServer::new(s)
}

fn later(what: &str) -> Status {
    Status::unimplemented(format!("{what} arrive with accounts and teams (platform phase 2)"))
}

fn account(login: &str) -> Account {
    let agent = login.starts_with("agent/") || login.starts_with("bot/");
    Account {
        login: login.to_owned(),
        display_name: login.rsplit('/').next().unwrap_or(login).to_owned(),
        kind: if agent { AccountKind::Agent } else { AccountKind::Person } as i32,
        owner: String::new(),
        avatar_url: String::new(),
        created_at_ms: 0,
        site_admin: false,
    }
}

#[tonic::async_trait]
impl platform_server::Platform for Platform {
    async fn viewer(&self, req: Request<Empty>) -> RpcResult<ViewerResponse> {
        let who = principal(&req);
        let mut me = account(&who.0);
        me.site_admin = self.hosting.admins.iter().any(|a| a == &who.0);
        Ok(Response::new(ViewerResponse { account: Some(me), organizations: self.organizations()? }))
    }

    async fn get_account(&self, req: Request<OrgName>) -> RpcResult<Account> {
        Ok(Response::new(account(&req.into_inner().login)))
    }

    async fn list_organizations(&self, _: Request<Empty>) -> RpcResult<ListOrganizationsResponse> {
        Ok(Response::new(ListOrganizationsResponse { organizations: self.organizations()? }))
    }

    async fn get_organization(&self, req: Request<OrgName>) -> RpcResult<Organization> {
        let login = req.into_inner().login;
        self.organizations()?
            .into_iter()
            .find(|o| o.login == login)
            .map(Response::new)
            .ok_or_else(|| Status::not_found(format!("no organisation `{login}` on this server")))
    }

    async fn create_organization(&self, _: Request<CreateOrganizationRequest>) -> RpcResult<Organization> {
        Err(Status::unimplemented("an organisation is created with its first repository (platform phase 1)"))
    }

    async fn list_members(&self, _: Request<OrgName>) -> RpcResult<ListMembersResponse> {
        Err(later("members"))
    }

    async fn set_member(&self, _: Request<SetMemberRequest>) -> RpcResult<Member> {
        Err(later("members"))
    }

    async fn list_teams(&self, _: Request<OrgName>) -> RpcResult<ListTeamsResponse> {
        Err(later("teams"))
    }

    async fn list_repositories(&self, req: Request<ListRepositoriesRequest>) -> RpcResult<ListRepositoriesResponse> {
        let r = req.into_inner();
        let q = r.query.to_lowercase();
        let limit = if r.limit == 0 { 200 } else { r.limit.min(1000) } as usize;
        let mut out = Vec::new();
        for rec in self.records((!r.owner.is_empty()).then_some(r.owner.as_str()))? {
            if !q.is_empty()
                && !rec.key.name.to_lowercase().contains(&q)
                && !rec.description.to_lowercase().contains(&q)
            {
                continue;
            }
            out.push(self.repository(&rec).await);
            if out.len() == limit {
                break;
            }
        }
        Ok(Response::new(ListRepositoriesResponse { repositories: out, next_page_token: String::new() }))
    }

    async fn get_repository(&self, req: Request<RepositoryRef>) -> RpcResult<Repository> {
        let r = req.into_inner();
        let key = RepoKey::new(&r.owner, &r.name)?;
        let rec = self
            .hosting
            .registry
            .get(&key)
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("no repository `{key}` on this server")))?;
        Ok(Response::new(self.repository(&rec).await))
    }

    async fn create_repository(&self, req: Request<CreateRepositoryRequest>) -> RpcResult<Repository> {
        let who = principal(&req);
        if !self.hosting.admins.iter().any(|a| a == &who.0) {
            return Err(Status::permission_denied(format!(
                "`{}` may not create repositories; add it to --platform-admins",
                who.0
            )));
        }
        let r = req.into_inner();
        let key = RepoKey::new(&r.owner, &r.name)?;
        let branch = if r.default_branch.is_empty() { "main".to_owned() } else { r.default_branch };
        if branch.starts_with('_') || branch.contains("..") || branch.contains(char::is_whitespace) {
            return Err(Status::invalid_argument(format!("`{branch}` is not a valid branch name")));
        }
        let visibility =
            if r.visibility == Visibility::Unspecified as i32 { Visibility::Private as i32 } else { r.visibility };
        let rec = RepoRecord {
            key: key.clone(),
            description: r.description,
            visibility,
            default_branch: branch,
            created_at_ms: now_ms(),
            archived: false,
            topics: Vec::new(),
        };
        if !self.hosting.create(rec.clone()).await? {
            return Err(Status::already_exists(format!("`{key}` already exists")));
        }
        tracing::info!(target: crate::telemetry::AUDIT, kind = "create_repository", principal = %who.0, repo = %key);
        Ok(Response::new(self.repository(&rec).await))
    }

    async fn update_repository(&self, _: Request<UpdateRepositoryRequest>) -> RpcResult<Repository> {
        Err(later("repository settings"))
    }

    async fn list_grants(&self, _: Request<RepositoryRef>) -> RpcResult<ListGrantsResponse> {
        Err(later("access grants"))
    }

    async fn set_grant(&self, _: Request<SetGrantRequest>) -> RpcResult<Grant> {
        Err(later("access grants"))
    }

    async fn list_tokens(&self, _: Request<Empty>) -> RpcResult<ListTokensResponse> {
        Err(later("tokens"))
    }

    async fn create_token(&self, _: Request<CreateTokenRequest>) -> RpcResult<CreateTokenResponse> {
        Err(later("tokens"))
    }

    async fn revoke_token(&self, _: Request<RevokeTokenRequest>) -> RpcResult<Empty> {
        Err(later("tokens"))
    }
}
