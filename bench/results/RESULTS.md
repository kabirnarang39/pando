# git vs Sapwood: git

* Machine: Apple M4, 10 cores
* git 2.50.1, sap 0.1.0
* History: 82443 commits, 4857 files at HEAD
* Each row: median of 5 runs (hyperfine), both systems local (git over file://, Sapwood over loopback gRPC)

| Scenario | git | Sapwood | Sapwood vs git |
|---|---:|---:|---|
| full-clone | 14,148 ms | 486 ms | 29.1x faster |
| sparse-clone (Documentation/) | 8,030 ms | 95 ms | 84.7x faster |
| read one file, no prior clone (CI) | 136 ms | 8 ms | 16.0x faster |
| status (clean, 4857 files) | 36 ms | 36 ms | 1.0x slower |
| log -n 50 -- builtin/add.c | 86 ms | 95 ms | 1.1x slower |
| blame Makefile | 390 ms | 672 ms | 1.7x slower |
| commit + publish 1 file | 136 ms | 74 ms | 1.8x faster |

## Footprint after full clone

| | git | Sapwood |
|---|---:|---:|
| Client metadata | 334M (`.git`, full history) | 380K (`.sap`, index only) |
| Server storage | 333M (delta-compressed pack) | 4.5G (per-object zstd, no deltas) |

## Concurrent publishing: 16 agents x 5 changes = 80 merges to one branch

| | git (push / fetch+rebase retry) | Sapwood (land) |
|---|---:|---:|
| Wall time | 41.2 s | 5.0 s |
| Throughput | 1.9 merges/s | 16.1 merges/s |
| Client retries | 455 | 0 (server rebased 79) |
