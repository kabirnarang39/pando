# Organisations and access

On a server that hosts many repositories (`pandod server --platform`, see
[Deploying Pando](deploy.md#hosting-many-repositories)), people and agents belong to
organisations, get roles on repositories, and land on protected branches through reviewed change
requests. This page covers who may do what, and how review works. Signing in from a browser,
invitations and multi-tenant servers are in [Sign-in, organisations, invitations](platform.md).

## Who may do what

| Role | Read | Write (land, push objects, lock) | Administer (access, settings) |
|---|---|---|---|
| Read, Triage | yes | | |
| Write, Maintain | yes | yes | |
| Admin | yes | yes | yes |

- A person's role on a repository is the highest of: their organisation role, grants to them,
  and grants to their teams. Internal and public repositories are readable by everyone with an
  account on the server. Repositories are private unless made otherwise.
- `--platform-admins` administer the whole server: they create organisations and act as
  administrators everywhere. An organisation's administrators add members, teams and
  repositories.
- An agent (`agent/NAME`) answers to the person who issued its first token and works with that
  person's role, or with its own if it has been given one.
- A token acts as you or as an agent you answer for. It can be limited to some repositories and
  to a highest role, and expires within a year (90 days by default). The server stores only a
  hash; tokens start with `pando_`, so secret scanners can find leaked ones.
- People who sign in with single sign-on or a static token get an account automatically; give
  them a role with `pando org member`.
- Someone who can't read a repository is told it does not exist, so private names don't leak.
  Every change to access is in the audit log.
- With no static tokens, single sign-on or platform tokens, the server is open, for local
  development; it refuses to listen beyond 127.0.0.1 that way unless `PANDO_ALLOW_ANONYMOUS=1`.
- Caches and node daemons serve the default repository only.

## Change requests and protected branches

```sh
pando protect set main --approvals 1 --check build --bypass admins   # repository admins
pando checkout -b lower-rate && $EDITOR tax.rs && pando commit -m "Lower the rate" && pando land
pando cr create --title "Lower the tax rate" --reviewer mira@lumen.dev
pando cr review 7 approve -m "Matches the regulation"                # mira
pando cr check 7 build passed --url https://ci.example.com/runs/91   # CI, with a write token
pando cr land 7
```

- A protected branch is updated only by those its protection lets bypass review (a login, a team
  `org/team`, or `admins`), and by agents when it lets agents land directly. Everyone else lands
  through a change request: a direct land opens one for them (from a `review/…` branch) and
  says so.
- A change request lands when nothing blocks it: enough approvals from people other than its
  author, every required check passed, no requested changes, no conflicts, not a draft. Each
  blocker is shown in words. With "a new version needs new approvals" (the default), pushing to
  the branch makes earlier approvals and requested changes stale.
- It lands as one changeset, rebased onto the target's tip file by file like any other land, with
  the title, description and approvers in its message. `pando cr rebase` replays a change request
  on its target's tip when they changed different files.
- Checks belong to a version of the change: CI reports them with `pando cr check` (or the
  `ReportCheck` call) and a new push starts with none.
- A change is reviewed and checked by someone else (ADR-030). The author, the person an agent
  author answers to, and every agent that person answers for count as one person: none of them
  can approve or request changes, their approvals never count, and they can't report the
  change's checks. Give CI its own account or token (for example `bot/ci`).
- Reading needs Read; commenting needs Read; opening a change request needs Triage; approving,
  requesting changes, reporting checks and landing need Write; protecting branches needs Admin.
