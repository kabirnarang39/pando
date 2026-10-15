# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres to
[Semantic Versioning](https://semver.org/).

## [2.0.0] - 2026-10-15

### Changed (breaking)
- **Sapwood is now Pando.** Named after the Pando aspen grove: some 47,000 trunks that share
  one root system. What changed, and what to do:
  - Binaries: `sap` → `pando`, `sapwood` → `pandod`, `git-remote-sapwood` → `git-remote-pando`
    (git URLs `pando::https://…`).
  - Environment variables: `SAPWOOD_*` → `PANDO_*`; `SAP_SERVER`, `SAP_TOKEN`, `SAP_AUTHOR`,
    `SAP_OBJECTS` → `PANDO_SERVER`, `PANDO_TOKEN`, `PANDO_AUTHOR`, `PANDO_OBJECTS`.
  - Workspaces live in `.pando/` instead of `.sap/`: clone existing workspaces again.
  - Wire protocol package `sapwood.v1` → `pando.v1`: upgrade clients and servers together.
  - Packages: crates `pando-vcs` (CLI), `pando-core`, `pando-adapters`, `pando-proto`,
    `pando-server`; PyPI `pando-vcs` (import `pando`); npm `pando-vcs`; Go module
    `github.com/kabirnarang39/pando/sdk/go/v2`; image `ghcr.io/kabirnarang39/pando`;
    Homebrew `kabirnarang39/tap/pando`.
- **Stored data carries over unchanged**: object ids, changesets and SQLite databases are
  identical, and Postgres/CockroachDB tables named `sapwood_*` are renamed on first start.

### Fixed
- `pando import-git` accepts merges that list the same parent twice, which git allows and
  the Linux kernel's history contains four of; the repeated parent is imported once.

## [1.9.0] - 2026-10-15

### Added
- **Delta storage** (`sapwood compact`, flag `delta-storage`): blob and tree versions are
  stored as zstd patch-from deltas against the previous version of the same path. On
  git/git (82k commits) server storage drops from 4.6 GB to 369 MB (git's pack: 333 MB).
  Single-writer compaction behind a lease, base pinning (chains never change after use, no
  cycles), verification before every replace, hash-verified reconstruction on every read.
  Wire protocol unchanged.
- `ObjectStore::replace` on memory, SQLite, Postgres and `object_store` backends (contract-tested).
- Install with Homebrew: `brew install kabirnarang39/tap/sapwood`.
- Release workflow publishes to crates.io (`sapwood`, `sapwood-server` and the library
  crates), PyPI (`sapwood`), npm (`sapwood-client`) and the Homebrew tap, each enabled by a
  repository variable. CI builds every crate from its packaged tarball.

### Changed
- `sap refs` / the Refs API hide system refs (`_sapwood/...`).
- The client crate is now named `sapwood` (binaries `sap` and `git-remote-sapwood` are
  unchanged): `cargo install sapwood`. The protocol files moved to
  `crates/sapwood-proto/proto/` so the crate can be published.

### Compatibility
- A store compacted by this version cannot be read by v1.8.x or older servers (they do not
  understand delta payloads). Upgrade every server before running `sapwood compact`.

### Fixed
- Linux release binaries are built on Ubuntu 22.04 and need glibc 2.35 or newer (Debian 12,
  Ubuntu 22.04, RHEL 10); they previously needed glibc 2.39.
- The release container image is assembled from the release binaries instead of compiling
  arm64 under emulation, which took over an hour and failed for v1.6.0. Images for earlier
  tags can be rebuilt with the Release workflow's manual trigger.

## [1.8.0] - 2026-10-15

### Added
- **Pre-land checks** (`sapwood server --land-hook URL [--land-hook-token T]
  [--land-hook-timeout SECS]`, flag `land-checks`): every land is POSTed as JSON to a CI gate,
  code review service or policy bot, and proceeds only on a 2xx answer. The response body is
  shown to the author as the reason; timeouts and unreachable hooks refuse the land. The check
  runs once before any server-side rebase, which happens only without path conflicts, so the
  approved changes are exactly what lands. New `LandCheck` port for in-process checks.

## [1.7.0] - 2026-10-15

### Added
- **Virtual checkouts on Linux** (`sap mount DIR [--rev main] [--objects URL]`, flag
  `virtual-checkout`): mounts a revision read-only through FUSE. Directories load when first
  listed, file sizes come from object headers without downloading content, and reads of a
  large file fetch only the chunks covering the requested range. Every object is hash-verified.

### Changed
- New objects record their uncompressed size in the zstd frame header, so a file's size is
  known without decompressing it. Ids and the storage format are unchanged; older objects are
  still read (and sized by decompressing).

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

[2.0.0]: https://github.com/kabirnarang39/pando/releases/tag/v2.0.0
[1.9.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.9.0
[1.8.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.8.0
[1.7.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.7.0
[1.6.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.6.0
[1.5.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.5.0
[1.4.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.4.0
[1.3.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.3.0
[1.2.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.2.0
[1.1.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.1.0
[1.0.1]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.0.1
[1.0.0]: https://github.com/kabirnarang39/sapwood/releases/tag/v1.0.0
