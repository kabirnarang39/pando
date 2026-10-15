# Architecture Decision Records

Each record states the decision, the alternatives considered and why they lost.

## ADR-001: Rust for every component

**Decision.** Server, tiers and client are written in Rust (edition 2024).
**Alternatives.** Go (fast to write, GC pauses on large heaps, larger memory per cached
object); C++ (performance parity, memory-safety risk in a network-facing parser-heavy
system); mixed Go server + Rust client (two toolchains, duplicated domain code).
**Why.** Predictable latency with no GC on hot cache paths, memory safety for code that
parses untrusted trees and streams, one domain crate shared by server and client.

## ADR-002: BLAKE3 content addressing

**Decision.** Object ids are BLAKE3-256 over `kind ‖ content`.
**Alternatives.** SHA-1 (git; broken collision resistance, SHAttered 2017); SHA-256 (git's
migration target; 3-6x slower than BLAKE3 on modern CPUs without SHA extensions).
**Why.** Cryptographic strength with SIMD throughput of several GB/s per core; domain
separation byte prevents blob/tree/changeset id confusion.

## ADR-003: zstd level 3 for stored objects, hash over uncompressed bytes

**Decision.** Store `kind ‖ zstd(content)`; hash the uncompressed content.
**Alternatives.** zlib (git; slower and worse ratio); LZ4 (faster, ~30% worse ratio);
higher zstd levels (marginal ratio, much slower writes).
**Why.** Best ratio/speed balance; hashing raw content keeps ids stable if compression
changes; every tier ships stored bytes as-is with no recompression.

## ADR-004: Changesets list changes; trees are derived data

**Decision.** A changeset records file changes relative to its first parent. Root trees
are computed by the generic derived-data mechanism.
**Alternatives.** git-style commits pointing at a full tree.
**Why.** Changeset creation cost scales with change size; derived data can be computed
eagerly or lazily and re-defined without touching clients.

## ADR-005: gRPC (tonic) over HTTP/2

**Decision.** One protobuf schema, gRPC transport, identical `Objects` service on every tier.
**Alternatives.** git smart-HTTP protocol (negotiates on what the client *has*, packfiles
are monolithic and not resumable); REST/JSON (no streaming, larger payloads).
**Why.** Streaming with backpressure, multiplexing, codegen for every language (so CI tools
in Python/Go/TS can call the API), reflection and health checking out of the box.

## ADR-006: Lock-free landing via CAS + server-side rebase

**Decision.** Refs move only by compare-and-swap; the server rebases on conflict-free races.
**Alternatives.** Merge queue with a global lock (serialises all merges; large monorepo operators
report removing exactly this); client-side rebase + push retry loop (git; each retry is a fetch).
**Why.** Throughput scales with disjointness of changes, not with a lock.

## ADR-007: Ports and adapters with contract tests

**Decision.** Every infrastructure dependency is a trait in `pando-core::ports`; each
adapter must pass a shared contract suite.
**Why.** Infrastructure is replaceable (SQLite → Postgres → CockroachDB; disk → S3) without
touching features; the contract suite turns "implements the trait" into "behaves identically".

## ADR-008: SQLite default, Postgres for HA, object_store for blobs

**Decision.** Default deployment is a single binary with SQLite (WAL). Postgres (and
Postgres-wire databases such as CockroachDB) for multi-replica deployments. Objects may
live in the metadata DB or in any `object_store` backend (S3, GCS, Azure, local).
**Alternatives.** Spanner only (not available to most users); FoundationDB
(excellent, but operationally rare); RocksDB (single-node only, no SQL ops tooling).
**Why.** Zero-setup start, standard managed services for scale, one adapter for every cloud.

## ADR-009: quick_cache for tier memory

**Decision.** Byte-weighted, sharded cache with CLOCK-Pro-style eviction.
**Alternatives.** `lru` (global lock, scan-sensitive); moka (excellent, heavier, async
eviction listeners not needed here).
**Why.** Low per-op overhead under high concurrency; resists one-off scans (CI checkouts)
evicting the hot set.

## ADR-010: git fast-export for import, fast-import for export

**Decision.** Stream git history through `git fast-export`/`fast-import`.
**Alternatives.** libgit2 / gitoxide object walking (requires re-diffing every commit).
**Why.** fast-export already emits changes relative to the first parent, which is the
changeset model. Single process, streaming, battle-tested by git itself. Import is
deterministic, so re-imports are idempotent and incremental.

