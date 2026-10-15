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
