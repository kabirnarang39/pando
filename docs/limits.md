# Plans and limits

Grovedock is open source and has no feature gates: everything on this page is a limit you can
see and, where it says so, change.

## Built-in limits

| What | Limit | Change it with |
|---|---|---|
| One call's running time | 600 s | `--request-timeout` |
| Calls in progress, whole server | 4,096 (more are refused with Unavailable, and clients back off) | `--max-in-flight` |
| Calls in progress, one connection | 256 | |
| Calls in progress, one account (platform servers) | 128 (more are refused with ResourceExhausted) | `--max-calls-per-principal` |
| One request message | 64 MiB on platform servers, 260 MiB otherwise; files of 8 MiB and more upload in chunks, so this never limits file size | `--max-request-mb` |
| Repositories open at once (platform servers) | 256; one unused for 10 minutes closes | `--max-open-repos`, `--repo-idle-minutes` |
| Organisations one person may create (multi-tenant servers) | 3 | `--max-orgs-per-account` |
| Access token lifetime | 90 days by default, a year at most | `pando token create --days` |
| Browser session | 30 days | revoke the token to sign out |
| Email sign-in link | 15 minutes, once | |
| Webhooks per repository | 20, keeping each one's last 200 deliveries | |
| Pre-land check | 10 s, then the land is refused | `--land-hook-timeout` |

## Plans

A platform server can record what each organisation uses (people, active agents, storage,
private repositories) and, on a multi-tenant server, enforce a limit on each. How usage is
counted and what happens at a limit is in
[Sign-in, organisations, invitations](platform.md#plans-and-limits).

**Grovedock Cloud**, a hosted Grovedock, is coming. Until then, everything runs on your own machines
with the same binary; see [Deploying Grovedock](deploy.md).
