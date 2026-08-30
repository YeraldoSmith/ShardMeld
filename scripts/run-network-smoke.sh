#!/bin/sh
set -eu

if [ "$#" -lt 3 ] || [ "$#" -gt 4 ]; then
  echo "usage: run-network-smoke.sh <fixture-directory> <new-result-directory> <shardmeld-binary> [port]" >&2
  exit 2
fi

FIXTURE_DIR=$1
RESULT_DIR=$2
BIN=$3
PORT=${4:-45872}
PEER=127.0.0.1:$PORT

if [ -e "$RESULT_DIR" ]; then
  echo "result directory already exists: $RESULT_DIR" >&2
  exit 2
fi
if [ ! -x "$BIN" ]; then
  echo "shardmeld binary is not executable: $BIN" >&2
  exit 2
fi

mkdir -p "$RESULT_DIR"
REQUESTS=$(find "$FIXTURE_DIR/missing" -type f -name '*.chunk' | wc -l | tr -d ' ')
if [ "$REQUESTS" -eq 0 ]; then
  echo "fixture contains no staged missing chunks" >&2
  exit 2
fi

"$BIN" serve-chunks \
  --source "$FIXTURE_DIR/missing" \
  --bind "$PEER" \
  --max-requests "$REQUESTS" \
  --json "$RESULT_DIR/server-report.json" \
  >"$RESULT_DIR/server.log" 2>&1 &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT

READY=0
ATTEMPT=0
while [ "$ATTEMPT" -lt 50 ]; do
  if nc -z 127.0.0.1 "$PORT" 2>/dev/null; then
    READY=1
    break
  fi
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    break
  fi
  ATTEMPT=$((ATTEMPT + 1))
  sleep 0.1
done
if [ "$READY" -ne 1 ]; then
  echo "chunk server did not become ready" >&2
  wait "$SERVER_PID" || true
  exit 1
fi

"$BIN" fetch-missing \
  --descriptor "$FIXTURE_DIR/target.meld" \
  --db "$FIXTURE_DIR/index.db" \
  --peer "$PEER" \
  --out-dir "$RESULT_DIR/fetched" \
  --json "$RESULT_DIR/fetch-report.json"

wait "$SERVER_PID"
trap - EXIT

"$BIN" rebuild \
  --descriptor "$FIXTURE_DIR/target.meld" \
  --db "$FIXTURE_DIR/index.db" \
  --missing-source "$RESULT_DIR/fetched" \
  --out "$RESULT_DIR/rebuilt-via-network.bin" \
  --json "$RESULT_DIR/rebuild-report.json"

"$BIN" verify \
  --descriptor "$FIXTURE_DIR/target.meld" \
  --file "$RESULT_DIR/rebuilt-via-network.bin" \
  --json "$RESULT_DIR/verify-report.json"
