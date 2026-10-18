# Deploying Pando

A single `pandod server` with SQLite is enough for a team on one machine. For more, run
stateless servers on Postgres or CockroachDB with objects in S3, GCS or Azure, and put a
regional cache and a node daemon between CI machines and the servers.

## The whole topology with Docker Compose

The repository's [`docker-compose.yml`](https://github.com/kabirnarang39/pando/blob/main/docker-compose.yml)
starts Postgres, an S3-compatible store, a server, a regional cache and a node daemon:

```sh
docker compose up -d
pando clone http://localhost:7420 work --objects http://localhost:7422
```

`--objects` sends file reads through the node daemon; commits and lands still go to the
server.

## For free, on one machine

[`deploy/free-tier`](https://github.com/kabirnarang39/pando/tree/main/deploy/free-tier) sets up a
production server on Oracle Cloud's Always Free ARM machine: HTTPS by Caddy, a hardened systemd
service, and hourly incremental backups to a free Neon database and R2 bucket. It costs nothing
until usage outgrows the free tiers, and moves to Postgres and object storage with `pandod copy`
when it does.

## Hosting many repositories

One `pandod` can host any number of repositories, each with its own storage, for a whole
company: organisations, members, teams, per-repository roles and access tokens.

```sh
# 1. The first administrator's token, written straight into the registry (shown once).
pandod token --platform sqlite:///var/lib/pando/platform.db --account kabir > ~/.pando-admin-token

# 2. The server. With a platform token in the registry, every call needs a token.
pandod server --listen 0.0.0.0:7420 \
  --platform sqlite:///var/lib/pando/platform.db \
  --repos-metadata 'sqlite:///var/lib/pando/repos/{repo}.db' \
  --repos-objects 'file:///var/lib/pando/objects/{repo_id}' \
  --platform-admins kabir

# 3. An organisation, a repository, people and agents.
export PANDO_TOKEN=$(cat ~/.pando-admin-token) PANDO_SERVER=https://pando.example.com
pando org create lumen
pando repo create lumen/fleet
pando org member lumen mira@lumen.dev write
pando team set lumen/platform --member mira@lumen.dev
pando repo grant lumen/fleet lumen/platform admin
pando token create perf-fleet --as agent/perf-05 --repo lumen/fleet --role write
```

- `--platform` is the registry: which repositories exist, and who may use them.
  `--repos-metadata` and `--repos-objects` are templates: `{repo}` becomes `OWNER/NAME` and
  `{repo_id}` becomes `OWNER--NAME`. Each repository gets its own database and its own landing
  queues, so one busy repository never slows another, and a repository can be backed up, moved
  or deleted on its own.
- Clients name a repository by adding `/OWNER/NAME` to the server address; the web console lists
  them. A repository call that names none is refused: the server's own `--metadata` repository
  is not reachable in this mode. To keep a single-repository server's history, create the
  repository and copy it in with `pandod copy`.
- `--multi-tenant` (`PANDO_MULTI_TENANT=1`) is for a server that strangers share; see
  [Multi-tenant servers](platform.md#multi-tenant-servers). `--max-orgs-per-account` (default 3)
  caps how many organisations a person may create there.
- Webhooks never call loopback, private networks or cloud metadata addresses.
  `--webhook-allow-private` lets them call loopback and private networks (self-hosted CI on an
  internal network); it can't be combined with `--multi-tenant`.

## Signing in from a browser

On a platform server, `--login-listen ADDR` serves sign-in pages (Google or any OpenID Connect
provider, and email links) that hand the console an ordinary platform token. Run it on a
loopback address and route `/login/*` to it from the reverse proxy that serves the console, on
the same host name as `--public-url`:

```
pando.example.com {
  handle /login/* {
    reverse_proxy 127.0.0.1:7430
  }
  # … the console and gRPC-Web routes
}
```

| Setting | Flag / environment |
|---|---|
| Public address | `--public-url` / `PANDO_PUBLIC_URL` (https; http only for localhost) |
| Provider | `--oidc google=https://accounts.google.com` (repeatable); `PANDO_OIDC_GOOGLE_CLIENT_ID`, `PANDO_OIDC_GOOGLE_CLIENT_SECRET` or `…_FILE` |
| Email | `--smtp-url` / `PANDO_SMTP_URL`, `--mail-from` / `PANDO_MAIL_FROM`; `PANDO_SMTP_USERNAME`, `PANDO_SMTP_PASSWORD` or `…_FILE` |
| Bot challenge | `--turnstile-site-key`; `PANDO_TURNSTILE_SECRET` or `…_FILE` (unset: none) |
| Who may sign up | `--signup waitlist\|open\|closed` / `PANDO_SIGNUP` (default `waitlist`, managed with `pando admin signup`) |

The proxy must set `X-Forwarded-For`, which the per-address request limits use. With sign-in
on, the server always requires a token, even before any exists. Setup, the waitlist and
linking the first administrator are described in [the platform notes](platform.md#sign-in).

## Pre-land checks

With `--land-hook`, the server sends each proposed land to your URL before it moves the
branch: who is landing, the branch, the changeset and its paths. Any 2xx answer lets it
land; anything else, including a timeout, refuses it. Use it to require green CI or an
approved review. The payload is described in [Architecture](ARCHITECTURE.md#land-checks).

## Running a hosted service

### Taking a repository down

```sh
pandod repo --platform sqlite:///var/lib/pando/platform.db disable acme/leaky --reason "takedown request 2026-118"
pandod repo --platform sqlite:///var/lib/pando/platform.db enable acme/leaky
```

`disable` refuses every read and write of the repository, for everyone including
administrators, with "disabled by the operator: REASON", and hides it from listings. Nothing is
deleted. It works while the server runs, which notices within 5 seconds. Both are recorded in
the audit log (`disable_repository`, `enable_repository`) when the command is given
`--audit-log` (`pandod --audit-log FILE repo …`).

### Deleting a repository

```sh
pandod repo --platform sqlite:///var/lib/pando/platform.db delete acme/old \
  --repos-metadata 'sqlite:///var/lib/pando/repos/{repo}.db' \
  --repos-objects 'file:///var/lib/pando/objects/{repo_id}' --confirm acme/old
```

This deletes the registry rows (change requests, reviews, issues, webhooks and their
deliveries, grants, protections) and the repository's SQLite database and `file://` object
directory. Objects in a cloud bucket and Postgres metadata are named in the output for you to
remove. It cannot be undone: take a `pandod backup` first. Platform tokens limited to the
repository keep its name in their list; revoke them if the name will be reused.

### Purging a leaked secret

History is content-addressed, so a committed secret can't be edited out in place. Rotate the
secret first (that is what actually protects you), then:

1. Disable the repository (`pandod repo disable`) so nobody fetches it meanwhile.
2. Rewrite the history without the secret: `pando export-git` to a git repository, remove the
   file or string with `git filter-repo`, and `pando import-git` into a new repository.
3. Delete the old repository (`pandod repo delete`; on Windows, stop the server first, because Windows won't delete files a running server holds open); recreate it under the same name if you
   want to, and import the rewritten history there.
4. Ask everyone with a checkout to clone again; old checkouts still hold the secret.

Deleting an object while other changesets still reference it is not supported: there is no
object garbage collection yet, so objects a deleted branch alone referenced stay in storage
until the repository is deleted. Plan capacity accordingly.

## Next steps

- [Organisations, people and access](orgs.md): roles, teams, tokens, protected branches.
- [Security and audit](security.md): single sign-on, access rules and the audit log.
- [Backup and restore](backup.md): `pandod copy` and `pandod backup`, and how to restore a
  platform server.
- [Scaling and operations](scaling.md): caches, node daemons, compaction and monitoring.
- [Configuration](reference/configuration.md): every common setting in one table.
