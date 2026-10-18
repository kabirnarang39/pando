# Security and audit

How a server decides who is calling, what they may do, and what it records. Report
vulnerabilities as described in the [security policy](security-policy.md); the security model
is there too.

> [!WARNING]
> A server without tokens, single sign-on or platform tokens accepts every caller. It refuses to
> listen beyond 127.0.0.1 in that state unless you pass `--allow-anonymous`; never do that on a
> network you don't control.

## Authentication and permissions

Without `--tokens`, `--oidc-issuer` or platform tokens the server accepts everyone, and so
it listens only on a loopback address unless `--allow-anonymous` (`PANDO_ALLOW_ANONYMOUS=1`)
says otherwise. In production, set at least one. With OpenID Connect, the claim named by `--oidc-claim` (for
example `email`) becomes the principal that access rules refer to.

Every `--oidc-require CLAIM=VALUE` (repeatable, or comma-separated in `PANDO_OIDC_REQUIRE`)
must match the token exactly; booleans and numbers compare as written (`email_verified=true`).
Issuers that sign tokens for anyone refuse to start without a rule that binds your tenant:
GitHub Actions (`https://token.actions.githubusercontent.com`) needs `repository_owner`,
`repository_owner_id`, `repository`, `repository_id`, `enterprise` or `enterprise_id`; Google
(`https://accounts.google.com`) needs `hd`. For other providers, pin the tenant the same way
(`tid=…`, `org_id=…`) when the issuer serves more than your organisation.

```sh
pandod server --oidc-issuer https://token.actions.githubusercontent.com --oidc-audience pando \
  --oidc-require repository_owner=acme ...
```

A token is refused when its principal is a `--platform-admins` login (unless that login is also
given with `--oidc-admin`), or starts with `agent/` or `bot/`, so an identity provider that lets
people choose their name can't make anyone an administrator or an agent. Unknown signing keys
refresh the key set at most once a minute.

Access rules grant read and write per directory and per branch. Principals who can read
only part of the repository get path-scoped checkouts, search and history, with changes
outside their scope removed. `--acl FILE` (or `PANDO_ACL`) names a JSON file of rules:

```json
{
  "default":    { "write": ["*"], "refs": ["user/*"] },
  "principals": {
    "release-bot": { "refs": ["main", "release/*"] },
    "api-team":    { "write": ["services/api", "libs/api-client"], "refs": ["main"] }
  }
}
```

A principal's grants are the union of `default` and its own entry. `read` and `write` list path
prefixes (`*` is everything); `refs` lists branch names or `prefix/*`. When no entry mentions
`read`, everyone may read everything.

## Audit log

`--audit-log FILE` (or `PANDO_AUDIT_LOG`) appends one JSON line per event: who read or
changed what, from which address, with what result. It is off by default because it grows
with every request. Events go to one file per UTC day, `FILE.YYYY-MM-DD`; files older than
`--audit-retention-days` (default 365) are deleted. Behind a reverse proxy on the same host,
add `--trust-proxy` so the address recorded is the client's, not the proxy's.

**Retention for hosted services.** Keep at least 365 days. In India, CERT-In's 2022
directions require 180 days of logs kept within India, and the DPDP Rules require one year from
May 2027; other jurisdictions have their own rules, so check yours. Copy the daily files to
durable storage in the required region (the same backup job as the databases works) rather than
relying on one disk.

**Clocks.** Timestamps are UTC (RFC 3339) from the host clock, so keep it synchronised: run
`systemd-timesyncd` or `chrony` against reliable servers. CERT-In asks Indian service providers
to synchronise with the NIC (`samay1.nic.in`, `samay2.nic.in`) or NPL (`time.nplindia.org`)
NTP servers; elsewhere use your cloud's time service or `pool.ntp.org`.

```json
{"timestamp":"2026-10-16T09:06:02.695Z","kind":"land","principal":"alice","ref":"main","changeset":"0844d2dc…","landed":"0844d2dc…","rebased":false}
{"timestamp":"2026-10-16T09:06:02.702Z","kind":"read_file","principal":"ci","rev":"main","path":"README"}
{"timestamp":"2026-10-16T09:06:02.702Z","kind":"rpc","principal":"ci","peer":"10.0.4.17","method":"/pando.v1.Browse/ReadFile","status":"0","ms":0}
{"timestamp":"2026-10-16T09:06:58.112Z","kind":"auth_failed","peer":"10.0.9.2","presented":true,"reason":"invalid token"}
```

| `kind` | Recorded for |
|---|---|
| `rpc` | every authenticated request: principal, client address, method, gRPC status, duration |
| `auth_failed` | every rejected request: client address, whether a token was presented, why (never the token) |
| `land` | each land: branch, changeset, and the landed id or the refusal |
| `set_ref` | branch moves through the refs API |
| `read_file`, `list_dir`, `checkout`, `find` | content reads, with revision and paths |
| `log`, `blame` | history queries, with revision and path |
| `lock`, `unlock` | file locks taken and released |
| `account_created`, `sign_in`, `sign_up`, `accept_invitation`, `create_token`, ... | changes to accounts, tokens, organisations, access, reviews and webhooks (platform servers), with a `target` |

Audit events never appear in the normal log, and the normal log never appears in the
audit file.
