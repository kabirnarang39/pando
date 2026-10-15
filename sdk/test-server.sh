#!/usr/bin/env bash
# Start a throwaway Pando server with a known repository for SDK tests.
#   eval "$(sdk/test-server.sh)"   # exports PANDO_URL and PANDO_TEST_PID
# Needs release binaries (cargo build --release -p pando-server -p pando).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${PANDO_BIN:-$ROOT/target/release}"
PORT="${PANDO_TEST_PORT:-7480}"
URL="http://127.0.0.1:$PORT"
WORK="$(mktemp -d)"
"$BIN/pandod" server --listen "127.0.0.1:$PORT" --metadata memory >"$WORK/server.log" 2>&1 &
PID=$!
for _ in $(seq 1 50); do "$BIN/pando" refs --help >/dev/null 2>&1 && PANDO_SERVER="$URL" "$BIN/pando" refs >/dev/null 2>&1 && break; sleep 0.1; done
"$BIN/pando" clone "$URL" "$WORK/ws" >/dev/null
cd "$WORK/ws"
mkdir -p src docs
printf 'hello pando\n' > README.md
printf 'pub fn answer() -> u32 { 41 }\n' > src/lib.rs
PANDO_AUTHOR="SDK Test <sdk@example.com>" "$BIN/pando" commit -m "initial" >/dev/null
"$BIN/pando" land >/dev/null
printf 'pub fn answer() -> u32 { 42 }\n' > src/lib.rs
printf '# Guide\n' > docs/guide.md
PANDO_AUTHOR="SDK Test <sdk@example.com>" "$BIN/pando" commit -m "fix the answer" >/dev/null
"$BIN/pando" land >/dev/null
echo "export PANDO_URL=$URL PANDO_TEST_PID=$PID"
