#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BACKEND="${GALAXY_BACKEND:-auto}"
ARGS=()
while (($#)); do
  case "$1" in
    --backend)
      if (($# < 2)); then
        echo "--backend requires auto, vulkan, or cuda" >&2
        exit 2
      fi
      BACKEND="$2"
      shift 2
      ;;
    --backend=*)
      BACKEND="${1#--backend=}"
      shift
      ;;
    *)
      ARGS+=("$1")
      shift
      ;;
  esac
done

case "$BACKEND" in
  auto|vulkan|cuda) ;;
  *)
    echo "Unknown backend '$BACKEND'; expected auto, vulkan, or cuda." >&2
    exit 2
    ;;
esac

GALAXY_CARGO="${GALAXY_CARGO:-cargo}"
if ! command -v "$GALAXY_CARGO" >/dev/null 2>&1; then
  GALAXY_CARGO="${CARGO_HOME:-$HOME/.cargo}/bin/cargo"
fi

cuda_python() {
  local python="${GALAXY_CUDA_PYTHON:-python3}"
  if ! command -v "$python" >/dev/null 2>&1; then
    echo "Python 3 is required for the CUDA backend." >&2
    return 1
  fi
  local site="${GALAXY_CUDA_SITE:-$ROOT/.galaxy-cuda-python}"
  if [[ -d "$site" ]]; then
    PYTHONPATH="$site${PYTHONPATH:+:$PYTHONPATH}" \
      exec "$python" runtime/cuda/galaxy_u64_cuda.py "${ARGS[@]}"
  fi
  exec "$python" runtime/cuda/galaxy_u64_cuda.py "${ARGS[@]}"
}

vulkan_runtime() {
  if [[ ! -x "$GALAXY_CARGO" ]] && ! command -v "$GALAXY_CARGO" >/dev/null 2>&1; then
    echo "Rust/Cargo is unavailable for the Vulkan backend." >&2
    return 1
  fi
  exec "$GALAXY_CARGO" run \
    --manifest-path runtime/Cargo.toml \
    --release \
    --locked \
    --bin galaxy-u64 \
    -- "${ARGS[@]}"
}

has_hardware_vulkan_hint() {
  local dir="${VK_ICD_FILENAMES:-${VK_DRIVER_FILES:-}}"
  if [[ -n "$dir" ]]; then
    return 0
  fi
  if command -v vulkaninfo >/dev/null 2>&1; then
    if vulkaninfo --summary 2>/dev/null |
      grep -Eiq 'deviceName.*(NVIDIA|AMD|Intel|Apple)|GPU[0-9].*(NVIDIA|AMD|Intel|Apple)'; then
      return 0
    fi
  fi
  if [[ -d /usr/share/vulkan/icd.d ]]; then
    if find /usr/share/vulkan/icd.d -maxdepth 1 -type f -name '*.json' \
      ! -iname '*lvp*' ! -iname '*lavapipe*' ! -iname '*swiftshader*' \
      -print -quit 2>/dev/null | grep -q .; then
      return 0
    fi
  fi
  return 1
}

case "$BACKEND" in
  vulkan)
    vulkan_runtime
    ;;
  cuda)
    cuda_python
    ;;
  auto)
    if { [[ -x "$GALAXY_CARGO" ]] || command -v "$GALAXY_CARGO" >/dev/null 2>&1; } &&
       has_hardware_vulkan_hint; then
      vulkan_runtime
    fi
    echo "GALAXY: no usable hardware Vulkan+Cargo path detected; selecting CUDA." >&2
    echo "GALAXY: use --backend vulkan or --backend cuda to force a backend." >&2
    cuda_python
    ;;
esac
