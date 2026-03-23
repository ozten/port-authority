#!/usr/bin/env bash
set -euo pipefail

PROTO_DIR="$(cd "$(dirname "$0")/../proto" && pwd)"
OUT_DIR="$(cd "$(dirname "$0")" && pwd)/src/lib/server/generated"
PLUGIN="$(cd "$(dirname "$0")" && pwd)/node_modules/.bin/protoc-gen-ts_proto"

mkdir -p "$OUT_DIR"

protoc \
  --plugin="$PLUGIN" \
  --ts_proto_out="$OUT_DIR" \
  --ts_proto_opt=outputServices=nice-grpc,outputServices=generic-definitions \
  --ts_proto_opt=oneof=unions \
  --ts_proto_opt=useDate=false \
  --ts_proto_opt=useOptionals=messages \
  --ts_proto_opt=esModuleInterop=true \
  --ts_proto_opt=env=node \
  --ts_proto_opt=forceLong=long \
  --ts_proto_opt=importSuffix=.js \
  --proto_path="$PROTO_DIR" \
  "$PROTO_DIR/portd.proto"

echo "Proto types generated in $OUT_DIR"
