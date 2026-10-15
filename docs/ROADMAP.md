# Roadmap

Items are ordered by expected impact. v1.x releases are backward compatible with the
`sapwood.v1` protocol and storage formats.

| Item | Why | Notes |
|---|---|---|
| Delta / dictionary storage for blobs | Per-object zstd stores every version whole; server storage is several times git's delta packs | Server-side only; wire format unchanged |
| Virtualized checkouts (FUSE / ProjFS / FSKit) | Materialise files on first read, as Scion does | Per-OS kernel integration |
| Path-history derived index | `log -- path` currently walks first-parent history (bounded) | Derived per-path skip index |
| Daemon warm-start snapshots | New nodes start with a hot cache | Periodic snapshot of hot objects |
| Code review integration hooks | Pre-land checks, required approvals | Land-time policy port |
