<div align="center">

# 🌳 Sapwood

**Source control for monorepos at agent scale.**

Server-side commits · lock-free landing · tiered caches · API-first reads · built-in MCP for AI agents

[![CI](https://github.com/kabirnarang39/sapwood/actions/workflows/ci.yml/badge.svg)](https://github.com/kabirnarang39/sapwood/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024-orange.svg)](https://www.rust-lang.org)
[![Release](https://img.shields.io/github/v/release/kabirnarang39/sapwood)](https://github.com/kabirnarang39/sapwood/releases)

**[Website](https://kabirnarang39.github.io/sapwood/)** · **[Benchmarks](docs/comparisons/git-vs-sapwood.md)** · **[Architecture](docs/ARCHITECTURE.md)**

</div>

---

When humans and AI agents merge **tens of thousands of changes a day** into one repository,
git's model starts to hurt: every CI job clones history it never reads, every push races a
global ref and loses, and every query needs a local copy of 100 GB of history. Anthropic
described hitting exactly this wall and building an internal system, Scion, to get past it
([*Redesigning source control*](https://www.anthropic.com/engineering/redesigning-source-control)).
Scion will not be released.

**Sapwood is an open-source, clean-room implementation of that architecture**, designed to
run on infrastructure you already have: from a single binary with SQLite on a laptop, to
stateless replicas on Postgres/CockroachDB + S3 behind regional caches.

> Sapwood is independent and not affiliated with or endorsed by Anthropic.
> See [Scion vs Sapwood](docs/comparisons/scion-vs-sapwood.md) for what comes from the post and what is ours.

## Benchmarks: git vs Sapwood

<!-- BENCH:START -->
Measured on **git/git** (82,443 commits, 4,857 files), Apple M4, release builds, median of 5 runs
([hyperfine](https://github.com/sharkdp/hyperfine)). Reproduce with [`bench/run.sh`](bench/run.sh).

| Scenario | git | Sapwood | Sapwood vs git |
|---|---:|---:|---|
| full-clone | 19,130 ms | 463 ms | 41.3x faster |
| sparse-clone (Documentation/) | 9,513 ms | 106 ms | 89.7x faster |
| read one file, no prior clone (CI) | 149 ms | 9 ms | 16.0x faster |
| status (clean, 4857 files) | 91 ms | 48 ms | 1.9x faster |
| log -n 50 -- builtin/add.c | 100 ms | 19 ms | 5.4x faster |
| blame Makefile | 528 ms | 352 ms | 1.5x faster |
| commit + publish 1 file | 130 ms | 77 ms | 1.7x faster |

**16 AI agents landing 80 changes on one branch at once:**

| | git push (fetch + rebase + retry) | sap land |
|---|---:|---:|
| Wall time | 42.8 s | **3.5 s** |
| Throughput | 1.9 merges/s | **22.7 merges/s** (12x) |
| Client retries | 456 | **0** (server rebased 79) |

Client metadata after a full clone: **380 KB** (`.sap`) vs **334 MB** (`.git`).
Where git still wins: offline work, and server storage (no delta compression yet:
4.6 GB vs a 333 MB pack; on the [roadmap](docs/ROADMAP.md)).
<!-- BENCH:END -->

Full results, methodology and the cases where git wins: [docs/comparisons/git-vs-sapwood.md](docs/comparisons/git-vs-sapwood.md).

## Why it is fast

| | git | Sapwood |
|---|---|---|
| Clone | downloads **all history** | downloads **one tree**, lazily, only the paths you ask for |
| Commit | local; publishing = push whole packs, lose races, fetch, rebase, retry | server builds the commit from the list of changed files; cost ∝ change size |
| Landing concurrent changes | one winner per race; everyone else fetches + rebases | server rebases non-conflicting changes; ref moves by compare-and-swap; **no lock, no merge queue** |
| Reading a file in CI | clone first | one RPC: `sap cat main path` |
| log / blame / diff / merge-base | local history required | server-side APIs, O(log n) ancestry via jump pointers |
| Caching | mirrors drift from the tip | immutable content-addressed objects: any tier answers any request, forever correct |
| Integrity | SHA-1 | BLAKE3, verified at every tier |

## Quick start

```sh
# 1. Install (macOS, Linux; verifies the release checksum). Windows: download the .zip from Releases.
curl -fsSL https://raw.githubusercontent.com/kabirnarang39/sapwood/main/install.sh | sh
#    or: docker run -p 7420:7420 ghcr.io/kabirnarang39/sapwood:latest
#    or: cargo install --git https://github.com/kabirnarang39/sapwood sap sapwood-server

# 2. Run a server (SQLite by default)
sapwood server --listen 127.0.0.1:7420 &

# 3. Bring your git history (idempotent, incremental, no lock-in)
sap import-git ~/code/my-repo --server http://127.0.0.1:7420

# 4. Work
sap clone http://127.0.0.1:7420 work && cd work
echo "hello" > hello.txt
sap commit -m "say hello"
sap land                      # server-side rebase + atomic ref move
```

Production topology (Postgres + S3 + regional cache + node daemon) in one command:

```sh
docker compose up -d
sap clone http://localhost:7420 work --objects http://localhost:7422
```

## Or keep using plain git

With `git-remote-sapwood` on your `PATH` (the installer puts it there), git talks to a
Sapwood server directly:

```sh
git clone sapwood::http://127.0.0.1:7420 repo
git push        # lands on the server; if main moved, the server rebases instead of rejecting
git pull --rebase
```

Fetches are incremental. Set `SAPWOOD_PUSH=strict` to get git's reject-if-behind behaviour.
Teams can adopt Sapwood one person at a time: git users and `sap` users share the same branch.

## git → sap cheat sheet

| git | sap | Notes |
|---|---|---|
| `git clone URL` | `sap clone URL DIR` | add `--path services/api` for a sparse clone |
| `git status` | `sap status` | alias `st` |
| `git add` + `git commit -m` | `sap commit -m MSG [PATHS]` | no staging area; PATHS = commit a subset |
| `git push` | `sap land` | alias `push`; server rebases if main moved |
| `git pull` | `sap sync` | alias `pull` |
| `git checkout -b x` / `git switch x` | `sap checkout -b x` / `sap checkout x` | |
| `git log -- path` | `sap log path` | server-side, no local history |
| `git diff` / `git diff A B` | `sap diff` / `sap diff A B` | |
| `git show REV` | `sap show REV` | |
| `git blame f` | `sap blame f` | |
| `git show REV:path` | `sap cat REV path` | works without a clone |
| `git ls-tree` | `sap ls REV [dir]` | works without a clone |
| (no equivalent) | `sap mount DIR --rev main` | Linux: read-only view, files fetched on first read |
| `git branch` | `sap refs` | alias `branch` |
| `git merge-base A B` | `sap merge-base A B` | |

## For AI agents: built-in MCP

```sh
claude mcp add sapwood -- sap mcp          # or any MCP client: Codex, Cursor, ...
```

Tools: `sap_cat`, `sap_ls`, `sap_find`, `sap_log`, `sap_blame`, `sap_diff`, `sap_status`,
`sap_commit`, `sap_land`, `sap_sync`. Agents read, commit and land without shelling out.

## SDKs

Read files, list directories, search and inspect history from your own tools, without a
clone. Every client verifies content against its BLAKE3 id.

| Language | Package | Example |
|---|---|---|
| Python | [`sdk/python`](sdk/python) (`sapwood`) | `Client(url).read_file("src/lib.rs")` |
| Go | [`sdk/go`](sdk/go) | `c.ReadFile(ctx, "main", "src/lib.rs")` |
| TypeScript / Node | [`sdk/typescript`](sdk/typescript) (`sapwood-client`) | `await new Client(url).readFile("src/lib.rs")` |

Any other language: generate a client from [`proto/sapwood.proto`](proto/sapwood.proto); object
encodings are published in [`proto/sapwood_objects.proto`](proto/sapwood_objects.proto).

## Architecture

```
sap (CLI/MCP) ──objects──▶ daemon (node) ──▶ cache (region) ──▶ server ──▶ SQLite | Postgres | CockroachDB
      └─────────────metadata RPCs─────────────────────────────────▶ │  └──▶ S3 | GCS | Azure | disk
```

* **Feature slices**: objects, changesets, refs, land, history, diff, merge-base, browse;
  each owns its use case, gRPC handler and **feature flag** (`SAPWOOD_FLAGS=land=off`).
* **Ports and adapters**: every infrastructure detail is a trait; every adapter passes the
  same contract test suite.
* **Plain git works**: `git clone sapwood::URL`, push and pull via the built-in remote helper.
* **SSO and permissions**: OpenID Connect login; per-directory **read** and write permissions
  with path-scoped checkout, search and redacted history for partial readers.
* **Large files**: content-defined chunking (FastCDC) with streaming upload and checkout;
  a 1 GiB file commits in 195 MB of memory, and an edit re-uploads only the changed chunks.
* **Security**: bearer tokens (constant-time), TLS, per-directory write ACLs, per-ref ACLs,
  hash verification at every tier, symlink-safe working-copy writes.
* **Operations**: Prometheus metrics, JSON logs, gRPC health + reflection, graceful shutdown.

Deep dive: [ARCHITECTURE.md](docs/ARCHITECTURE.md) · Decisions: [DECISIONS.md](docs/DECISIONS.md)

## Configuration

| Setting | Flag / env | Default |
|---|---|---|
| Metadata store | `--metadata` / `SAPWOOD_METADATA` | `sqlite://./sapwood-data/meta.db` (`memory`, `postgres://…`) |
| Object store | `--objects` / `SAPWOOD_OBJECTS` | `metadata` (`file:///…`, `s3://…`, `gs://…`, `az://…`) |
| Auth tokens | `--tokens` / `SAPWOOD_TOKENS` | none (dev mode); `alice:tok1,ci:tok2` |
| SSO (OIDC) | `--oidc-issuer`, `--oidc-audience`, `--oidc-claim` | off; e.g. Okta, Entra ID, Google, Auth0, Keycloak |
| Access rules | `--acl` / `SAPWOOD_ACL` | allow all; see [SECURITY.md](SECURITY.md) |
| Pre-land check (CI, code review) | `--land-hook URL`, `--land-hook-token`, `--land-hook-timeout` | off; see [Land checks](docs/ARCHITECTURE.md#land-checks) |
| TLS | `--tls-cert`, `--tls-key` | off |
| Metrics | `--metrics` / `SAPWOOD_METRICS` | off; e.g. `0.0.0.0:9420` |
| Feature flags | `SAPWOOD_FLAGS` | all on; e.g. `land=off,git-import=off` |
| Client token | `SAP_TOKEN` | none |

## Status

**v1.0**: the protocol (`sapwood.v1`), storage formats and CLI are stable and follow
SemVer. See the [CHANGELOG](CHANGELOG.md) and [roadmap](docs/ROADMAP.md) (virtualized
FUSE checkouts, read ACLs, delta storage, chunked large files).

## License

[Apache-2.0](LICENSE). Contributions welcome; see [CONTRIBUTING.md](CONTRIBUTING.md).
