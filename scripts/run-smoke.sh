#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
  echo "usage: run-smoke.sh <new-result-directory> <cargo-target-directory>" >&2
  exit 2
fi

RESULT_DIR=$1
BUILD_DIR=$2
PROJECT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

if [ -e "$RESULT_DIR" ]; then
  echo "result directory already exists: $RESULT_DIR" >&2
  exit 2
fi

CARGO_TARGET_DIR=$BUILD_DIR cargo build --manifest-path "$PROJECT_DIR/Cargo.toml" --release
CARGO_TARGET_DIR=$BUILD_DIR cargo run --quiet --manifest-path "$PROJECT_DIR/Cargo.toml" -p meld-core --example generate_fixture -- "$RESULT_DIR"

BIN=$BUILD_DIR/release/shardmeld
DB=$RESULT_DIR/index.db
DESCRIPTOR=$RESULT_DIR/target.meld
TARGET=$RESULT_DIR/target-v2.bin
MISSING=$RESULT_DIR/missing
REBUILT=$RESULT_DIR/rebuilt-v2.bin

"$BIN" index --source "$RESULT_DIR/sources" --db "$DB" --profile m --json "$RESULT_DIR/index-report.json"
"$BIN" describe --target "$TARGET" --out "$DESCRIPTOR" --profile m
"$BIN" compare --descriptor "$DESCRIPTOR" --db "$DB" --json "$RESULT_DIR/compare-report.json"
"$BIN" stage-missing --descriptor "$DESCRIPTOR" --target "$TARGET" --db "$DB" --out-dir "$MISSING" --json "$RESULT_DIR/stage-report.json"
"$BIN" rebuild --descriptor "$DESCRIPTOR" --db "$DB" --missing-source "$MISSING" --out "$REBUILT" --json "$RESULT_DIR/rebuild-report.json"
"$BIN" verify --descriptor "$DESCRIPTOR" --file "$REBUILT" --json "$RESULT_DIR/verify-report.json"
