//! Plans and limits (ADR-035): active agents are counted from real lands only, and a
//! multi-tenant server enforces an organisation's limits while reads keep working.

mod common;

use pando::remote::{GrpcRemote, Remote};
use pando_core::Id;
use pando_core::object::{Changeset, FileChange, Kind, Mode, encode_object};
use pando_proto::{
    CreateRepositoryRequest, CreateTokenRequest, Empty, GetUsageRequest, OrgLimits, Role, SetLimitsRequest,
    SetMemberRequest, SetSuspendedRequest, Visibility,
};
use pando_server::permissions::Access;
use pando_server::plans::Limits;

fn connect(url: &str, token: &str) -> GrpcRemote {
    GrpcRemote::connect(url, None, Some(token)).unwrap()
}

/// A changeset adding `path` on top of `main`'s tip.
async fn change(r: &GrpcRemote, path: &str) -> anyhow::Result<(Option<Id>, Id)> {
    let (blob, payload) = encode_object(Kind::Blob, path.as_bytes());
    r.objects().put_many(vec![(blob, payload)]).await?;
    let base = r.get_ref("main").await?;
    let cs = r
        .create_changeset(Changeset {
            parents: base.iter().map(|b| b.0.to_vec()).collect(),
            changes: vec![FileChange::upsert(path, Mode::File, blob)],
            author: "agent".into(),
            message: format!("Add {path}"),
            timestamp_ms: pando_server::hosting::now_ms(),
        })
        .await?;
    Ok((base, cs.id))
}

/// Lands `path` on `main` as the holder of `r`.
async fn land(r: &GrpcRemote, path: &str) -> anyhow::Result<()> {
    let (base, cs) = change(r, path).await?;
    match base {
        None => anyhow::ensure!(r.compare_and_swap("main", None, cs).await?, "lost the swap"),
        Some(_) => {
            r.land("main", cs).await?;
        }
    }
    Ok(())
}

async fn agent_token(dev: &GrpcRemote, agent: &str) -> String {
    let t = dev
        .platform()
        .create_token(CreateTokenRequest { name: agent.into(), acts_as: agent.into(), ..Default::default() })
        .await
        .unwrap()
        .into_inner();
    t.secret
}

fn member(login: &str, role: Role) -> SetMemberRequest {
    SetMemberRequest { org: "lumen".into(), login: login.into(), role: role as i32 }
}

fn repo(name: &str, visibility: Visibility) -> CreateRepositoryRequest {
    CreateRepositoryRequest {
        owner: "lumen".into(),
        name: name.into(),
        visibility: visibility as i32,
        ..Default::default()
    }
}

