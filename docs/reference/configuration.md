# Configuration

The settings most deployments touch, with their environment variables and defaults. Every
option of every command is in the [server reference](server.md), generated from `--help`.

| Setting | Flag / environment | Default |
|---|---|---|
| Metadata store | `--metadata` / `PANDO_METADATA` | `sqlite://./pando-data/meta.db`; also `memory`, `postgres://…` |
| Object store | `--objects` / `PANDO_OBJECTS` | `metadata` (in the metadata store); also `file:///…`, `s3://…`, `gs://…`, `az://…` |
| Bearer tokens | `--tokens` / `PANDO_TOKENS` | none; e.g. `alice:$(openssl rand -hex 24)`. Tokens shorter than 16 characters or left as a placeholder (`CHANGE-ME`) are refused at startup. Without tokens or OIDC, pandod listens only on a loopback address unless `--allow-anonymous` / `PANDO_ALLOW_ANONYMOUS=1` says otherwise |
| Request limits | `--request-timeout` / `PANDO_REQUEST_TIMEOUT`, `--max-in-flight` / `PANDO_MAX_IN_FLIGHT` | 600 s per call; 4,096 calls in progress, beyond which calls are refused with Unavailable so clients back off; 256 per connection |
| Per-account limits | `--max-calls-per-principal` / `PANDO_MAX_CALLS_PER_PRINCIPAL`, `--max-request-mb` / `PANDO_MAX_REQUEST_MB` | with `--platform`: 128 calls in progress per account (more are refused with ResourceExhausted) and 64 MiB per request message; otherwise no per-account limit and 260 MiB. Files of 8 MiB and more are uploaded in chunks, so 64 MiB never limits file size |
| Browser access | `--cors-origin` / `PANDO_WEB_ORIGINS` (repeatable or comma-separated; `--web-origins` still works) | none: same-origin only. List only origins you control, such as your own copy of the web console |
| Client addresses | `--trust-proxy` / `PANDO_TRUST_PROXY` | off; on, a connection from loopback (a reverse proxy on the same host) gives the client address in `X-Forwarded-For`, used in the audit log |
| Durability | `--sqlite-synchronous` / `PANDO_SQLITE_SYNCHRONOUS` | `full`: every SQLite commit is on disk before it is acknowledged, so a land survives power loss. `normal` survives process crashes only. `file://` object stores always sync each object and its directory |
| Open repositories (platform) | `--max-open-repos` / `PANDO_MAX_OPEN_REPOS`, `--repo-idle-minutes` / `PANDO_REPO_IDLE_MINUTES` | 256 open at once; one unused for 10 minutes closes. A repository in use is never closed |
| Audit retention | `--audit-retention-days` / `PANDO_AUDIT_RETENTION_DAYS` | 365 days of daily files; 0 keeps all |
| Single sign-on | `--oidc-issuer`, `--oidc-audience`, `--oidc-claim`, `--oidc-require CLAIM=VALUE`, `--oidc-admin LOGIN` | off; works with Okta, Entra ID, Google, Auth0, Keycloak, GitHub Actions |
| Browser sign-in (platform) | `--login-listen`, `--public-url`, `--oidc ID=ISSUER`, `--smtp-url`, `--mail-from`, `--turnstile-site-key`, `--signup` | off; see [Signing in from a browser](../deploy.md#signing-in-from-a-browser) |
| Access rules | `--acl` / `PANDO_ACL` | allow everything |
| Pre-land check | `--land-hook URL`, `--land-hook-token`, `--land-hook-timeout` | off |
| TLS | `--tls-cert`, `--tls-key` | off |
| Metrics | `--metrics` / `PANDO_METRICS` | off; e.g. `0.0.0.0:9420` |
| Feature flags | `PANDO_FLAGS` | all on; e.g. `land=off,git-import=off` |

Run `pandod server --help` for every option.
