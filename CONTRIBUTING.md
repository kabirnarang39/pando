# Contributing to Pando

Thanks for helping. Pando aims to stay small, fast and correct, so contributions are
held to a few concrete rules.

## Setup

```sh
# Rust stable (see rust-version in Cargo.toml) and protoc
brew install protobuf        # or: apt-get install protobuf-compiler
cargo test --workspace       # unit, property, contract and end-to-end tests
```

Optional, for the database adapters:

```sh
docker run -d -p 55432:5432 -e POSTGRES_PASSWORD=pando -e POSTGRES_DB=pando postgres:17-alpine
PANDO_TEST_POSTGRES_URL=postgres://postgres:pando@localhost:55432/pando \
  cargo test -p pando-adapters --test contract
```

## Where code goes

* **Domain logic** → `crates/pando-core`. No I/O, no transport.
* **Infrastructure** → a new adapter in `crates/pando-adapters` implementing a port.
  It must pass `tests/contract.rs`; add your adapter to that file.
* **A server feature** → a new slice in `crates/pando-server/src/features/<name>/`
  owning its use case, gRPC handler and feature flag (`pando_core::flags::Flag`).
* **A client feature** → `crates/pando/src/features/<name>.rs`, returning data; printing
  belongs in `main.rs`.
* **Protocol changes** → `crates/pando-proto/proto/` (`pando.proto`, `pando_platform.proto`). v1 is stable: add fields and RPCs, never
  renumber or change the meaning of existing ones.

## Before opening a PR

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

* Every behaviour change comes with a test that fails without it.
* Performance-sensitive changes include before/after numbers (`cargo bench -p pando-core`
  or `bench/run.sh`).
* Keep commits focused; use [Conventional Commits](https://www.conventionalcommits.org/)
  (`feat:`, `fix:`, `perf:`, `docs:`...).

## Tests for new functionality

Every new feature and every behaviour change lands with automated tests in the same pull
request: unit tests next to the code, and an end-to-end test in `crates/*/tests/` for anything
a user can run. A bug fix includes a test that fails without the fix. Changes are not merged
without them. See [How Pando is tested](https://kabirnarang39.github.io/pando/docs/testing.html).

## Questions and discussion

Ask questions and discuss ideas in
[GitHub Discussions](https://github.com/kabirnarang39/pando/discussions); track bugs and
feature requests as [issues](https://github.com/kabirnarang39/pando/issues). Both are public,
searchable and open to anyone with a GitHub account. Issues, pull requests and code comments
are in English.

## Reporting bugs

Open an issue with the `pando --version` and `pandod --version` output, the command, what
you expected and what happened. Security issues: see [SECURITY.md](https://github.com/kabirnarang39/pando/blob/main/SECURITY.md).