async fn usage(admin: &GrpcRemote) -> pando_proto::Usage {
    admin
        .platform()
        .get_usage(GetUsageRequest { org: "lumen".into(), month: String::new() })
        .await
        .unwrap()
        .into_inner()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_multi_tenant_server_enforces_plans() {
    let s = common::start_with(Access {
        admins: vec!["admin".into()],
        multi_tenant: true,
        max_orgs_per_account: 3,
        default_org_limits: Some(Limits::parse("seats=2,agents=2,private_repos=1,plan=Starter").unwrap()),
        billing_url: "https://billing.example/upgrade".into(),
        ..Default::default()
    })
    .await;
    let admin = connect(&s.url, "admin-token");
    let dev = connect(&s.url, "dev-token");
    for who in ["dev-token", "mira-token", "eve-token"] {
        connect(&s.url, who).platform().viewer(Empty {}).await.unwrap(); // signs in: an account
    }
    let caps = dev.platform().get_capabilities(Empty {}).await.unwrap().into_inner();
    assert_eq!((caps.billing_url.as_str(), caps.limits), ("https://billing.example/upgrade", true));

    // Private repositories: one, by the default limits; public ones are not limited.
    admin.platform().create_repository(repo("fleet", Visibility::Private)).await.unwrap();
    let e = admin.platform().create_repository(repo("web", Visibility::Private)).await.unwrap_err();
    assert_eq!(e.code(), tonic::Code::ResourceExhausted, "{e:?}");
    assert!(e.message().contains("1 of 1 private repositories"), "{e:?}");
    admin.platform().create_repository(repo("site", Visibility::Public)).await.unwrap();

    // Seats: two people; changing a member's role takes no new seat; agents take none.
    admin.platform().set_member(member("dev", Role::Write)).await.unwrap();
    admin.platform().set_member(member("mira", Role::Read)).await.unwrap();
    let e = admin.platform().set_member(member("eve", Role::Read)).await.unwrap_err();
    assert_eq!(e.code(), tonic::Code::ResourceExhausted, "{e:?}");
    assert!(e.message().contains("2 of 2 seats") && e.message().contains("billing.example"), "{e:?}");
    admin.platform().set_member(member("mira", Role::Write)).await.unwrap();

    let fleet = format!("{}/lumen/fleet", s.url);
    let mut agents = Vec::new();
    for a in ["agent/a0", "agent/a1", "agent/a2", "agent/a3"] {
        agents.push(connect(&fleet, &agent_token(&dev, a).await));
    }

    // A lost compare-and-swap moves nothing, so its agent isn't active.
    land(&connect(&fleet, "dev-token"), "README.md").await.unwrap();
    let (_, cs) = change(&agents[0], "lost.txt").await.unwrap();
    assert!(!agents[0].compare_and_swap("main", None, cs).await.unwrap(), "expected an empty branch");
    assert!(usage(&admin).await.active_agents.is_empty(), "people and lost swaps aren't active agents");

    // Two agents may land this month; the third is refused, the first two keep landing.
    land(&agents[1], "a1.txt").await.unwrap();
    land(&agents[2], "a2.txt").await.unwrap();
    let e = land(&agents[3], "a3.txt").await.unwrap_err();
    let msg = format!("{e:#}");
    assert!(msg.contains("2 of 2 active agents this month") && msg.contains("wait until"), "{msg}");
    land(&agents[1], "a1-again.txt").await.unwrap();
    land(&agents[2], "a2-again.txt").await.unwrap();

    let u = usage(&admin).await;
    let names: Vec<_> = u.active_agents.iter().map(|a| a.login.as_str()).collect();
    assert_eq!(names, ["agent/a1", "agent/a2"], "the refused agent isn't counted");
    assert_eq!((u.seats, u.private_repos, u.enforced), (2, 1, true));
    let limits = u.limits.unwrap();
    assert_eq!((limits.active_agents, limits.plan_label.as_str()), (Some(2), "Starter"));

    // Only organisation administrators see usage; only site administrators change limits.
    let e = dev.platform().get_usage(GetUsageRequest { org: "lumen".into(), month: String::new() }).await.unwrap_err();
    assert_eq!(e.code(), tonic::Code::PermissionDenied);
    let raise = SetLimitsRequest {
        org: "lumen".into(),
        limits: Some(OrgLimits {
            seats: Some(10),
            active_agents: Some(3),
            plan_label: "Team".into(),
            ..Default::default()
        }),
    };
    let e = dev.platform().set_limits(raise.clone()).await.unwrap_err();
    assert_eq!(e.code(), tonic::Code::PermissionDenied);
    let e =
        admin.platform().get_usage(GetUsageRequest { org: "lumen".into(), month: "2026-13".into() }).await.unwrap_err();
    assert_eq!(e.code(), tonic::Code::InvalidArgument);
    admin.platform().set_limits(raise).await.unwrap();
    land(&agents[3], "a3.txt").await.unwrap();
    admin.platform().set_member(member("eve", Role::Read)).await.unwrap();
    assert_eq!(usage(&admin).await.active_agents.len(), 3);

    // A suspended organisation can be read but not written.
    let suspend = |suspended| SetSuspendedRequest { org: "lumen".into(), suspended };
    let l = admin.platform().set_suspended(suspend(true)).await.unwrap().into_inner();
    assert!(l.suspended && l.active_agents == Some(3), "suspending keeps the limits: {l:?}");
    let e = land(&agents[1], "suspended.txt").await.unwrap_err();
    assert!(format!("{e:#}").contains("organisation lumen is suspended; contact support"), "{e:#}");
    assert!(agents[1].get_ref("main").await.unwrap().is_some(), "reads still work");
    let org = dev.platform().get_organization(pando_proto::OrgName { login: "lumen".into() }).await.unwrap();
    assert!(org.into_inner().suspended, "members see the suspension (the console's banner)");
    admin.platform().set_suspended(suspend(false)).await.unwrap();
    land(&agents[1], "restored.txt").await.unwrap();

    // Over its storage, an organisation can't write, but can still read.
    pando_server::plans::measure_storage(&s.hosting).await.unwrap();
    let u = usage(&admin).await;
    assert!(u.storage_bytes > 0 && u.peak_storage_bytes >= u.storage_bytes, "{u:?}");
    admin
        .platform()
        .set_limits(SetLimitsRequest {
            org: "lumen".into(),
            limits: Some(OrgLimits { storage_bytes: Some(1024), ..Default::default() }),
        })
        .await
        .unwrap();
    let e = land(&agents[1], "big.txt").await.unwrap_err();
    assert!(format!("{e:#}").contains("of storage"), "{e:#}");
    assert!(agents[1].get_ref("main").await.unwrap().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn without_multi_tenant_nothing_is_limited() {
    let s = common::start().await;
    let admin = connect(&s.url, "admin-token");
    admin.platform().create_repository(repo("fleet", Visibility::Private)).await.unwrap();
    admin
        .platform()
        .set_limits(SetLimitsRequest {
            org: "lumen".into(),
            limits: Some(OrgLimits { active_agents: Some(0), private_repos: Some(0), ..Default::default() }),
        })
        .await
        .unwrap();
    admin.platform().set_suspended(SetSuspendedRequest { org: "lumen".into(), suspended: true }).await.unwrap();
    admin.platform().create_repository(repo("web", Visibility::Private)).await.unwrap();
    admin.platform().set_member(member("agent/fix-01", Role::Write)).await.unwrap();
    let agent = connect(&format!("{}/lumen/fleet", s.url), "agent-fix-01-token");
    land(&agent, "fix.txt").await.unwrap();
    let u = usage(&admin).await;
    assert!(!u.enforced);
    assert_eq!(u.active_agents.len(), 1, "usage is still recorded");
}
