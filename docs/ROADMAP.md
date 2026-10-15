# Roadmap

Items are ordered by expected impact. v1.x releases are backward compatible with the
`sapwood.v1` protocol and storage formats.

| Item | Why | Notes |
|---|---|---|
| Virtualized checkouts (FUSE / ProjFS / FSKit) | Materialise files on first read, as Scion does | Per-OS kernel integration |
| Path-scoped read ACLs | v1 read access is repository-wide | Needs object-level authorization |
| Path-history derived index | `log -- path` currently walks first-parent history (bounded) | Derived per-path skip index |
| Daemon warm-start snapshots | New nodes start with a hot cache | Periodic snapshot of hot objects |
| Code review integration hooks | Pre-land checks, required approvals | Land-time policy port |
| Language SDKs (Python, Go, TypeScript) | API-first tooling | Generated from `proto/sapwood.proto` |
