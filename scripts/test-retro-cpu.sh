#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
set -eu

SCRIPT_DIR=$(dirname "$0")
ROOT=$(CDPATH= cd -P "$SCRIPT_DIR/.." && pwd)
cd "$ROOT"

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf '%s\n' "GALAXY retro CPU test: missing required command '$1'." >&2
    exit 1
  fi
}

need node

GALAXY_CARGO=${GALAXY_CARGO:-cargo}
if ! command -v "$GALAXY_CARGO" >/dev/null 2>&1; then
  GALAXY_CARGO=${CARGO_HOME:-$HOME/.cargo}/bin/cargo
fi
if [ ! -x "$GALAXY_CARGO" ] && ! command -v "$GALAXY_CARGO" >/dev/null 2>&1; then
  printf '%s\n' "GALAXY retro CPU test: Rust/Cargo is required." >&2
  exit 1
fi

SAMPLES=${GALAXY_CPU_BENCH_SAMPLES:-1048576}
REPEATS=${GALAXY_CPU_BENCH_REPEATS:-5}

case $SAMPLES in
  ''|*[!0-9]*)
    printf '%s\n' "GALAXY_CPU_BENCH_SAMPLES must be a positive decimal integer." >&2
    exit 2
    ;;
esac
case $REPEATS in
  ''|*[!0-9]*)
    printf '%s\n' "GALAXY_CPU_BENCH_REPEATS must be a positive decimal integer." >&2
    exit 2
    ;;
esac
if [ "$SAMPLES" = 0 ] || [ "$REPEATS" = 0 ]; then
  printf '%s\n' "GALAXY CPU benchmark sample and repeat counts must be greater than zero." >&2
  exit 2
fi

printf '%s\n' "== Host =="
uname -s
uname -m
printf 'node='; node --version
printf 'cargo='; "$GALAXY_CARGO" --version
if command -v rustc >/dev/null 2>&1; then
  printf 'rustc='; rustc --version
else
  printf '%s\n' "rustc=resolved by Cargo/rustup"
fi

printf '\n%s\n' "== Shared deterministic vectors =="
node tests/retro-math.mjs
"$GALAXY_CARGO" test --manifest-path retro/Cargo.toml --locked --offline
"$GALAXY_CARGO" run --manifest-path retro/Cargo.toml --example retro_vectors --locked --offline

printf '\n%s\n' "== CPU projection microbenchmark =="
printf 'samples=%s repeats=%s\n' "$SAMPLES" "$REPEATS"
GALAXY_CPU_BENCH_SAMPLES=$SAMPLES \
GALAXY_CPU_BENCH_REPEATS=$REPEATS \
  "$GALAXY_CARGO" run --manifest-path retro/Cargo.toml --release --locked --offline --example cpu_bench

printf '\n%s\n' "GALAXY retro CPU validation completed."
printf '%s\n' "Benchmark timing is host-specific evidence, not a cross-machine performance claim."
