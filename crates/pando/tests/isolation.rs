//! Keeping tenants apart: repository calls must name their repository, and a multi-tenant server
//! (ADR-032) keeps internal repositories, organisations and webhooks to their own tenant.

use pando::remote::{GrpcRemote, Remote};
use pando_core::object::{Changeset, FileChange, Kind, Mode, encode_object};
use pando_proto::{
    CreateOrganizationRequest, CreateRepositoryRequest, CreateTokenRequest, CreateWebhookRequest, Empty, RepositoryRef,
    Role, SetMemberRequest, Visibility,
};
use pando_server::permissions::Access;
use tonic::Code;

mod common;
use common::{Server, start, start_with};

async fn multi_tenant() -> Server {
    start_with(Access {
        open: false,
        admins: vec!["admin".into()],
        multi_tenant: true,
        max_orgs_per_account: 3,
        webhook_allow_private: false,
        ..Default::default()
    })
    .await
}

fn as_(s: &Server, who: &str, repo: &str) -> GrpcRemote {
    let url = if repo.is_empty() { s.url.clone() } else { format!("{}/{repo}", s.url) };
    GrpcRemote::connect(&url, None, Some(&format!("{}-token", who.replace('/', "-")))).unwrap()
}

async fn changeset(r: &GrpcRemote, parent: Option<pando_core::Id>, body: &str) -> pando_core::Id {
    let (blob, payload) = encode_object(Kind::Blob, body.as_bytes());
    r.objects().put_many(vec![(blob, payload)]).await.unwrap();
    r.create_changeset(Changeset {
        parents: parent.iter().map(|p| p.0.to_vec()).collect(),
        changes: vec![FileChange::upsert("a.txt", Mode::File, blob)],
        author: "x".into(),
        message: "m".into(),
        timestamp_ms: 1,
    })
    .await
    .unwrap()
    .id
}

async fn create(s: &Server, name: &str, visibility: Visibility) {
    let (owner, name) = name.split_once('/').unwrap();
    as_(s, "admin", "")
        .platform()
        .create_repository(CreateRepositoryRequest {
            owner: owner.into(),
            name: name.into(),
            visibility: visibility as i32,
            ..Default::default()
        })
        .await
        .unwrap();
}

async fn member(s: &Server, org: &str, login: &str) -> Result<(), tonic::Status> {
    let m = SetMemberRequest { org: org.into(), login: login.into(), role: Role::Write as i32 };
    as_(s, "admin", "").platform().set_member(m).await.map(|_| ())
}

/// Signs in once, which records the account.
async fn sign_in(s: &Server, who: &str) {
    as_(s, who, "").platform().viewer(Empty {}).await.unwrap();
}

