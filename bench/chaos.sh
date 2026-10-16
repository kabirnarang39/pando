#!/bin/bash
# Landing under real failures: three pandod replicas on one Postgres, clients landing on all
# of them (examples/land_chaos.rs) through a proxy that cuts connections mid-reply
# (bench/chaos_proxy.py), while replicas are killed with SIGKILL and restarted and the
# database is paused. The client checks the invariants at the end and exits non-zero if
# any fails.
#
#   bench/chaos.sh [SECONDS]        needs Docker; builds release binaries first
set -euo pipefail
SECS="${1:-60}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release"
PG=pando-chaos-pg
PORTS=(7441 7442 7443)
DB="postgres://postgres:pando@127.0.0.1:55432/postgres"
LOGS="$(mktemp -d)"

cargo build --release -q -p pando-server --manifest-path "$ROOT/Cargo.toml"
cargo build --release -q -p pando-vcs --example land_chaos --manifest-path "$ROOT/Cargo.toml"

cleanup() {
  for p in "${PORTS[@]}"; do pkill -9 -f "listen 127.0.0.1:$p" 2>/dev/null || true; done
  pkill -f "chaos_proxy.py" 2>/dev/null || true
  docker rm -f "$PG" >/dev/null 2>&1 || true
}
trap cleanup EXIT
cleanup
docker run -d --name "$PG" -e POSTGRES_PASSWORD=pando -p 55432:5432 postgres:17-alpine >/dev/null
until docker exec "$PG" pg_isready -U postgres >/dev/null 2>&1; do sleep 0.5; done
sleep 1

start() { "$BIN/pandod" server --listen "127.0.0.1:$1" --metadata "$DB" >>"$LOGS/$1.log" 2>&1 & }
for p in "${PORTS[@]}"; do start "$p"; done
sleep 2

python3 "$ROOT/bench/chaos_proxy.py" 7451:7441 7452:7442 7453:7443 --drop "${DROP:-0.02}" &
sleep 1
servers="http://127.0.0.1:7451,http://127.0.0.1:7452,http://127.0.0.1:7453"
direct=$(printf "http://127.0.0.1:%s," "${PORTS[@]}"); direct=${direct%,}
"$BIN/examples/land_chaos" --servers "$servers" --direct "$direct" --secs "$SECS" --seed "$RANDOM" &
CLIENT=$!

kills=0; pauses=0; end=$(( $(date +%s) + SECS ))
while [ "$(date +%s)" -lt "$end" ]; do
  sleep "$(( RANDOM % 3 + 1 )).$(( RANDOM % 10 ))"
  if [ $(( RANDOM % 6 )) -eq 0 ]; then
    docker pause "$PG" >/dev/null; sleep 1; docker unpause "$PG" >/dev/null; pauses=$((pauses + 1))
  else
    p=${PORTS[$(( RANDOM % 3 ))]}
    pkill -9 -f "listen 127.0.0.1:$p" || true; kills=$((kills + 1))
    sleep "0.$(( RANDOM % 9 + 1 ))"; start "$p"
  fi
done
echo "chaos: $kills replica kills, $pauses database pauses"
wait "$CLIENT"
