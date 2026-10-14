# v1.0 release checklist

Sign-off requires every box checked with evidence (test name, CI job or benchmark file).

## Features
- [x] Content-addressed objects (BLAKE3, zstd, verified everywhere): `object.rs` tests, `decode_never_panics` (fuzz)
- [x] Server-side changesets: `clone_commit_land_sync_roundtrip`
- [x] Derived data (root tree, generation, jump pointers): `derived.rs`, `is_ancestor` bench
- [x] Lock-free land + server rebase + conflict detection: `concurrent_lands_rebase_server_side`, `conflicting_land_is_rejected_and_nothing_moves`, `conflict_detection`
- [x] History: log, blame, diff, merge-base: `blame_attributes_each_line_to_its_changeset`, `checkout_branch_and_switch`
- [x] Browse: cat, ls, find (no checkout): `clone_commit_land_sync_roundtrip`
- [x] Sparse checkout: `sparse_checkout_materialises_only_requested_paths`
- [x] Tiers (cache + daemon, disk persistence): `tiers_serve_identical_objects_through_cache_and_daemon`
- [x] Storage adapters (memory, SQLite, Postgres, CockroachDB, S3/GCS/Azure/local): `contract.rs` + CI `databases`, `s3`
- [x] Auth tokens + principals, TLS: `auth_tokens_and_feature_flags_enforced`, `static_tokens`
- [x] Per-directory write ACLs + ref ACLs: `per_directory_write_and_ref_permissions`, `rules`
- [x] Feature flags on every slice: `require_maps_to_disabled`, `parse_spec`, e2e flag tests
- [x] git import (idempotent, incremental) / export (bit-identical trees): `git_import_export_roundtrip_preserves_trees_exactly`, real import of git/git (82k commits)
- [x] MCP server: `mcp_server_speaks_json_rpc_and_reads_repo`
- [x] Ops: metrics, JSON logs, health, reflection, graceful shutdown

## Quality gates
- [ ] `cargo fmt --check`, `cargo clippy -D warnings` clean
- [ ] `cargo test --workspace` green locally (macOS) and in CI (Linux, macOS, Windows)
- [ ] Postgres + CockroachDB + MinIO (S3) contract tests green in CI
- [ ] e2e suite stable across 10 consecutive runs
- [ ] `cargo audit` clean
- [ ] Docker image builds; `docker compose up` topology serves a clone end to end
- [ ] Benchmarks run and published (`bench/results/RESULTS.md`)
- [ ] Dogfood: Sapwood's own history imported, a change landed through `sap`, exported back to git with identical tree

## Repository
- [x] README, ARCHITECTURE, DECISIONS (ADRs), ROADMAP, comparisons
- [x] CONTRIBUTING, SECURITY, CODE_OF_CONDUCT, CHANGELOG, LICENSE (Apache-2.0)
- [x] Issue / PR templates, Dependabot
- [x] CI and release workflows (5 targets + multi-arch GHCR image)
- [ ] Public GitHub repository, CI green, v1.0.0 tag and release published

<!-- dogfood: this line was landed through sap itself -->