## ADR-011: Feature flags on every slice

**Decision.** Each slice checks a flag at its entry; flags come from a port.
**Why.** Operators can disable a feature (e.g. `land` during an incident) without a
deploy; new features can ship dark.

## ADR-012: MCP server via the official Rust SDK

**Decision.** `pando mcp` exposes read/commit/land tools over stdio using `rmcp`.
**Why.** Agent-agnostic integration: any MCP client can drive Pando without bespoke
glue. The official SDK tracks the protocol specification.

## ADR-015: Path-scoped reads without object-level ACLs

**Decision.** A principal with restricted read access may not read objects by id at all.
The server serves them path-filtered views: a streaming `Checkout`, filtered `ListDir`/`Find`,
diffs limited to readable paths, and redacted changesets (marked `redacted`, so clients
know they cannot hash-verify them). `pando` detects the scope via `WhoAmI` and switches to
these APIs automatically.
**Alternatives.** Per-object ACLs (an object can be reachable from many paths and
revisions, so every read would need a reverse index); per-principal projected trees (new
tree ids per principal break content addressing and caching).
**Why.** Nothing a restricted reader receives names content they cannot read (unreadable
directory ids are blanked in listings), the content-addressed core stays untouched, and
full-read principals keep the fast object paths.

## ADR-013: v1 permissions scope

**Decision.** v1 enforces per-directory **write** permissions and per-ref permissions.
Read access is repository-wide.
**Why.** Content-addressed objects are reachable through parent trees, so path-scoped
reads require object-level authorization (tracked on the roadmap). Shipping write
protection now covers the OWNERS-style use case without a leaky read model.

## ADR-014: Delta storage via an offline, single-writer compaction job

**Decision.** Blob versions are delta-encoded in storage by `pandod compact`: zstd
"patch-from" frames (the previous version of the same path as a raw-content prefix, with
long-distance matching), chain depth ≤ 50 (git's default `pack.depth`). Trees are
delta-encoded the same way; they dominate storage because every changeset writes a new root. Live servers only *read* deltas: the
`DeltaObjects` decorator reconstructs and hash-verifies every object before it leaves the
storage layer, so the wire protocol, tiers and clients are unchanged.

**Alternatives.**
* Delta-encode at write time on every server: concurrent replicas can create delta cycles
  (A→B and B→A) that make both objects unreadable. Rejected: data loss is unacceptable.
* git-style packfiles: repacking requires global maintenance windows and breaks the
  "every request is one immutable object" property that makes caching trivial.
* zstd dictionaries trained per repository: ~2x, versus 20x+ for per-path deltas.

**Why this is safe.**
* One compactor at a time (lease = CAS on a reserved system ref).
* A base is *pinned* before anything is stored against it, and pinned objects are never
  converted; chains never change after something depends on them (depth headers exact,
  no cycles). A chain check guards against cycles as a second line of defence.
* Each delta is decoded and compared with the original in memory before the atomic
  `replace`; readers re-verify the BLAKE3 id on every reconstruction.
* Readers accept chains up to 256 frames (5x what compaction creates), so an unexpected
  chain can only slow a read, never make data unreadable.

**Measured.** Level 3 + long-distance matching: a one-line edit to a 22 KB source file
deltas to 45 bytes (vs 6.9 KB for standalone zstd) at the same speed as plain level 3;
higher levels cost 50-500x more time for <10% gain. On git/git (82,443 commits) compaction
converts 312k object versions (159k of them trees) in 15 minutes: storage 4.6 GB -> 369 MB
(git's own pack: 333 MB), and all 24,380 mainline commits read back with identical content.

## ADR-016: Pre-land checks as a fail-closed webhook, run before rebase

**Decision.** Landing calls a `LandCheck` port once, after authorization and before the
rebase/compare-and-swap loop. The shipped adapter POSTs the proposal to a URL; only a 2xx
answer lands, and timeouts or errors refuse the land.
**Why.** A webhook lets any CI system or review tool gate landing without Pando owning a
review database. Checking before rebase keeps the hook off the contended retry path, and is
sound because a server-side rebase only happens when no changed path conflicts, so the
approved file changes are exactly what lands. Failing closed means an outage of the gate
cannot let unreviewed code onto a protected branch.
