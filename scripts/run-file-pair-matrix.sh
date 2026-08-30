#!/bin/sh
set -eu

if [ "$#" -ne 4 ]; then
  echo "usage: run-file-pair-matrix.sh <authorized-source-directory> <target-file> <new-result-directory> <shardmeld-binary>" >&2
  exit 2
fi

SOURCE_DIR=$1
TARGET=$2
RESULT_DIR=$3
BIN=$4

if [ ! -d "$SOURCE_DIR" ]; then
  echo "source directory does not exist: $SOURCE_DIR" >&2
  exit 2
fi
if [ ! -f "$TARGET" ]; then
  echo "target file does not exist: $TARGET" >&2
  exit 2
fi
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
  "$BIN" index --source "$SOURCE_DIR" --db "$PROFILE_DIR/index.db" --profile "$PROFILE" --json "$PROFILE_DIR/index-report.json"
  "$BIN" describe --target "$TARGET" --out "$PROFILE_DIR/target.meld" --profile "$PROFILE"
  "$BIN" compare --descriptor "$PROFILE_DIR/target.meld" --db "$PROFILE_DIR/index.db" --json "$PROFILE_DIR/compare-report.json"
done

# Profile M is the default candidate. Prove that its plan can reconstruct the
# exact target, rather than reporting a reuse estimate only.
M_DIR=$RESULT_DIR/m
"$BIN" stage-missing --descriptor "$M_DIR/target.meld" --target "$TARGET" --db "$M_DIR/index.db" --out-dir "$M_DIR/missing" --json "$M_DIR/stage-report.json"
"$BIN" rebuild --descriptor "$M_DIR/target.meld" --db "$M_DIR/index.db" --missing-source "$M_DIR/missing" --out "$M_DIR/rebuilt.bin" --json "$M_DIR/rebuild-report.json"
"$BIN" verify --descriptor "$M_DIR/target.meld" --file "$M_DIR/rebuilt.bin" --json "$M_DIR/verify-report.json"
