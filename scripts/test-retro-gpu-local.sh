#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

ADAPTER="${GALAXY_GPU_ADAPTER:-NVIDIA}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUTPUT="${GALAXY_RETRO_OUTPUT:-runs/retro-gpu-${STAMP}}"

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "GALAXY retro GPU test: missing required command '$1'." >&2
    exit 1
  fi
}

need node
need nvidia-smi

GALAXY_CARGO="${GALAXY_CARGO:-cargo}"
if ! command -v "$GALAXY_CARGO" >/dev/null 2>&1; then
  GALAXY_CARGO="${CARGO_HOME:-$HOME/.cargo}/bin/cargo"
fi
if [[ ! -x "$GALAXY_CARGO" ]] && ! command -v "$GALAXY_CARGO" >/dev/null 2>&1; then
  echo "GALAXY retro GPU test: Rust/Cargo is required." >&2
  exit 1
fi

echo "== NVIDIA hardware =="
nvidia-smi --query-gpu=name,driver_version,memory.total --format=csv,noheader

echo
echo "== Shared retro deterministic vectors =="
node tests/retro-math.mjs
"$GALAXY_CARGO" test --manifest-path retro/Cargo.toml --locked --offline
"$GALAXY_CARGO" run --manifest-path retro/Cargo.toml --example retro_vectors --locked --offline

echo
echo "== Production Wasm source remains reproducible =="
bash scripts/build-wasm.sh
git diff --exit-code -- wasm/ data/uff-demo.js rust/src/uff_data.rs

echo
echo "== Native GALAXY devices =="
bash scripts/run-gpu.sh devices

echo
echo "== Native hardware equation verification =="
bash scripts/run-gpu.sh verify --adapter "$ADAPTER"

echo
echo "== >u32 Vulkan address verification =="
bash scripts/run-u64.sh --backend vulkan verify --adapter "$ADAPTER"

if [[ "${GALAXY_RETRO_TEST_CUDA:-0}" == "1" ]]; then
  echo
  echo "== >u32 CUDA address verification =="
  bash scripts/run-u64.sh --backend cuda verify
fi

echo
echo "== Local spin workload on hardware adapter '$ADAPTER' =="
if [[ -e "$OUTPUT" ]]; then
  echo "GALAXY retro GPU test: output path already exists: $OUTPUT" >&2
  exit 1
fi
bash scripts/run-gpu.sh run \
  --adapter "$ADAPTER" \
  --job runtime/jobs/spin-local.json \
  --output "$OUTPUT"

echo
echo "GALAXY retro GPU validation completed."
echo "Artifacts: $OUTPUT"
echo "Keep the receipt and terminal log with any benchmark claim."
