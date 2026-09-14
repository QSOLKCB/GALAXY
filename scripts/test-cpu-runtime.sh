#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
set -eu

SCRIPT_DIR=$(dirname "$0")
ROOT=$(CDPATH= cd -P "$SCRIPT_DIR/.." && pwd)
cd "$ROOT"

GALAXY_CARGO=${GALAXY_CARGO:-cargo}
if ! command -v "$GALAXY_CARGO" >/dev/null 2>&1; then
  GALAXY_CARGO=${CARGO_HOME:-$HOME/.cargo}/bin/cargo
fi
if [ ! -x "$GALAXY_CARGO" ] && ! command -v "$GALAXY_CARGO" >/dev/null 2>&1; then
  printf '%s\n' "GALAXY CPU runtime test: Rust/Cargo is required." >&2
  exit 1
fi

LOGICAL=${GALAXY_CPU_LOGICAL:-18446744073709551615}
RESIDENT=${GALAXY_CPU_RESIDENT:-1048576}
FRAMES=${GALAXY_CPU_FRAMES:-8}
REPEATS=${GALAXY_CPU_REPEATS:-5}
SEED=${GALAXY_CPU_SEED:-303}
WORKERS=${GALAXY_CPU_WORKERS:-}
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
OUTPUT=${GALAXY_CPU_OUTPUT:-runs/cpu-runtime-${STAMP}}
RECEIPT=$OUTPUT/receipt.json

printf '%s\n' "== GALAXY native CPU runtime tests =="
"$GALAXY_CARGO" test --manifest-path cpu-runtime/Cargo.toml --locked --offline

printf '\n%s\n' "== Exact u64 / scalar-parallel verification =="
if [ -n "$WORKERS" ]; then
  "$GALAXY_CARGO" run --manifest-path cpu-runtime/Cargo.toml --release --locked --offline -- \
    verify --workers "$WORKERS"
else
  "$GALAXY_CARGO" run --manifest-path cpu-runtime/Cargo.toml --release --locked --offline -- verify
fi

printf '\n%s\n' "== Native CPU benchmark/runtime execution =="
set -- bench \
  --logical "$LOGICAL" \
  --resident "$RESIDENT" \
  --frames "$FRAMES" \
  --repeats "$REPEATS" \
  --seed "$SEED" \
  --receipt "$RECEIPT"
if [ -n "$WORKERS" ]; then
  set -- "$@" --workers "$WORKERS"
fi
"$GALAXY_CARGO" run --manifest-path cpu-runtime/Cargo.toml --release --locked --offline -- "$@"

printf '\n%s\n' "GALAXY native CPU runtime validation completed."
printf 'receipt=%s\n' "$RECEIPT"
