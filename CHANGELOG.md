# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres to
[Semantic Versioning](https://semver.org/).

## [1.0.0] - 2026-10-15

First stable release. The `sapwood.v1` wire protocol, storage formats and `sap` CLI are
covered by SemVer from this release on.

### Server (`sapwood`)
- Content-addressed object store: BLAKE3-256 ids, zstd-compressed payloads, verified on every write and every tier hop.
- Server-side changesets: clients send changed paths; the server builds Merkle trees (cost proportional to the change).
- Generic derived-data framework: root trees, generation numbers, skew-binary jump pointers (O(log n) ancestry).
- Lock-free landing: server-side rebase with path-level conflict detection plus compare-and-swap ref updates; no merge queue.
- Server-side history: log (path-filtered), blame, diff, merge-base, file browsing and path search, all without a client checkout.
- Tiered object serving: identical `Objects` RPC on server, regional cache (`sapwood cache`) and node daemon (`sapwood daemon`).
- Storage adapters: memory, SQLite (WAL), PostgreSQL / CockroachDB, and S3 / GCS / Azure / local disk via `object_store`.
- Security: bearer-token auth with principals, TLS, per-directory write ACLs and per-ref ACLs (JSON rules).
- Operations: feature flags for every feature, Prometheus metrics, JSON logs, gRPC health and reflection, graceful shutdown.

### Client (`sap`)
- `clone` (lazy, sparse with `--path`), `status`, `commit`, `land`, `sync`, `checkout [-b]`, `log`, `diff`, `show`, `cat`, `ls`, `find`, `blame`, `refs`, `merge-base`.
- git-compatible aliases: `push` → `land`, `pull` → `sync`, `st`, `ci`, `switch`, `branch`.
- `import-git` (deterministic, idempotent, incremental) and `export-git` (bit-identical trees; submodules are not imported).
- `sap mcp`: Model Context Protocol server so AI agents can read, commit and land natively.
- Parallel status hashing, batched and pipelined object fetches, symlink-safe working-copy writes.

[1.0.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.0.0
