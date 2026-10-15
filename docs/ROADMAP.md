# Roadmap

Items are ordered by expected impact. v1.x releases are backward compatible with the
`sapwood.v1` protocol and storage formats.

| Item | Why | Notes |
|---|---|---|
| Delta / dictionary storage for blobs | Per-object zstd stores every version whole; server storage is several times git's delta packs | Server-side only; wire format unchanged |
| Virtualized checkouts on Windows and macOS (ProjFS / FSKit) | Linux has `sap mount` (FUSE) since v1.7; writable overlays would follow | Per-OS kernel integration |
| Path-history derived index | `log -- path` currently walks first-parent history (bounded) | Derived per-path skip index |
| Code review integration hooks | Pre-land checks, required approvals | Land-time policy port |
