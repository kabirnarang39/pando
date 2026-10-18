# Scaling and operations

One `pandod server` on SQLite is enough for a team on one machine. This page covers the
processes you add as load grows, storage maintenance, and monitoring.

## Processes

| Process | Command | Runs where | State |
|---|---|---|---|
| Server | `pandod server` | behind a load balancer, any number; with `--platform`, exactly one (see below) | none; everything is in the stores |
| Regional cache | `pandod cache --upstream URL` | one or more per region | memory only |
| Node daemon | `pandod daemon --upstream URL` | one per CI machine | memory and local disk |
| Compaction | `pandod compact --metadata URL` | a scheduled job | none; safe while serving |

A platform server (`--platform`) is single-node: its registry (accounts, tokens, change
requests, reviews, issues, protections, webhooks) is one SQLite connection in one process. Run
exactly one active `pandod server --platform` per registry, and never put the registry on a
network filesystem; scale reads with regional caches instead. Without `--platform`, servers
over Postgres and an object store can run behind a load balancer in any number.

Objects are immutable and named by their BLAKE3 hash, so caches never need invalidating
and every tier verifies what it receives. Only branch pointers change, and only the
server changes them.

## Storage maintenance

Run `pandod compact` on a schedule. It stores old file versions as deltas against newer
ones and returns space to the filesystem; on git/git's history this takes the store from
4.6 GB to 369 MB. Runs are incremental and safe while the server is serving.

## Monitoring

`--metrics ADDR` serves three endpoints on its own port, without authentication (keep the port
private to your network and monitoring):

| Path | Answer |
|---|---|
| `/metrics` | Prometheus metrics: request counts and latencies per RPC (`pando_rpc_total`, `pando_rpc_seconds`; methods the protocol does not define are labelled `unknown`), lands, refused lands, attempts and batch sizes (`pando_land_*`), and objects warmed by node daemons (`pando_prefetch_objects_total`) |
| `/healthz` | liveness: `200 ok` while the process answers |
| `/readyz` | readiness: `200 ready`, or `503 not ready` when the registry can't be used or a local data directory (the metadata, objects and registry directories) can't take a synced write, and from the moment shutdown starts. Checked every 5 seconds; the reason goes to the log |

The standard gRPC health check (`grpc.health.v1.Health`) needs no token either and reports
`NOT_SERVING` whenever `/readyz` would answer 503, for load balancers that speak gRPC.
`--log-json` writes structured logs.

On SIGTERM or Ctrl-C the server stops being ready, finishes the calls in progress (lands
included), stops the webhook worker (deliveries in flight finish or are retried after a
restart) and exits. Give it the longest call you allow (`--request-timeout`) before a hard kill.
