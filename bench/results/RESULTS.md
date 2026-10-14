# git vs Sapwood: git

* Machine: Apple M4, 10 cores
* git 2.50.1, sap 1.0.0
* History: 82443 commits, 4857 files at HEAD
* Each row: median of 5 runs (hyperfine), both systems local (git over file://, Sapwood over loopback gRPC)

| Scenario | git | Sapwood | Sapwood vs git |
|---|---:|---:|---|
| full-clone | 19,130 ms | 463 ms | 41.3x faster |
| sparse-clone (Documentation/) | 9,513 ms | 106 ms | 89.7x faster |
| read one file, no prior clone (CI) | 149 ms | 9 ms | 16.0x faster |
| status (clean, 4857 files) | 91 ms | 48 ms | 1.9x faster |
| log -n 50 -- builtin/add.c | 100 ms | 19 ms | 5.4x faster |
| blame Makefile | 528 ms | 352 ms | 1.5x faster |
| commit + publish 1 file | 130 ms | 77 ms | 1.7x faster |

## Footprint after full clone

| | git | Sapwood |
|---|---:|---:|
| Client metadata | 334M (`.git`, full history) | 380K (`.sap`, index only) |
| Server storage | 333M (delta-compressed pack) | 4.6G (per-object zstd, no deltas) |

Full-history import: 226.3 s (first run); re-import of unchanged history: 129.63 s (no uploads)

## Concurrent publishing: 16 agents x 5 changes = 80 merges to one branch

| | git (push / fetch+rebase retry) | Sapwood (land) |
|---|---:|---:|
| Wall time | 42.8 s | 3.5 s |
| Throughput | 1.9 merges/s | 22.7 merges/s |
| Client retries | 456 | 0 (server rebased 79) |
