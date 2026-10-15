# git vs Pando

Both tools are excellent at different things. This page says where each one wins, with
measurements you can reproduce (`bench/run.sh`).

## Measured results

<!-- BENCH:START -->
| Scenario | git | Pando | Pando vs git |
|---|---:|---:|---|
| full-clone | 19,130 ms | 463 ms | 41.3x faster |
| sparse-clone (Documentation/) | 9,513 ms | 106 ms | 89.7x faster |
| read one file, no prior clone (CI) | 149 ms | 9 ms | 16.0x faster |
| status (clean, 4857 files) | 91 ms | 48 ms | 1.9x faster |
| log -n 50 -- builtin/add.c | 100 ms | 19 ms | 5.4x faster |
| blame Makefile | 528 ms | 352 ms | 1.5x faster |
| commit + publish 1 file | 130 ms | 77 ms | 1.7x faster |

`status` measured at parity (36 ms vs 36 ms) in an earlier run on the same machine; treat it
as parity to ~2x faster. Other rows were stable across runs.

### Footprint after full clone

| | git | Pando |
|---|---:|---:|
| Client metadata | 334M (`.git`, full history) | 380K (`.pando`, index only) |
| Server storage | 333M (delta-compressed pack) | 4.6G per-object zstd; 369M after `pandod compact` |

Full-history import: 226 s for 82,443 commits (first run). Re-import of unchanged history
uploads nothing.

### Concurrent publishing: 16 agents x 5 changes = 80 merges to one branch

| | git (push / fetch+rebase retry) | Pando (land) |
|---|---:|---:|
| Wall time | 42.8 s | 3.5 s |
| Throughput | 1.9 merges/s | 22.7 merges/s |
| Client retries | 456 | 0 (server rebased 79) |
<!-- BENCH:END -->

## Landing at scale: 16 to 5,000 concurrent agents

Every agent holds its own connection and loops: read the branch head, upload one new file
in its own directory, create a changeset on that head, land it. All changes are
non-conflicting, so every lost race is resolved by the server (Pando) or by the client's
fetch-rebase-push loop (git). Lands that finish after the 30 s window are not counted.

| Agents | git lands/s | git p99 | Pando lands/s | Pando p50 | Pando p99 | Pando errors |
|---:|---:|---:|---:|---:|---:|---:|
| 16 | 1.9 | 23.6 s | 520 | 23 ms | 118 ms | 0 |
| 200 | 0.6 | 28.5 s | 581 | 335 ms | 1.26 s | 0 |
| 1,000 | 0.1 (4 lands) | 33.3 s | 387 | 2.74 s | 2.93 s | 0 |
| 2,000 | not run | | 232 | 7.6 s | 11.0 s | 0 |
| 5,000 | not run | | 167 | 16.2 s | 16.3 s | 0 |

* Setup: Apple M4 (10 cores), macOS 26.6, release build, one `pandod` on SQLite, a fresh
  store per run. git pushes to a local bare repository over `file://`, its best case: no
  network and no server-side hooks. Agents and server share the machine.
* Pando: `cargo run --release --example land_load -- --agents N --secs 30` against
  `pandod server`. git: `python3 bench/git_push_load.py --agents N --secs 30 --work DIR`.
* Before batching (ADR-017), the same Pando build managed 105 lands/s at 16 agents and
  10 lands/s with 237 "too contended" errors at 200.
* Limits: past 1,000 agents throughput falls because the same process also serves every
  agent's uploads and changeset creation, and each agent's directory adds an entry to one
  shared directory that every land rewrites. At 5,000 agents almost every agent landed once
  in the window, so that row mostly measures the first round. A multi-replica deployment
  on Postgres has not been measured yet.
* git at 2,000 and 5,000 agents was not run: at 1,000 it landed 4 changes in 30 s.


### Methodology

* Corpus: [git/git](https://github.com/git/git) (82,443 commits, ~4,900 files): real,
  public, and with 20 years of history.
* git is served over `file://` (the real pack protocol, no network latency) with
  `uploadpack.allowFilter` enabled so partial clone is available to git.
* Pando runs as a release-build server on SQLite, over loopback gRPC.
* For each scenario git gets its best practice: partial clone + sparse checkout for sparse
  clones, `--depth 1 --filter=blob:none` for single-file CI reads, `--first-parent` for
  log/blame (Pando's history is first-parent, i.e. what landed).
* [hyperfine](https://github.com/sharkdp/hyperfine) median of 5 runs after a warm-up run.

## Where Pando wins, and why

* **Clones**: Pando transfers one tree's worth of objects, never history. The gap
  grows with history length; at monorepo scale git's full clone is the dominant CI cost.
* **CI reads**: `pando cat main path` is one RPC. git needs at least a shallow, filtered clone.
* **Publishing under contention**: git's push loses every race and must fetch + rebase +
  retry on the client. Pando rebases on the server and moves the ref by compare-and-swap,
  so concurrent non-conflicting lands all succeed without client retries.
* **Thin clients**: a Pando workspace's metadata is an index, not a copy of history.

## Where git wins

* **Offline work**: git has everything locally. Pando needs the server for history,
  commits and landing (by design).
* **Server storage (before compaction)**: uncompacted, Pando stores every object version
  whole (4.6 GB for git/git vs a 333 MB pack). After `pandod compact` (delta storage) the
  same store is **369 MB, 1.1x git's pack**.
* **Cold path-filtered history**: Pando's `log -- path` and `blame` are fast once the
  server's immutable metadata cache is warm (measured above); on a cold server they walk
  first-parent history from storage. A derived per-path index is on the roadmap.
* **Ecosystem**: every tool speaks git. Pando offers lossless `import-git` / `export-git`
  so you are never locked in.

## Semantics differences

| | git | Pando |
|---|---|---|
| Commit | local snapshot | server-side changeset (list of changed files vs first parent) |
| Staging area | yes | no; `pando commit [PATHS]` commits all or a subset |
| Branches | cheap, everywhere | supported (`pando checkout -b`), but `main` is the default lane; refs are global state |
| History shown by log/blame | full DAG | first-parent (what landed) |
| Submodules | yes | not supported (skipped on import) |
| Hash | SHA-1 (SHA-256 opt-in) | BLAKE3 |
