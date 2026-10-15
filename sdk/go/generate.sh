#!/usr/bin/env bash
# Regenerate Go stubs from ../../crates/pando-proto/proto (run after protocol changes).
set -euo pipefail
cd "$(dirname "$0")"
M=github.com/kabirnarang39/pando/sdk/go/v2
protoc -I ../../crates/pando-proto/proto \
  --go_out=. --go_opt=module=$M \
  --go-grpc_out=. --go-grpc_opt=module=$M \
  --go_opt=Mpando.proto=$M/pandopb --go-grpc_opt=Mpando.proto=$M/pandopb \
  --go_opt=Mpando_objects.proto=$M/objectspb --go-grpc_opt=Mpando_objects.proto=$M/objectspb \
  pando.proto pando_objects.proto
