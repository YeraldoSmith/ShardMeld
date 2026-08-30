#!/bin/sh
set -eu

if [ "$#" -ne 3 ]; then
  echo "usage: run-profile-matrix.sh <fixture-directory> <new-result-directory> <shardmeld-binary>" >&2
  exit 2
fi

FIXTURE_DIR=$1
RESULT_DIR=$2
BIN=$3

if [ -e "$RESULT_DIR" ]; then
  echo "result directory already exists: $RESULT_DIR" >&2
  exit 2
fi
if [ ! -x "$BIN" ]; then
  echo "shardmeld binary is not executable: $BIN" >&2
  exit 2
fi

mkdir -p "$RESULT_DIR"
for PROFILE in s m l; do
  PROFILE_DIR=$RESULT_DIR/$PROFILE
  mkdir -p "$PROFILE_DIR"
  "$BIN" index --source "$FIXTURE_DIR/sources" --db "$PROFILE_DIR/index.db" --profile "$PROFILE" --json "$PROFILE_DIR/index-report.json"
  "$BIN" describe --target "$FIXTURE_DIR/target-v2.bin" --out "$PROFILE_DIR/target.meld" --profile "$PROFILE"
  "$BIN" compare --descriptor "$PROFILE_DIR/target.meld" --db "$PROFILE_DIR/index.db" --json "$PROFILE_DIR/compare-report.json"
done
