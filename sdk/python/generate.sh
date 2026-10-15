#!/usr/bin/env bash
# Regenerate gRPC/protobuf stubs from ../../proto (run after protocol changes).
set -euo pipefail
cd "$(dirname "$0")"
OUT=src/sapwood/_gen
uv run --group dev python -m grpc_tools.protoc -I ../../proto \
  --python_out=$OUT --grpc_python_out=$OUT --pyi_out=$OUT \
  sapwood.proto sapwood_objects.proto
# grpc_tools emits absolute imports; make them package-relative.
sed -i.bak 's/^import sapwood_pb2 as/from . import sapwood_pb2 as/' $OUT/sapwood_pb2_grpc.py && rm $OUT/*.bak
rm -f $OUT/sapwood_objects_pb2_grpc.py  # message-only file, no services
