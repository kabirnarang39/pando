#!/usr/bin/env bash
# Start a throwaway Sapwood server with a known repository for SDK tests.
#   eval "$(sdk/test-server.sh)"   # exports SAPWOOD_URL and SAPWOOD_TEST_PID
# Needs release binaries (cargo build --release -p sapwood-server -p sap).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${SAPWOOD_BIN:-$ROOT/target/release}"
PORT="${SAPWOOD_TEST_PORT:-7480}"
URL="http://127.0.0.1:$PORT"
WORK="$(mktemp -d)"
"$BIN/sapwood" server --listen "127.0.0.1:$PORT" --metadata memory >"$WORK/server.log" 2>&1 &
PID=$!
for _ in $(seq 1 50); do "$BIN/sap" refs --help >/dev/null 2>&1 && SAP_SERVER="$URL" "$BIN/sap" refs >/dev/null 2>&1 && break; sleep 0.1; done
"$BIN/sap" clone "$URL" "$WORK/ws" >/dev/null
cd "$WORK/ws"
mkdir -p src docs
printf 'hello sapwood\n' > README.md
printf 'pub fn answer() -> u32 { 41 }\n' > src/lib.rs
SAP_AUTHOR="SDK Test <sdk@example.com>" "$BIN/sap" commit -m "initial" >/dev/null
"$BIN/sap" land >/dev/null
printf 'pub fn answer() -> u32 { 42 }\n' > src/lib.rs
printf '# Guide\n' > docs/guide.md
SAP_AUTHOR="SDK Test <sdk@example.com>" "$BIN/sap" commit -m "fix the answer" >/dev/null
"$BIN/sap" land >/dev/null
echo "export SAPWOOD_URL=$URL SAPWOOD_TEST_PID=$PID"
