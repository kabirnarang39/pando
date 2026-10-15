# Roadmap

Items are ordered by expected impact. v1.x releases are backward compatible with the
`pando.v1` protocol and storage formats.

| Item | Why | Notes |
|---|---|---|
| Virtualized checkouts on Windows and macOS (ProjFS / FSKit) | Linux has `pando mount` (FUSE) since v1.7; writable overlays would follow | Per-OS kernel integration |
| Path-history derived index | `log -- path` currently walks first-parent history (bounded) | Derived per-path skip index |
