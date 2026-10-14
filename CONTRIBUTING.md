# Contributing to Sapwood

Thanks for helping. Sapwood aims to stay small, fast and correct, so contributions are
held to a few concrete rules.

## Setup

```sh
# Rust stable (see rust-version in Cargo.toml) and protoc
brew install protobuf        # or: apt-get install protobuf-compiler
cargo test --workspace       # unit, property, contract and end-to-end tests
```

Optional, for the database adapters:

```sh
docker run -d -p 55432:5432 -e POSTGRES_PASSWORD=sapwood -e POSTGRES_DB=sapwood postgres:17-alpine
SAPWOOD_TEST_POSTGRES_URL=postgres://postgres:sapwood@localhost:55432/sapwood \
  cargo test -p sapwood-adapters --test contract
```

## Where code goes

* **Domain logic** → `crates/sapwood-core`. No I/O, no transport.
* **Infrastructure** → a new adapter in `crates/sapwood-adapters` implementing a port.
  It must pass `tests/contract.rs`; add your adapter to that file.
* **A server feature** → a new slice in `crates/sapwood-server/src/features/<name>/`
  owning its use case, gRPC handler and feature flag (`sapwood_core::flags::Flag`).
* **A client feature** → `crates/sap/src/features/<name>.rs`, returning data; printing
  belongs in `main.rs`.
* **Protocol changes** → `proto/sapwood.proto`. v1 is stable: add fields and RPCs, never
  renumber or change the meaning of existing ones.

## Before opening a PR

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

* Every behaviour change comes with a test that fails without it.
* Performance-sensitive changes include before/after numbers (`cargo bench -p sapwood-core`
  or `bench/run.sh`).
* Keep commits focused; use [Conventional Commits](https://www.conventionalcommits.org/)
  (`feat:`, `fix:`, `perf:`, `docs:`...).

## Reporting bugs

Open an issue with the `sap --version` / `sapwood --version` output, the command, what
you expected and what happened. Security issues: see [SECURITY.md](SECURITY.md).
