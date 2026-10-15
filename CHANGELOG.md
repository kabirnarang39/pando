# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres to
[Semantic Versioning](https://semver.org/).

## [1.6.0] - 2026-10-15

### Added
- **Daemon prefetch** (`sapwood daemon --server URL --prefetch main [--prefetch-path P]
  [--prefetch-interval SECS]`, flag `daemon-prefetch`): warms a node's cache with a revision
  in the background and keeps it warm as the branch moves; already-cached objects cost
  nothing. On git/git, a new node's first full checkout went from 4.2 s to 0.35 s.

## [1.5.0] - 2026-10-15

### Added
- **SDKs for Python, Go and TypeScript** (`sdk/`): read files, list directories, search, log,
  diff, blame, refs and merge-base without a clone, with BLAKE3 verification of changesets and
  file contents. Tested in CI against a live server.
- `proto/sapwood_objects.proto`: the canonical object encodings, published for third-party
  tools; a test proves it is byte-identical to the core's encoding.

## [1.4.0] - 2026-10-15

### Added
- **Path-scoped read permissions**: `read` grants in the access rules. Restricted principals
  are denied raw object reads and get path-filtered views instead: streaming `Checkout`, filtered
  `ListDir`/`Find`, diffs limited to readable paths, and redacted history. New `Access.WhoAmI`
  lets `sap clone/sync` switch to path-scoped checkout automatically. Rules without `read`
  keep full read access for everyone.

## [1.3.0] - 2026-10-15

### Added
- **Large files**: files of 8 MiB or more are stored as FastCDC chunks plus a manifest under
  the file's own id, removing the 256 MiB file limit. Status hashing, commit upload, checkout
  and sync all stream; chunks dedupe across files and versions; the server verifies uploaded
  manifests against the file id and clients verify reassembled content end to end. Large git
  blobs are chunked on `import-git` and on `git push` through the bridge.
- `sap commit` reports how many new objects were uploaded.

### Performance
- Tiered storage is now write-around and forwards batched writes upstream intact (one
  SQLite transaction per batch instead of per object): a 1 GiB commit went from 29 s to 14 s,
  and large uploads no longer evict the server's hot read cache.

## [1.2.0] - 2026-10-15

### Added
- **SSO via OpenID Connect** (`--oidc-issuer`, `--oidc-audience`, `--oidc-claim`, flag `oidc`):
  JWT bearer tokens verified against the issuer's JWKS with discovery and automatic key
  rotation. Asymmetric algorithms only, algorithm must match the key, `iss`/`aud`/`exp`
  required. The principal claim feeds per-directory and per-ref permissions. Static tokens
  and OIDC can be used together.

## [1.1.0] - 2026-10-15

### Added
- **Two-way git bridge**: `git-remote-sapwood` lets plain git clone, fetch, pull and push
  against a Sapwood server (`git clone sapwood::http://host:7420`). Fetches are incremental;
  pushes land server-side, so a push to a branch that moved is rebased instead of rejected
  (`SAPWOOD_PUSH=strict` restores git's behaviour). Feature flag `git-bridge`. Verified by
  cloning git/git's 24,466 mainline commits through plain `git clone` (136 s, identical tree).
- Project website (GitHub Pages) and a checksum-verifying `install.sh`.

## [1.0.1] - 2026-10-15

### Fixed
- `sap commit` failed with "`x` is both a file and a directory" when a file was replaced by a
  directory of the same name (or the reverse) in one commit. The redundant delete is now
  folded into the upsert that replaces it. Found by the compaction test suite; covered by
  `file_and_directory_swaps_commit_cleanly`.

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

[1.6.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.6.0
[1.5.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.5.0
[1.4.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.4.0
[1.3.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.3.0
[1.2.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.2.0
[1.1.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.1.0
[1.0.1]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.0.1
[1.0.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.0.0
