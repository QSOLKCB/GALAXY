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
  printf '%s\n' "GALAXY SIMD probe: Rust/Cargo is required." >&2
  exit 1
fi

ITEMS=${GALAXY_SIMD_ITEMS:-4194304}
REPEATS=${GALAXY_SIMD_REPEATS:-7}
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
OUTPUT=${GALAXY_SIMD_OUTPUT:-runs/cpu-simd-${STAMP}-$$}
GENERIC_TARGET=$OUTPUT/target-generic
NATIVE_TARGET=$OUTPUT/target-native
GENERIC_RECEIPT=$OUTPUT/generic.json
NATIVE_RECEIPT=$OUTPUT/native.json

mkdir -p "$OUTPUT"

{
  printf 'date_utc=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf 'uname=%s\n' "$(uname -a)"
  if [ -r /proc/cpuinfo ]; then
    grep -m1 '^model name' /proc/cpuinfo || true
    grep -m1 '^flags' /proc/cpuinfo || true
  fi
} > "$OUTPUT/host.txt"

printf '%s\n' "== GALAXY SIMD probe: generic x86-64 build =="
CARGO_TARGET_DIR="$GENERIC_TARGET" \
  "$GALAXY_CARGO" build --manifest-path cpu-runtime/Cargo.toml \
  --release --locked --offline --bin simd_probe
"$GENERIC_TARGET/release/simd_probe" \
  --items "$ITEMS" --repeats "$REPEATS" --receipt "$GENERIC_RECEIPT"

printf '\n%s\n' "== GALAXY SIMD probe: host-native build =="
CARGO_TARGET_DIR="$NATIVE_TARGET" RUSTFLAGS='-C target-cpu=native' \
  "$GALAXY_CARGO" build --manifest-path cpu-runtime/Cargo.toml \
  --release --locked --offline --bin simd_probe
"$NATIVE_TARGET/release/simd_probe" \
  --items "$ITEMS" --repeats "$REPEATS" --receipt "$NATIVE_RECEIPT"

if command -v objdump >/dev/null 2>&1; then
  for variant in generic native; do
    if [ "$variant" = generic ]; then
      binary=$GENERIC_TARGET/release/simd_probe
    else
      binary=$NATIVE_TARGET/release/simd_probe
    fi
    asm=$OUTPUT/${variant}-galaxy_hash_batch.asm
    regs=$OUTPUT/${variant}-vector-registers.txt
    objdump -d --disassemble=galaxy_hash_batch "$binary" > "$asm"
    grep -oE '%(xmm|ymm|zmm)[0-9]+' "$asm" | sort | uniq -c > "$regs" || true
  done
else
  printf '%s\n' "objdump not found; assembly evidence skipped." >&2
fi

printf '\n%s\n' "GALAXY SIMD probe completed."
printf 'output=%s\n' "$OUTPUT"
printf 'generic_receipt=%s\n' "$GENERIC_RECEIPT"
printf 'native_receipt=%s\n' "$NATIVE_RECEIPT"
printf 'host_evidence=%s\n' "$OUTPUT/host.txt"
