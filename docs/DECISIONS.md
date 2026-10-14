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
eagerly or lazily and re-defined without touching clients. Matches the design Anthropic
described for Scion.

## ADR-005: gRPC (tonic) over HTTP/2

**Decision.** One protobuf schema, gRPC transport, identical `Objects` service on every tier.
**Alternatives.** git smart-HTTP protocol (negotiates on what the client *has*, packfiles
are monolithic and not resumable); REST/JSON (no streaming, larger payloads).
**Why.** Streaming with backpressure, multiplexing, codegen for every language (so CI tools
in Python/Go/TS can call the API), reflection and health checking out of the box.

## ADR-006: Lock-free landing via CAS + server-side rebase

**Decision.** Refs move only by compare-and-swap; the server rebases on conflict-free races.
**Alternatives.** Merge queue with a global lock (serialises all merges; Anthropic reports
removing exactly this); client-side rebase + push retry loop (git; each retry is a fetch).
**Why.** Throughput scales with disjointness of changes, not with a lock.

## ADR-007: Ports and adapters with contract tests

**Decision.** Every infrastructure dependency is a trait in `sapwood-core::ports`; each
adapter must pass a shared contract suite.
**Why.** Infrastructure is replaceable (SQLite → Postgres → CockroachDB; disk → S3) without
touching features; the contract suite turns "implements the trait" into "behaves identically".

## ADR-008: SQLite default, Postgres for HA, object_store for blobs

**Decision.** Default deployment is a single binary with SQLite (WAL). Postgres (and
Postgres-wire databases such as CockroachDB) for multi-replica deployments. Objects may
live in the metadata DB or in any `object_store` backend (S3, GCS, Azure, local).
**Alternatives.** Spanner only (Scion's choice; not available to most users); FoundationDB
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

**Decision.** `sap mcp` exposes read/commit/land tools over stdio using `rmcp`.
**Why.** Agent-agnostic integration: any MCP client can drive Sapwood without bespoke
glue. The official SDK tracks the protocol specification.

## ADR-013: v1 permissions scope

**Decision.** v1 enforces per-directory **write** permissions and per-ref permissions.
Read access is repository-wide.
**Why.** Content-addressed objects are reachable through parent trees, so path-scoped
reads require object-level authorization (tracked on the roadmap). Shipping write
protection now covers the OWNERS-style use case without a leaky read model.
