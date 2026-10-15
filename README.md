<div align="center">

# 🌳 Pando

**Source control for monorepos at agent scale.**

<sub>Named after <a href="https://en.wikipedia.org/wiki/Pando_(tree)">Pando</a>, a grove of some 47,000 aspen trunks that share one root system: one repository, many agents.</sub>

Server-side commits · landing without a merge queue · tiered caches · API-first reads · built-in MCP for AI agents

[![CI](https://github.com/kabirnarang39/pando/actions/workflows/ci.yml/badge.svg)](https://github.com/kabirnarang39/pando/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024-orange.svg)](https://www.rust-lang.org)
[![Release](https://img.shields.io/github/v/release/kabirnarang39/pando)](https://github.com/kabirnarang39/pando/releases)

**[Website](https://kabirnarang39.github.io/pando/)** · **[Benchmarks](docs/comparisons/git-vs-pando.md)** · **[Architecture](docs/ARCHITECTURE.md)**

<img src="docs/assets/demo.gif" alt="Terminal recording on git/git's 82,443-commit history: git clone takes about 22 seconds, pando clone under one second; then pando log and pando cat answer in about 10 ms without a clone." width="820">

<sub>Live recording, both servers on one laptop. Reproduce with <a href="bench/run.sh"><code>bench/run.sh</code></a> and <a href="docs/assets/demo.tape"><code>docs/assets/demo.tape</code></a>.</sub>

</div>

---

When humans and AI agents merge **tens of thousands of changes a day** into one repository,
git's model starts to hurt: every CI job clones history it never reads, every push races a
global ref and loses, and every query needs a local copy of 100 GB of history. The largest
engineering organisations solved this with in-house systems that move history and commit
creation to the server.

**Pando is an open-source source control system built on that model**, designed to run on
infrastructure you already have: from a single binary with SQLite on a laptop, to stateless
replicas on Postgres/CockroachDB + S3 behind regional caches. See [Prior art](docs/PRIOR_ART.md)
for the public work it builds on and how it compares.

## Benchmarks: git vs Pando

<!-- BENCH:START -->
Measured on **git/git** (82,443 commits, 4,857 files), Apple M4, release builds, median of 5 runs
([hyperfine](https://github.com/sharkdp/hyperfine)). Reproduce with [`bench/run.sh`](bench/run.sh).

| Scenario | git | Pando | Pando vs git |
|---|---:|---:|---|
| full-clone | 19,130 ms | 463 ms | 41.3x faster |
| sparse-clone (Documentation/) | 9,513 ms | 106 ms | 89.7x faster |
| read one file, no prior clone (CI) | 149 ms | 9 ms | 16.0x faster |
| status (clean, 4857 files) | 91 ms | 48 ms | 1.9x faster |
| log -n 50 -- builtin/add.c | 100 ms | 19 ms | 5.4x faster |
| blame Makefile | 528 ms | 352 ms | 1.5x faster |
| commit + publish 1 file | 130 ms | 77 ms | 1.7x faster |

**16 AI agents landing 80 changes on one branch at once:**

| | git push (fetch + rebase + retry) | pando land |
|---|---:|---:|
| Wall time | 42.8 s | **3.5 s** |
| Throughput | 1.9 merges/s | **22.7 merges/s** (12x) |
| Client retries | 456 | **0** (server rebased 79) |

Client metadata after a full clone: **380 KB** (`.pando`) vs **334 MB** (`.git`).
Server storage after `pandod compact` (delta storage): **369 MB vs git's 333 MB pack**
(1.1x; 4.6 GB before compaction). Where git still wins: offline work.
<!-- BENCH:END -->

**1,000 agents landing on one branch at once** (each agent on its own connection, writing its
own directory; 30 s window, fresh store per run, same laptop):

| Agents | git push (fetch + rebase + retry) | pando land | pando p99 latency | pando errors |
|---:|---:|---:|---:|---:|
| 16 | 2.0 lands/s | **758 lands/s** | 55 ms | 0 |
| 200 | 0.5 lands/s | **1,012 lands/s** | 0.30 s | 0 |
| 1,000 | 0.2 lands/s (5 in 30 s) | **504 lands/s** (907 across 64 directories) | 2.7 s | 0 |
| 5,000 | not run | **172 lands/s** (531 across 64 directories) | 16 s | 0 |

One `pandod` on SQLite. For scale: Anthropic reported over 30,000 merges per day in
September 2026 and Google about 40,000 commits per workday in 2015, both under one per second on average.
Reproduce with [`examples/land_load.rs`](crates/pando/examples/land_load.rs) and
[`bench/git_push_load.py`](bench/git_push_load.py).

Full results, methodology and the cases where git wins: [docs/comparisons/git-vs-pando.md](docs/comparisons/git-vs-pando.md).

## Why it is fast

| | git | Pando |
|---|---|---|
| Clone | downloads **all history** | downloads **one tree**, lazily, only the paths you ask for |
| Commit | local; publishing = push whole packs, lose races, fetch, rebase, retry | server builds the commit from the list of changed files; cost ∝ change size |
| Landing concurrent changes | one winner per race; everyone else fetches + rebases | server rebases non-conflicting changes in batches; one compare-and-swap per batch; **no merge queue, no client retries** |
| Reading a file in CI | clone first | one RPC: `pando cat main path` |
| log / blame / diff / merge-base | local history required | server-side APIs, O(log n) ancestry via jump pointers |
| Caching | mirrors drift from the tip | immutable content-addressed objects: any tier answers any request, forever correct |
| Integrity | SHA-1 | BLAKE3, verified at every tier |

## Quick start

```sh
# 1. Install (macOS, Linux; verifies the release checksum). Windows: download the .zip from Releases.
curl -fsSL https://raw.githubusercontent.com/kabirnarang39/pando/main/install.sh | sh
#    or: brew install kabirnarang39/tap/pando
#    or: docker run -p 7420:7420 ghcr.io/kabirnarang39/pando:latest
#    or: cargo install --git https://github.com/kabirnarang39/pando pando pando-server

# 2. Run a server (SQLite by default)
pandod server --listen 127.0.0.1:7420 &

# 3. Bring your git history (idempotent, incremental, no lock-in)
pando import-git ~/code/my-repo --server http://127.0.0.1:7420

# 4. Work
pando clone http://127.0.0.1:7420 work && cd work
echo "hello" > hello.txt
pando commit -m "say hello"
pando land                      # server-side rebase + atomic ref move
```

Production topology (Postgres + S3 + regional cache + node daemon) in one command:

```sh
docker compose up -d
pando clone http://localhost:7420 work --objects http://localhost:7422
```

## Or keep using plain git

With `git-remote-pando` on your `PATH` (the installer puts it there), git talks to a
Pando server directly:

```sh
git clone pando::http://127.0.0.1:7420 repo
git push        # lands on the server; if main moved, the server rebases instead of rejecting
git pull --rebase
```

Fetches are incremental. Set `PANDO_PUSH=strict` to get git's reject-if-behind behaviour.
Teams can adopt Pando one person at a time: git users and `pando` users share the same branch.

## git → pando cheat sheet

| git | pando | Notes |
|---|---|---|
| `git clone URL` | `pando clone URL DIR` | add `--path services/api` for a sparse clone |
| `git status` | `pando status` | alias `st` |
| `git add` + `git commit -m` | `pando commit -m MSG [PATHS]` | no staging area; PATHS = commit a subset |
| `git push` | `pando land` | alias `push`; server rebases if main moved |
| `git pull` | `pando sync` | alias `pull` |
| `git checkout -b x` / `git switch x` | `pando checkout -b x` / `pando checkout x` | |
| `git log -- path` | `pando log path` | server-side, no local history |
| `git diff` / `git diff A B` | `pando diff` / `pando diff A B` | |
| `git show REV` | `pando show REV` | |
| `git blame f` | `pando blame f` | |
| `git show REV:path` | `pando cat REV path` | works without a clone |
| `git ls-tree` | `pando ls REV [dir]` | works without a clone |
| (no equivalent) | `pando mount DIR --rev main` | Linux: read-only view, files fetched on first read |
| `git branch` | `pando refs` | alias `branch` |
| `git merge-base A B` | `pando merge-base A B` | |

## For AI agents: built-in MCP

```sh
claude mcp add pando -- pando mcp          # or any MCP client: Codex, Cursor, ...
```

Tools: `sap_cat`, `sap_ls`, `sap_find`, `sap_log`, `sap_blame`, `sap_diff`, `sap_status`,
`sap_commit`, `sap_land`, `sap_sync`. Agents read, commit and land without shelling out.

## SDKs

Read files, list directories, search and inspect history from your own tools, without a
clone. Every client verifies content against its BLAKE3 id.

| Language | Package | Example |
|---|---|---|
| Python | [`sdk/python`](sdk/python) (`pando`) | `Client(url).read_file("src/lib.rs")` |
| Go | [`sdk/go`](sdk/go) | `c.ReadFile(ctx, "main", "src/lib.rs")` |
| TypeScript / Node | [`sdk/typescript`](sdk/typescript) (`pando-client`) | `await new Client(url).readFile("src/lib.rs")` |

Any other language: generate a client from [`pando.proto`](crates/pando-proto/proto/pando.proto); object
encodings are published in [`pando_objects.proto`](crates/pando-proto/proto/pando_objects.proto).

## Architecture

```
pando (CLI/MCP) ──objects──▶ daemon (node) ──▶ cache (region) ──▶ server ──▶ SQLite | Postgres | CockroachDB
      └─────────────metadata RPCs─────────────────────────────────▶ │  └──▶ S3 | GCS | Azure | disk
```

* **Feature slices**: objects, changesets, refs, land, history, diff, merge-base, browse;
  each owns its use case, gRPC handler and **feature flag** (`PANDO_FLAGS=land=off`).
* **Ports and adapters**: every infrastructure detail is a trait; every adapter passes the
  same contract test suite.
* **Plain git works**: `git clone pando::URL`, push and pull via the built-in remote helper.
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
| Metadata store | `--metadata` / `PANDO_METADATA` | `sqlite://./pando-data/meta.db` (`memory`, `postgres://…`) |
| Object store | `--objects` / `PANDO_OBJECTS` | `metadata` (`file:///…`, `s3://…`, `gs://…`, `az://…`) |
| Auth tokens | `--tokens` / `PANDO_TOKENS` | none (dev mode); `alice:tok1,ci:tok2` |
| SSO (OIDC) | `--oidc-issuer`, `--oidc-audience`, `--oidc-claim` | off; e.g. Okta, Entra ID, Google, Auth0, Keycloak |
| Access rules | `--acl` / `PANDO_ACL` | allow all; see [SECURITY.md](SECURITY.md) |
| Pre-land check (CI, code review) | `--land-hook URL`, `--land-hook-token`, `--land-hook-timeout` | off; see [Land checks](docs/ARCHITECTURE.md#land-checks) |
| TLS | `--tls-cert`, `--tls-key` | off |
| Metrics | `--metrics` / `PANDO_METRICS` | off; e.g. `0.0.0.0:9420` |
| Feature flags | `PANDO_FLAGS` | all on; e.g. `land=off,git-import=off` |
| Client token | `PANDO_TOKEN` | none |

## Status

**v1.0**: the protocol (`pando.v1`), storage formats and CLI are stable and follow
SemVer. See the [CHANGELOG](CHANGELOG.md) and [roadmap](docs/ROADMAP.md) (virtualized
FUSE checkouts, read ACLs, delta storage, chunked large files).

## License

[Apache-2.0](LICENSE). Contributions welcome; see [CONTRIBUTING.md](CONTRIBUTING.md).
