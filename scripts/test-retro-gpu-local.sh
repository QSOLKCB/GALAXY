#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
set -eu

SCRIPT_DIR=$(dirname "$0")
ROOT=$(CDPATH= cd -P "$SCRIPT_DIR/.." && pwd)
cd "$ROOT"

ADAPTER=${GALAXY_GPU_ADAPTER:-NVIDIA}
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
OUTPUT=${GALAXY_RETRO_OUTPUT:-runs/retro-gpu-$STAMP}

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf '%s\n' "GALAXY retro GPU test: missing required command '$1'." >&2
    exit 1
  fi
}

need node
need nvidia-smi
# The validation wrapper itself is POSIX sh. Existing production build/runtime
# helpers remain Bash programs, so their interpreter is an explicit dependency.
need bash

GALAXY_CARGO=${GALAXY_CARGO:-cargo}
if ! command -v "$GALAXY_CARGO" >/dev/null 2>&1; then
  GALAXY_CARGO=${CARGO_HOME:-$HOME/.cargo}/bin/cargo
fi
if [ ! -x "$GALAXY_CARGO" ] && ! command -v "$GALAXY_CARGO" >/dev/null 2>&1; then
  printf '%s\n' "GALAXY retro GPU test: Rust/Cargo is required." >&2
  exit 1
fi

printf '%s\n' "== NVIDIA hardware =="
nvidia-smi --query-gpu=name,driver_version,memory.total --format=csv,noheader

printf '\n%s\n' "== Shared retro deterministic vectors =="
node tests/retro-math.mjs
"$GALAXY_CARGO" test --manifest-path retro/Cargo.toml --locked --offline
"$GALAXY_CARGO" run --manifest-path retro/Cargo.toml --example retro_vectors --locked --offline

printf '\n%s\n' "== Production Wasm source remains reproducible =="
bash scripts/build-wasm.sh
git diff --exit-code -- wasm/ data/uff-demo.js rust/src/uff_data.rs

printf '\n%s\n' "== Native GALAXY devices =="
bash scripts/run-gpu.sh devices

printf '\n%s\n' "== Native hardware equation verification =="
bash scripts/run-gpu.sh verify --adapter "$ADAPTER"

printf '\n%s\n' "== >u32 Vulkan address verification =="
bash scripts/run-u64.sh --backend vulkan verify --adapter "$ADAPTER"

if [ "${GALAXY_RETRO_TEST_CUDA:-0}" = 1 ]; then
  printf '\n%s\n' "== >u32 CUDA address verification =="
  bash scripts/run-u64.sh --backend cuda verify
fi

printf '\n%s\n' "== Local spin workload on hardware adapter '$ADAPTER' =="
if [ -e "$OUTPUT" ]; then
  printf '%s\n' "GALAXY retro GPU test: output path already exists: $OUTPUT" >&2
  exit 1
fi
bash scripts/run-gpu.sh run \
  --adapter "$ADAPTER" \
  --job runtime/jobs/spin-local.json \
  --output "$OUTPUT"

printf '\n%s\n' "GALAXY retro GPU validation completed."
printf 'Artifacts: %s\n' "$OUTPUT"
printf '%s\n' "Keep the receipt and terminal log with any benchmark claim."
