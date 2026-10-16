# Pando

Pando is open-source source control for monorepos where many people and AI agents commit
at once. Commits, rebases and history run on the server, so a clone fetches only the files
you ask for, CI reads files without checking anything out, and concurrent changes land
without a merge queue. Plain git keeps working through a remote helper.

It runs as one binary: on a laptop with SQLite, or as stateless replicas on Postgres or
CockroachDB with S3, GCS or Azure behind regional caches.

## Where to start

- [Getting started](getting-started.md): install, run a server, import a git repository,
  and land your first change. About five minutes.
- [Coming from git](git-users.md): every git command and its Pando equivalent, and how to
  keep using git against a Pando server.
- [Deploying Pando](deploy.md): Postgres and S3, caches, single sign-on, permissions,
  pre-land checks and monitoring.
- [AI agents and SDKs](agents.md): the built-in MCP server and the Python, TypeScript and
  Go clients.

## How it differs from git

| | git | Pando |
|---|---|---|
| Clone | downloads all history | downloads one tree, lazily, only the paths you ask for |
| Commit | built locally, pushed as packs | the server builds it from the list of changed files |
| Concurrent pushes to `main` | one wins, the rest fetch, rebase and retry | the server rebases changes that touch different files and lands them all |
| Reading a file in CI | clone first | one call: `pando cat main path` |
| `log`, `blame`, `diff` | need local history | server APIs; ancestry in O(log n) |
| Integrity | SHA-1 | BLAKE3, verified at every cache tier |

Pando is not a fit when you need full history offline; git is the better tool there.
