#!/usr/bin/env bash
# Regenerate gRPC/protobuf stubs from ../../crates/pando-proto/proto (run after protocol changes).
set -euo pipefail
cd "$(dirname "$0")"
OUT=src/pando/_gen
uv run --group dev python -m grpc_tools.protoc -I ../../crates/pando-proto/proto \
  --python_out=$OUT --grpc_python_out=$OUT --pyi_out=$OUT \
  pando.proto pando_objects.proto
# grpc_tools emits absolute imports; make them package-relative.
sed -i.bak 's/^import pando_pb2 as/from . import pando_pb2 as/' $OUT/pando_pb2_grpc.py && rm $OUT/*.bak
rm -f $OUT/pando_objects_pb2_grpc.py  # message-only file, no services