/// A repository call that names no repository is refused: it used to reach the server's own
/// repository with no access check at all.
#[tokio::test(flavor = "multi_thread")]
async fn repository_calls_must_name_a_repository() {
    let s = start().await;
    let eve = as_(&s, "eve", "");
    let (blob, payload) = encode_object(Kind::Blob, b"eve was here");
    assert!(eve.objects().put_many(vec![(blob, payload)]).await.is_err());
    assert!(eve.get_ref("main").await.is_err());

    // Nor does a token limited to reading one repository write anywhere else.
    create(&s, "lumen/web", Visibility::Private).await;
    let t = as_(&s, "admin", "")
        .platform()
        .create_token(CreateTokenRequest {
            name: "ro".into(),
            repositories: vec!["lumen/web".into()],
            max_role: Role::Read as i32,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let ro = GrpcRemote::connect(&s.url, None, Some(&t.secret)).unwrap();
    let e = ro.compare_and_swap("main", None, pando_core::Id([7; 32])).await.unwrap_err();
    assert!(format!("{e:#}").contains("name one"), "{e:#}");
    let web = GrpcRemote::connect(&format!("{}/lumen/web", s.url), None, Some(&t.secret)).unwrap();
    assert_eq!(web.get_ref("main").await.unwrap(), None, "named, it reads what it may");
}

#[tokio::test(flavor = "multi_thread")]
async fn internal_repositories_are_for_the_owning_organisation() {
    for mt in [true, false] {
        let s = if mt { multi_tenant().await } else { start().await };
        create(&s, "lumen/docs", Visibility::Internal).await;
        sign_in(&s, "dev").await;
        member(&s, "lumen", "dev").await.unwrap();
        let dev = as_(&s, "dev", "lumen/docs");
        let tip = changeset(&dev, None, "hello").await;
        assert!(dev.compare_and_swap("main", None, tip).await.unwrap());
        let eve = as_(&s, "eve", "lumen/docs").get_ref("main").await;
        if mt {
            assert!(format!("{:#}", eve.unwrap_err()).contains("no repository `lumen/docs`"));
        } else {
            assert_eq!(eve.unwrap(), Some(tip), "without --multi-tenant every account reads internal repositories");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn members_must_have_signed_in() {
    let s = multi_tenant().await;
    create(&s, "lumen/docs", Visibility::Private).await;
    let e = member(&s, "lumen", "stranger@example.com").await.unwrap_err();
    assert_eq!(e.code(), Code::FailedPrecondition);
    assert!(e.message().contains("has no Pando account yet"), "{e:?}");
    sign_in(&s, "mira").await;
    member(&s, "lumen", "mira").await.unwrap();
    member(&s, "lumen", "agent/new-01").await.unwrap();

    let s = start().await;
    create(&s, "lumen/docs", Visibility::Private).await;
    member(&s, "lumen", "stranger@example.com").await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn people_create_a_few_organisations_of_their_own() {
    let s = multi_tenant().await;
    let org = |login: &str| CreateOrganizationRequest { login: login.into(), ..Default::default() };
    let dev = as_(&s, "dev", "");
    for name in ["acme", "acme-labs", "acme-ops"] {
        let o = dev.platform().create_organization(org(name)).await.unwrap().into_inner();
        assert_eq!(o.viewer_role, Role::Admin as i32, "the creator administers {name}");
    }
    let e = dev.platform().create_organization(org("acme-four")).await.unwrap_err();
    assert_eq!(e.code(), Code::ResourceExhausted, "{e:?}");

    // Organisations and accounts share one namespace; some names belong to the service.
    sign_in(&s, "mira").await;
    let mira = as_(&s, "mira", "");
    assert_eq!(mira.platform().create_organization(org("dev")).await.unwrap_err().code(), Code::AlreadyExists);
    assert_eq!(mira.platform().create_organization(org("acme")).await.unwrap_err().code(), Code::AlreadyExists);
    for reserved in ["admin", "API", "pando", "-x"] {
        let e = mira.platform().create_organization(org(reserved)).await.unwrap_err();
        assert_eq!(e.code(), Code::InvalidArgument, "{reserved}: {e:?}");
    }
    let e = as_(&s, "agent/fix-01", "").platform().create_organization(org("bots")).await.unwrap_err();
    assert_eq!(e.code(), Code::PermissionDenied, "{e:?}");

    // Without --multi-tenant only site administrators create organisations.
    let s = start().await;
    let e = as_(&s, "dev", "").platform().create_organization(org("acme")).await.unwrap_err();
    assert_eq!(e.code(), Code::PermissionDenied);
}

#[tokio::test(flavor = "multi_thread")]
async fn webhooks_may_not_call_the_servers_own_network() {
    let hook = |url: &str| CreateWebhookRequest {
        repository: Some(RepositoryRef { owner: "lumen".into(), name: "fleet".into() }),
        url: url.into(),
        ..Default::default()
    };
    let refused = [
        "http://127.0.0.1:7420",
        "http://10.0.0.5",
        "http://[::1]",
        "http://[::ffff:127.0.0.1]",
        "http://169.254.169.254",
        "http://100.64.0.1",
        "http://localhost:7420/hook",
        "http://user:pw@example.com/",
        "file:///etc/passwd",
    ];
    // Multi-tenant, and a server left at its defaults: the same refusals.
    let default = start_with(Access { open: false, admins: vec!["admin".into()], ..Default::default() }).await;
    for s in [multi_tenant().await, default] {
        create(&s, "lumen/fleet", Visibility::Private).await;
        let admin = as_(&s, "admin", "");
        for url in refused {
            let e = admin.platform().create_webhook(hook(url)).await.unwrap_err();
            assert_eq!(e.code(), Code::InvalidArgument, "{url}: {e:?}");
        }
    }
    // --webhook-allow-private lets a self-hosted server call its own network, not metadata.
    let s = start().await;
    create(&s, "lumen/fleet", Visibility::Private).await;
    let admin = as_(&s, "admin", "");
    admin.platform().create_webhook(hook("http://127.0.0.1:7420")).await.unwrap();
    let e = admin.platform().create_webhook(hook("http://169.254.169.254")).await.unwrap_err();
    assert_eq!(e.code(), Code::InvalidArgument);
}

// ---- the cross-tenant suite (release gate) ----

/// Raw protobuf bytes in and out, so one client calls every method in the protocol.
#[derive(Clone, Copy, Default)]
struct Raw;

impl tonic::codec::Codec for Raw {
    type Encode = Vec<u8>;
    type Decode = Vec<u8>;
    type Encoder = Raw;
    type Decoder = Raw;
    fn encoder(&mut self) -> Raw {
        Raw
    }
    fn decoder(&mut self) -> Raw {
        Raw
    }
}

impl tonic::codec::Encoder for Raw {
    type Item = Vec<u8>;
    type Error = tonic::Status;
    fn encode(&mut self, item: Vec<u8>, dst: &mut tonic::codec::EncodeBuf<'_>) -> Result<(), tonic::Status> {
        bytes::BufMut::put_slice(dst, &item);
        Ok(())
    }
}

impl tonic::codec::Decoder for Raw {
    type Item = Vec<u8>;
    type Error = tonic::Status;
    fn decode(&mut self, src: &mut tonic::codec::DecodeBuf<'_>) -> Result<Option<Vec<u8>>, tonic::Status> {
        let n = bytes::Buf::remaining(src);
        Ok(Some(bytes::Buf::copy_to_bytes(src, n).to_vec()))
    }
}

/// Calls `method` (`/pando.v1.Service/Method`) with one message, as any kind of RPC: the wire is
/// the same. Returns every response message.
async fn call(
    ch: &tonic::transport::Channel,
    token: &str,
    repo: Option<&str>,
    method: &str,
    body: &[u8],
) -> Result<Vec<Vec<u8>>, tonic::Status> {
    let mut grpc = tonic::client::Grpc::new(ch.clone());
    grpc.ready().await.map_err(|e| tonic::Status::unavailable(e.to_string()))?;
    let mut req = tonic::Request::new(futures::stream::iter([body.to_vec()]));
    req.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    if let Some(r) = repo {
        req.metadata_mut().insert("x-pando-repo", r.parse().unwrap());
    }
    let path = tonic::codegen::http::uri::PathAndQuery::try_from(method).unwrap();
    let mut stream = grpc.streaming(req, path, Raw).await?.into_inner();
    let mut out = Vec::new();
    while let Some(m) = stream.message().await? {
        out.push(m);
    }
    Ok(out)
}

/// One call aimed at organisation A (`alpha`).
struct Probe {
    method: String,
    repo: Option<&'static str>,
    body: Vec<u8>,
    /// May succeed (a listing or the caller's own account), as long as nothing of A's is in it.
    ok: bool,
    /// Aimed at `alpha/vault`: a token limited to another of A's repositories is refused too.
    vault: bool,
    /// Another acceptable refusal, for a namespace shared by every tenant.
    also: Option<Code>,
}

fn probe(method: &str, msg: impl prost::Message) -> Probe {
    let body = msg.encode_to_vec();
    let vault = body.windows(5).any(|w| w == b"vault");
    Probe { method: format!("/pando.v1.{method}"), repo: None, body, ok: false, vault, also: None }
}

impl Probe {
    fn ok(mut self) -> Self {
        self.ok = true;
        self
    }
    fn vault(mut self) -> Self {
        self.vault = true;
        self
    }
}

/// Every method in the protocol, called with organisation B's tokens at organisation A's
/// repositories, change requests, issues, webhooks, protections, locks, teams and tokens: each is
/// refused as missing or forbidden, or answers with nothing of A's. A method missing from the
/// table fails the test, so no RPC ships unchecked. A token limited to one of A's repositories
/// reaches none of the others.
#[tokio::test(flavor = "multi_thread")]
async fn every_method_keeps_tenants_apart() {
    use pando_proto::*;
    let s = multi_tenant().await;
    let org = |login: &str| CreateOrganizationRequest { login: login.into(), ..Default::default() };
    let new_repo = |owner: &str, name: &str, v: Visibility| CreateRepositoryRequest {
        owner: owner.into(),
        name: name.into(),
        visibility: v as i32,
        ..Default::default()
    };
    let r = |owner: &str, name: &str| Some(RepositoryRef { owner: owner.into(), name: name.into() });

    // Organisation A: dev's. Organisation B: mira's.
    let dev = as_(&s, "dev", "");
    let mira = as_(&s, "mira", "");
    let mut p = dev.platform();
    p.create_organization(org("alpha")).await.unwrap();
    p.create_repository(new_repo("alpha", "vault", Visibility::Private)).await.unwrap();
    p.create_repository(new_repo("alpha", "handbook", Visibility::Internal)).await.unwrap();
    let mut pb = mira.platform();
    pb.create_organization(org("bravo")).await.unwrap();
    pb.create_repository(new_repo("bravo", "ops", Visibility::Private)).await.unwrap();
    pb.create_repository(new_repo("bravo", "wiki", Visibility::Internal)).await.unwrap();

    // A's resources: branches, a change request and an issue naming mira, a comment thread, a
    // webhook, a protection, a lock, a team and tokens.
    let vault = as_(&s, "dev", "alpha/vault");
    let main = changeset(&vault, None, "alpha secret").await;
    assert!(vault.compare_and_swap("main", None, main).await.unwrap());
    let feature = changeset(&vault, Some(main), "alpha secret v2").await;
    assert!(vault.compare_and_swap("alpha-feature", None, feature).await.unwrap());
    vault.lock("alpha-lock.txt").await.unwrap();
    let cr = dev
        .reviews()
        .create_change_request(CreateChangeRequestRequest {
            repository: r("alpha", "vault"),
            source: "alpha-feature".into(),
            target: "main".into(),
            title: "alpha plans".into(),
            reviewers: vec!["mira".into()],
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let cref = || Some(ChangeRequestRef { repository: r("alpha", "vault"), number: cr.number });
    let comment = dev
        .reviews()
        .add_comment(AddCommentRequest { r#ref: cref(), body: "alpha note".into(), ..Default::default() })
        .await
        .unwrap()
        .into_inner();
    let issue = dev
        .issues()
        .create_issue(CreateIssueRequest {
            repository: r("alpha", "vault"),
            title: "alpha bug".into(),
            assignees: vec!["mira".into()],
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let iref = || Some(IssueRef { repository: r("alpha", "vault"), number: issue.number });
    let hook = p
        .create_webhook(CreateWebhookRequest {
            repository: r("alpha", "vault"),
            url: "http://192.0.2.1/alpha-hook".into(),
            events: vec!["check".into()],
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner()
        .webhook
        .unwrap();
    let protection = Protection { repository: r("alpha", "vault"), pattern: "main".into(), ..Default::default() };
    dev.reviews().set_protection(protection.clone()).await.unwrap();
    let team = Team { org: "alpha".into(), slug: "alpha-team".into(), name: "alpha team".into(), ..Default::default() };
    p.set_team(team.clone()).await.unwrap();
    let token = |name: &str, acts_as: &str, repos: &[&str]| CreateTokenRequest {
        name: name.into(),
        acts_as: acts_as.into(),
        repositories: repos.iter().map(|r| r.to_string()).collect(),
        ..Default::default()
    };
    let a_token = p.create_token(token("alpha-ci", "", &[])).await.unwrap().into_inner();
    p.create_token(token("alpha-bot", "agent/alpha-bot", &[])).await.unwrap();
    let handbook_only = p.create_token(token("alpha-docs", "", &["alpha/handbook"])).await.unwrap().into_inner().secret;
    // B's tokens: an agent of B's limited to B's repository, and a person's token limited the same way.
    let b_agent =
        pb.create_token(token("bravo-bot", "agent/bravo-bot", &["bravo/ops"])).await.unwrap().into_inner().secret;
    let b_scoped = pb.create_token(token("bravo-ci", "", &["bravo/ops"])).await.unwrap().into_inner().secret;

    let id = |s: &str| WebhookRef { id: s.into() };
    let mut table = vec![
        // Platform
        probe("Platform/Viewer", Empty {}).ok(),
        probe("Platform/GetAccount", OrgName { login: "dev".into() }).ok(),
        probe("Platform/ListOrganizations", Empty {}).ok(),
        probe("Platform/GetOrganization", OrgName { login: "alpha".into() }),
        Probe { also: Some(Code::AlreadyExists), ..probe("Platform/CreateOrganization", org("alpha")) },
        probe("Platform/ListMembers", OrgName { login: "alpha".into() }),
        probe(
            "Platform/SetMember",
            SetMemberRequest { org: "alpha".into(), login: "mira".into(), role: Role::Admin as i32 },
        ),
        probe("Platform/ListTeams", OrgName { login: "alpha".into() }),
        probe("Platform/SetTeam", Team { members: vec!["mira".into()], ..team.clone() }),
        probe("Platform/DeleteTeam", TeamRef { org: "alpha".into(), slug: "alpha-team".into() }),
        probe("Platform/ListRepositories", ListRepositoriesRequest::default()).ok(),
        probe("Platform/ListRepositories", ListRepositoriesRequest { owner: "alpha".into(), ..Default::default() })
            .ok(),
        probe("Platform/GetRepository", RepositoryRef { owner: "alpha".into(), name: "vault".into() }),
        probe("Platform/GetRepository", RepositoryRef { owner: "alpha".into(), name: "handbook".into() }),
        probe("Platform/CreateRepository", new_repo("alpha", "intruder", Visibility::Private)),
        probe(
            "Platform/UpdateRepository",
            UpdateRepositoryRequest {
                owner: "alpha".into(),
                name: "vault".into(),
                fields: vec!["visibility".into()],
                visibility: Visibility::Public as i32,
                ..Default::default()
            },
        ),
        probe("Platform/ListGrants", RepositoryRef { owner: "alpha".into(), name: "vault".into() }),
        probe(
            "Platform/SetGrant",
            SetGrantRequest { repository: r("alpha", "vault"), principal: "mira".into(), role: Role::Admin as i32 },
        ),
        probe("Platform/ListTokens", Empty {}).ok(),
        probe("Platform/CreateToken", token("x", "", &["alpha/vault"])),
        probe("Platform/CreateToken", token("x", "agent/alpha-bot", &[])),
        probe("Platform/RevokeToken", RevokeTokenRequest { id: a_token.token.unwrap().id }),
        probe("Platform/ListWebhooks", RepositoryRef { owner: "alpha".into(), name: "vault".into() }),
        probe(
            "Platform/CreateWebhook",
            CreateWebhookRequest {
                repository: r("alpha", "vault"),
                url: "http://192.0.2.1/bravo".into(),
                events: vec!["check".into()],
                ..Default::default()
            },
        ),
        probe("Platform/DeleteWebhook", id(&hook.id)).vault(),
        probe("Platform/ListWebhookDeliveries", id(&hook.id)).vault(),
        probe("Platform/PingWebhook", id(&hook.id)).vault(),
        probe("Platform/ListSignupAllow", Empty {}),
        probe("Platform/SetSignupAllow", SetSignupAllowRequest { entry: "@alpha.example".into(), allowed: true }),
        probe(
            "Platform/LinkIdentity",
            LinkIdentityRequest { login: "dev".into(), provider: "email".into(), subject: "mira@bravo.example".into() },
        ),
        probe(
            "Platform/CreateInvitation",
            CreateInvitationRequest {
                org: "alpha".into(),
                email: "mira@bravo.example".into(),
                role: Role::Admin as i32,
            },
        ),
        probe("Platform/ListInvitations", OrgName { login: "alpha".into() }),
        probe("Platform/RevokeInvitation", InvitationRef { org: "alpha".into(), id: "alpha-invite".into() }),
        probe("Platform/GetUsage", GetUsageRequest { org: "alpha".into(), ..Default::default() }),
        probe("Platform/SetLimits", SetLimitsRequest { org: "alpha".into(), limits: Some(OrgLimits::default()) }),
        probe("Platform/SetSuspended", SetSuspendedRequest { org: "alpha".into(), suspended: true }),
        probe("Platform/GetCapabilities", Empty {}).ok(),
        probe("Platform/GetLicense", Empty {}).ok(),
        // Issues
        probe("Issues/ListIssues", ListIssuesRequest { repository: r("alpha", "vault"), ..Default::default() }),
        probe("Issues/ListIssues", ListIssuesRequest { repository: r("alpha", "handbook"), ..Default::default() }),
        probe("Issues/GetIssue", IssueRef { repository: r("alpha", "vault"), number: issue.number }),
        probe(
            "Issues/CreateIssue",
            CreateIssueRequest { repository: r("alpha", "vault"), title: "bravo".into(), ..Default::default() },
        ),
        probe(
            "Issues/UpdateIssue",
            UpdateIssueRequest {
                r#ref: iref(),
                fields: vec!["title".into()],
                title: "bravo".into(),
                ..Default::default()
            },
        ),
        probe("Issues/ListIssueComments", IssueRef { repository: r("alpha", "vault"), number: issue.number }),
        probe("Issues/AddIssueComment", AddIssueCommentRequest { r#ref: iref(), body: "bravo".into() }),
        // Reviews
        probe(
            "Reviews/ListChangeRequests",
            ListChangeRequestsRequest { repository: r("alpha", "vault"), ..Default::default() },
        ),
        probe(
            "Reviews/ListChangeRequests",
            ListChangeRequestsRequest { repository: r("alpha", "handbook"), ..Default::default() },
        ),
        probe("Reviews/GetChangeRequest", ChangeRequestRef { repository: r("alpha", "vault"), number: cr.number }),
        probe(
            "Reviews/CreateChangeRequest",
            CreateChangeRequestRequest {
                repository: r("alpha", "vault"),
                source: "alpha-feature".into(),
                target: "main".into(),
                title: "bravo".into(),
                ..Default::default()
            },
        ),
        probe(
            "Reviews/UpdateChangeRequest",
            UpdateChangeRequestRequest {
                r#ref: cref(),
                fields: vec!["state".into()],
                state: ChangeRequestState::Closed as i32,
                ..Default::default()
            },
        ),
        probe("Reviews/ListComments", ChangeRequestRef { repository: r("alpha", "vault"), number: cr.number }),
        probe("Reviews/AddComment", AddCommentRequest { r#ref: cref(), body: "bravo".into(), ..Default::default() }),
        probe(
            "Reviews/ResolveThread",
            ResolveThreadRequest { r#ref: cref(), thread: comment.id.clone(), resolved: true },
        ),
        probe(
            "Reviews/SubmitReview",
            SubmitReviewRequest { r#ref: cref(), verdict: Verdict::Approved as i32, body: "lgtm".into() },
        ),
        probe("Reviews/Land", ChangeRequestRef { repository: r("alpha", "vault"), number: cr.number }),
        probe("Reviews/Rebase", ChangeRequestRef { repository: r("alpha", "vault"), number: cr.number }),
        probe("Reviews/ListProtections", RepositoryRef { owner: "alpha".into(), name: "vault".into() }),
        probe("Reviews/SetProtection", Protection { bypass: vec!["mira".into()], ..protection.clone() }),
        probe("Reviews/DeleteProtection", ProtectionRef { repository: r("alpha", "vault"), pattern: "main".into() }),
        probe(
            "Reviews/ReportCheck",
            ReportCheckRequest {
                r#ref: cref(),
                check: Some(Check { name: "ci".into(), state: CheckState::Passed as i32, ..Default::default() }),
            },
        ),
        probe("Reviews/Inbox", InboxRequest::default()).ok(),
        probe("Reviews/ListEvents", ListEventsRequest { repository: r("alpha", "vault"), ..Default::default() }),
        probe("Reviews/ListEvents", ListEventsRequest::default()).ok(),
    ];
    // Repository services: refused before the request is read, at either of A's repositories.
    let repo_methods = [
        "Objects/Get",
        "Objects/Put",
        "Objects/Missing",
        "Changesets/Create",
        "Changesets/Get",
        "Refs/Get",
        "Refs/CompareAndSwap",
        "Refs/List",
        "Land/Land",
        "History/Log",
        "History/Blame",
        "Diff/Diff",
        "MergeBase/MergeBase",
        "Browse/ListDir",
        "Browse/ReadFile",
        "Browse/Find",
        "Browse/Checkout",
        "Access/WhoAmI",
        "Locks/Lock",
        "Locks/Unlock",
        "Locks/List",
    ];
    for m in repo_methods {
        for repo in ["alpha/vault", "alpha/handbook"] {
            let unlock = UnlockRequest { path: "alpha-lock.txt".into(), force: true };
            let body = if m == "Locks/Unlock" { prost::Message::encode_to_vec(&unlock) } else { vec![] };
            table.push(Probe { repo: Some(repo), body, vault: repo == "alpha/vault", ..probe(m, Empty {}) });
        }
    }

    // Every method in the protocol has an entry.
    let set = <prost_types::FileDescriptorSet as prost::Message>::decode(pando_proto::FILE_DESCRIPTOR_SET).unwrap();
    let all: Vec<String> = set
        .file
        .iter()
        .flat_map(|f| {
            f.service
                .iter()
                .flat_map(move |s| s.method.iter().map(move |m| format!("/{}.{}/{}", f.package(), s.name(), m.name())))
        })
        .collect();
    let unchecked: Vec<&String> = all.iter().filter(|m| !table.iter().any(|p| &p.method == *m)).collect();
    assert!(unchecked.is_empty(), "add these methods to the cross-tenant table: {unchecked:?}");
    assert!(all.len() >= 78, "{} methods", all.len());

    let ch = tonic::transport::Endpoint::from_shared(s.url.clone()).unwrap().connect().await.unwrap();
    let mut failures = Vec::new();
    // (who, token, marker of what they must not see, probes aimed at the vault only)
    let callers = [
        ("mira", "mira-token".to_owned(), "alpha", false),
        ("eve (no organisation)", "eve-token".to_owned(), "alpha", false),
        ("agent/bravo-bot", b_agent, "alpha", false),
        ("mira, limited to bravo/ops", b_scoped, "alpha", false),
        ("dev, limited to alpha/handbook", handbook_only, "vault", true),
    ];
    for (who, secret, marker, vault_only) in &callers {
        for p in table.iter().filter(|p| !vault_only || p.vault || p.ok) {
            let leaked =
                |msgs: &[Vec<u8>]| msgs.iter().any(|m| m.windows(marker.len()).any(|w| w == marker.as_bytes()));
            match call(&ch, secret, p.repo, &p.method, &p.body).await {
                Ok(msgs) if p.ok && !leaked(&msgs) => {}
                Ok(msgs) => failures.push(format!(
                    "{who}: {} {:?} succeeded: {:?}",
                    p.method,
                    p.repo,
                    msgs.iter().map(|m| String::from_utf8_lossy(m).into_owned()).collect::<Vec<_>>()
                )),
                Err(e) if matches!(e.code(), Code::NotFound | Code::PermissionDenied) || Some(e.code()) == p.also => {}
                Err(e) => failures.push(format!("{who}: {} {:?}: {:?} {}", p.method, p.repo, e.code(), e.message())),
            }
        }
    }
    assert!(failures.is_empty(), "{} cross-tenant failures:\n{}", failures.len(), failures.join("\n"));

    // Regression: naming a repository in a new token told whether a private one exists.
    for name in ["alpha/vault", "alpha/nothing"] {
        let e = pb.create_token(token("probe", "", &[name])).await.unwrap_err();
        assert_eq!(
            (e.code(), e.message()),
            (Code::NotFound, format!("no repository `{name}` on this server").as_str())
        );
    }

    // Regression: granting a repository to a team told whether another organisation has it.
    for team in ["alpha/alpha-team", "alpha/ghost"] {
        let grant = SetGrantRequest { repository: r("bravo", "ops"), principal: team.into(), role: Role::Read as i32 };
        let e = pb.set_grant(grant).await.unwrap_err();
        assert_eq!(e.code(), Code::FailedPrecondition, "{team}: {e:?}");
        assert!(e.message().contains("has no Pando account yet"), "{team}: {e:?}");
    }

    // The probes aim at real things: A's owner reads each of them.
    let reads = table.iter().filter(|p| {
        let m = p.method.rsplit('/').next().unwrap();
        p.repo.is_none() && !p.ok && (m.starts_with("Get") || m.starts_with("List")) && m != "ListSignupAllow"
            || p.method == "/pando.v1.Refs/List"
    });
    for p in reads {
        if let Err(e) = call(&ch, "dev-token", p.repo, &p.method, &p.body).await {
            panic!("dev: {} {:?}: {e:?}", p.method, p.repo);
        }
    }

    // A's things are all still there.
    assert_eq!(vault.locks("").await.unwrap().len(), 1);
    let crs = dev
        .reviews()
        .list_change_requests(ListChangeRequestsRequest { repository: r("alpha", "vault"), ..Default::default() })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(crs.change_requests[0].state, ChangeRequestState::Open as i32);
    assert_eq!(
        p.list_webhooks(RepositoryRef { owner: "alpha".into(), name: "vault".into() })
            .await
            .unwrap()
            .into_inner()
            .webhooks
            .len(),
        1
    );
    assert_eq!(p.list_tokens(Empty {}).await.unwrap().into_inner().tokens.len(), 3);
    let repo =
        p.get_repository(RepositoryRef { owner: "alpha".into(), name: "vault".into() }).await.unwrap().into_inner();
    assert_eq!(repo.visibility, Visibility::Private as i32);
}
