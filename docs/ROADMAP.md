# Roadmap

What comes next for the open-source server, roughly in order. Everything here stays
Apache-2.0. Releases stay backward compatible with the `pando.v1` protocol and storage formats.

## Shipped in 3.x

Hosting many repositories; organisations, teams, roles and scoped tokens; change requests,
reviews, protected branches and required checks; issues assignable to people and agents;
webhooks; remote MCP for agents; `pando migrate` and `pando mirror`; the web console for all of
it.

## Next

| Item | Why |
|---|---|
| Platform registry on Postgres | several `pandod` replicas sharing accounts, reviews and webhooks, for high availability |
| Email notifications | review requested, assigned, mentioned, with per-person settings |
| Sign in with GitHub or Google, self-serve organisations, invitations | teams start without an administrator setting up tokens |
| Per-person and per-token rate limits | fairness on shared servers |
| Review comments carried across `pando migrate` | pull requests arrive with their discussion |
| Faster first paint on phones (inline first-screen styles, smaller first bundle) | the console's mobile Lighthouse score is 88 |
| Path-history index | `log -- path` walks first-parent history today |
| Writable virtual checkouts on macOS and Windows | Linux has a FUSE mount; macOS and Windows mount read-only |

## Later

Pando Cloud, a managed service with a free tier, and enterprise features (SAML, SCIM,
audit-log streaming, high-availability deployment templates) are planned. The open-source
server stays complete: everything shipped here stays open.
