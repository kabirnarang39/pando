#!/usr/bin/env bash
# Regenerate Go stubs from ../../crates/sapwood-proto/proto (run after protocol changes).
set -euo pipefail
cd "$(dirname "$0")"
M=github.com/kabirnarang39/sapwood/sdk/go
protoc -I ../../crates/sapwood-proto/proto \
  --go_out=. --go_opt=module=$M \
  --go-grpc_out=. --go-grpc_opt=module=$M \
  --go_opt=Msapwood.proto=$M/sapwoodpb --go-grpc_opt=Msapwood.proto=$M/sapwoodpb \
  --go_opt=Msapwood_objects.proto=$M/objectspb --go-grpc_opt=Msapwood_objects.proto=$M/objectspb \
  sapwood.proto sapwood_objects.proto
