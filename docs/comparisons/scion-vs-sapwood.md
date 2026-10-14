# Scion vs Sapwood

Scion is Anthropic's internal source control system, described in
[*Redesigning source control*](https://www.anthropic.com/engineering/redesigning-source-control)
(October 2026). Anthropic has said it will not release Scion, so it cannot be benchmarked.
Sapwood is an independent, clean-room, open-source implementation of the publicly described
design. **No Anthropic code was used; Sapwood is not affiliated with or endorsed by Anthropic.**

Every row below is tagged:

* **[post]**: Scion behaviour as stated in Anthropic's post.
* **[sapwood]**: Sapwood's own design decision or extension.

## Design mapping

| Scion principle [post] | Sapwood implementation |
|---|---|
| Always-online clients, data fetched on demand | Workspaces hold no history; trees and blobs fetched lazily, batched and pipelined |
| Client/server asymmetry: server does history, blame, commits, merges, rebases | `history` (log, blame), `changesets` (server-side commit), `land` (server-side rebase), `diff`, `merge_base` slices |
| API-first: CI and tools need no local repository | `browse` slice (`ReadFile`, `ListDir`, `Find`), `sap cat/ls/find/log/blame/diff` with no workspace, gRPC with reflection, plus MCP **[sapwood]** |
| No full copy on any server; no packfiles; no maintenance | Content-addressed objects in pluggable storage; nothing to repack or gc |
| Commits cost scales with change size | Changeset → Merkle tree update touches `O(changes × depth)` objects (benchmarked) |
| Changeset = list of changed files vs first parent | Same model |
| Refs moved by CAS on a single row | `RefStore::compare_and_swap`; single-row conditional UPDATE on SQL backends |
| Generic derived data, defined recursively from parents, eager or deferred | `Derivation` trait; `RootTree` and `Generation` (generation number + skew-binary jump pointers) derivations |
| Skip lists for the commit graph | Skew-binary jump pointers: O(log n) first-parent ancestry with O(1) storage per changeset **[sapwood choice of structure]** |
| Regional caches + per-node daemon, identical RPCs, strict tiers | `sapwood cache` / `sapwood daemon`, same `Objects` service; read-through/write-through `Tiered` store |
| Negotiation only over what the client *wants* | Clients request exact object ids level by level; every request is for an immutable object, so any tier can answer |
| Removed the global merge-queue lock | Lock-free land loop (CAS + server rebase) |
| Per-directory permissions | Per-directory **write** + per-ref ACLs via `AccessPolicy` port (read ACLs: roadmap) |
| Sparse / virtualized working copies | Sparse checkout (`--path`). Virtualized FUSE filesystem: **roadmap** |

## Where Sapwood goes beyond Scion's described scope [sapwood]

* **Open source** (Apache-2.0), self-hostable, from a laptop to a fleet.
* **Pluggable infrastructure**: SQLite, Postgres, CockroachDB for metadata; S3, GCS, Azure,
  local disk, or the database for objects. Scion is described as tied to Spanner and
  Anthropic's internal infrastructure.
* **No lock-in**: deterministic `import-git` (idempotent, incremental) and `export-git`
  with bit-identical tree hashes (verified on git/git's 82k-commit history; submodules excluded).
* **Feature flags** on every feature, toggleable at runtime config.
* **Agent-native**: built-in MCP server so any agent can read, commit and land.
* **Verified integrity at every tier**, constant-time token auth, symlink-safe writes.

## Where Scion is ahead [post]

* **Proven at scale**: >30,000 merges/day and ~25M CI checkouts/day in production.
  Sapwood v1 is new; its numbers come from benchmarks, not years of production.
* **Virtualized filesystem checkouts** (materialise on read). Sapwood v1 has sparse checkout.
* **Integrated ecosystem**: code review (Antique), code search, IAM, batch landing tools.
* **Spanner** gives globally consistent metadata out of the box; Sapwood relies on
  Postgres/CockroachDB for that.

## Summary

Use Scion if you work at Anthropic. Use Sapwood if you want Scion's architecture
(server-side commits, lock-free landing, tiered caches, API-first reads) on infrastructure
you control, under an open-source license.
