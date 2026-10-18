# Roadmap

What comes next for the open-source server, roughly in order. Everything here stays
Apache-2.0. Releases stay backward compatible with the `pando.v1` protocol and storage formats.

## Shipped in 3.x

Hosting many repositories; organisations, teams, roles and scoped tokens; change requests,
reviews, protected branches and required checks; issues assignable to people and agents;
webhooks; remote MCP for agents; `pando migrate` and `pando mirror`; the web console for all of
it. In 3.4: browser sign-in (Google, any OpenID Connect provider, email links), multi-tenant
servers with self-serve organisations, and `pandod backup`. In 3.5: `pando login` from the
browser, invitations by email, and usage and limits per organisation.

## Next

| Item | Why |
|---|---|
| Platform registry on Postgres | several `pandod` replicas sharing accounts, reviews and webhooks, for high availability |
| Email notifications | review requested, assigned, mentioned, with per-person settings |
| Per-person and per-token rate limits | fairness on shared servers |
| Review comments carried across `pando migrate` | pull requests arrive with their discussion |
| Path-history index | `log -- path` walks first-parent history today |
| Writable virtual checkouts | `pando mount` is read-only on every platform today |

## Later

Pando Cloud, a managed service with a free tier, and enterprise features (SAML, SCIM,
audit-log streaming, high-availability deployment templates) are planned. The open-source
server stays complete: everything shipped here stays open.
