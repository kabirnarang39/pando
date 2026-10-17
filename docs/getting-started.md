# Getting started

This page takes you from nothing to a landed change in about five minutes.

## Install

```sh
brew install kabirnarang39/tap/pando
```

Other ways:

```sh
# macOS and Linux, verifies the release checksum
curl -fsSL https://raw.githubusercontent.com/kabirnarang39/pando/main/install.sh | sh

# Docker
docker run -p 7420:7420 ghcr.io/kabirnarang39/pando:latest

# From source
cargo install --git https://github.com/kabirnarang39/pando pando-vcs pando-server
```

Windows: download the `.zip` from the [releases page](https://github.com/kabirnarang39/pando/releases).

You get three programs: `pando` (the client), `pandod` (server, cache and node daemon) and
`git-remote-pando` (lets plain git talk to a Pando server).

## Run a server

```sh
pandod server --listen 127.0.0.1:7420
```

With no options the server keeps everything in `./pando-data/` on SQLite and accepts
requests without authentication. That is fine on a laptop; see
[Deploying Pando](deploy.md) before you expose it.

## Import a git repository

```sh
pando import-git ~/code/my-repo --server http://127.0.0.1:7420
```

Import is deterministic and incremental: run it again later and it uploads only new
commits. `pando export-git` goes the other way, with identical trees.

## Make and land a change

```sh
pando clone http://127.0.0.1:7420 work
cd work
echo hello > hello.txt
pando commit -m "Say hello"
pando land
```

`pando commit` creates the commit on the server from the files you changed. `pando land`
moves `main` to include it. If someone landed first and your change touches different
files, the server rebases it for you; there is nothing to pull and retry.

To check out part of a large repository, add `--path`:

```sh
pando clone http://127.0.0.1:7420 api --path services/api
```

## Read without a clone

```sh
export PANDO_SERVER=http://127.0.0.1:7420
pando cat main services/api/config.yaml
pando log services/api
pando ls main services
```

Inside a workspace these commands use the workspace's server, so `PANDO_SERVER` is only
needed elsewhere.

## Mount instead of checking out

A mount shows a whole revision at once and downloads a file only when something reads it,
which suits big repositories and CI jobs that touch a few files:

```sh
mkdir ~/main && pando mount ~/main --rev main      # Ctrl-C to unmount
```

It works on Linux (FUSE), macOS (built in, nothing to install) and Windows (the built-in
Projected File System; enable it once with
`Enable-WindowsOptionalFeature -Online -FeatureName Client-ProjFS`). Mounts are read-only;
use `pando clone` to make changes. If your Mac's management policy refuses the mount, run
`pando mount ~/main --serve-only` and have an administrator run the command it prints.

## Lock files that cannot be merged

Art, models, video and other binaries cannot be merged, so two people editing one at the
same time lose someone's work. Lock the file first:

```sh
pando lock art/hero.psd          # others can still read it, but cannot land changes to it
pando locks art                  # who holds what
pando unlock art/hero.psd        # or --force to release someone else's lock
```

A land that changes a file someone else has locked is refused with
`rejected: art/hero.psd is locked by alice`. Locks need write access to the path, and
every lock and unlock is recorded in the audit log.

## Next

- Keep using git against the same server: [Coming from git](git-users.md).
- Run it for a team: [Deploying Pando](deploy.md).
