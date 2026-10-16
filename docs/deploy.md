# Deploying Pando

A single `pandod server` with SQLite is enough for a team on one machine. For more, run
stateless servers on Postgres or CockroachDB with objects in S3, GCS or Azure, and put a
regional cache and a node daemon between CI machines and the servers.

## The whole topology with Docker Compose

The repository's [`docker-compose.yml`](https://github.com/kabirnarang39/pando/blob/main/docker-compose.yml)
starts Postgres, an S3-compatible store, a server, a regional cache and a node daemon:

```sh
docker compose up -d
pando clone http://localhost:7420 work --objects http://localhost:7422
```

`--objects` sends file reads through the node daemon; commits and lands still go to the
server.

## Roles

| Process | Command | Runs where | State |
|---|---|---|---|
| Server | `pandod server` | behind a load balancer, any number | none; everything is in the stores |
| Regional cache | `pandod cache --upstream URL` | one or more per region | memory only |
| Node daemon | `pandod daemon --upstream URL` | one per CI machine | memory and local disk |
| Compaction | `pandod compact --metadata URL` | a scheduled job | none; safe while serving |

Objects are immutable and named by their BLAKE3 hash, so caches never need invalidating
and every tier verifies what it receives. Only branch pointers change, and only the
server changes them.

## Configuration

| Setting | Flag / environment | Default |
|---|---|---|
| Metadata store | `--metadata` / `PANDO_METADATA` | `sqlite://./pando-data/meta.db`; also `memory`, `postgres://…` |
| Object store | `--objects` / `PANDO_OBJECTS` | `metadata` (in the metadata store); also `file:///…`, `s3://…`, `gs://…`, `az://…` |
| Bearer tokens | `--tokens` / `PANDO_TOKENS` | none, which means no authentication; e.g. `alice:tok1,ci:tok2` |
| Single sign-on | `--oidc-issuer`, `--oidc-audience`, `--oidc-claim` | off; works with Okta, Entra ID, Google, Auth0, Keycloak |
| Access rules | `--acl` / `PANDO_ACL` | allow everything |
| Pre-land check | `--land-hook URL`, `--land-hook-token`, `--land-hook-timeout` | off |
| TLS | `--tls-cert`, `--tls-key` | off |
| Metrics | `--metrics` / `PANDO_METRICS` | off; e.g. `0.0.0.0:9420` |
| Feature flags | `PANDO_FLAGS` | all on; e.g. `land=off,git-import=off` |

Run `pandod server --help` for every option.

## Authentication and permissions

Without `--tokens` or `--oidc-issuer` the server accepts everyone and logs a warning. In
production, set at least one. With OpenID Connect, the claim named by `--oidc-claim` (for
example `email`) becomes the principal that access rules refer to.

Access rules grant read and write per directory and per branch. Principals who can read
only part of the repository get path-scoped checkouts, search and history, with changes
outside their scope removed. The rule format and the security model are in
[SECURITY.md](https://github.com/kabirnarang39/pando/blob/main/SECURITY.md).

## Pre-land checks

With `--land-hook`, the server sends each proposed land to your URL before it moves the
branch: who is landing, the branch, the changeset and its paths. Any 2xx answer lets it
land; anything else, including a timeout, refuses it. Use it to require green CI or an
approved review. The payload is described in [Architecture](ARCHITECTURE.md#land-checks).

## Storage maintenance

Run `pandod compact` on a schedule. It stores old file versions as deltas against newer
ones and returns space to the filesystem; on git/git's history this takes the store from
4.6 GB to 369 MB. Runs are incremental and safe while the server is serving.

## Backups, restores and moving between backends

`pandod copy` copies a repository from one store to another. The same command makes a
backup, restores one, and moves a repository to another backend:

```sh
# Back up the production database to a SQLite file
pandod copy --from-metadata postgres://pando@db/pando --from-objects s3://pando-objects \
            --to-metadata sqlite:///backups/pando.db

# Move a single-node server from SQLite to Postgres and S3
pandod copy --from-metadata sqlite:///var/lib/pando/meta.db \
            --to-metadata postgres://pando@db/pando --to-objects s3://pando-objects
```

It is safe while the source is serving: branch pointers are read first and written to
the destination last, and everything they point to was stored before them, so the copy
is always consistent. Objects are copied as stored, so compacted history stays compact,
and whole objects are verified against their hashes on the way. Anything the destination
already holds is skipped, so running the same copy again only moves what is new: on
git/git's history (84,050 changesets, 330,000 objects) a first copy to SQLite takes 13 s
and a repeat 3.5 s. Run it on a schedule for incremental backups. System pointers such as
the compaction lease are not copied.

## Monitoring

`--metrics` exposes Prometheus metrics: request counts and latencies per RPC
(`pando_rpc_total`, `pando_rpc_seconds`), lands, refused lands, attempts and batch sizes
(`pando_land_*`), and objects warmed by node daemons (`pando_prefetch_objects_total`). `--log-json` writes structured logs. The server
implements the standard gRPC health check.
