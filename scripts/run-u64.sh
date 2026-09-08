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
  export GALAXY_BACKEND_REQUESTED="$BACKEND"
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

vulkaninfo_has_hardware_adapter() {
  command -v vulkaninfo >/dev/null 2>&1 || return 1
  vulkaninfo --summary 2>/dev/null |
    grep -Eiq 'deviceName.*(NVIDIA|AMD|Intel|Apple)|GPU[0-9].*(NVIDIA|AMD|Intel|Apple)'
}

explicit_icd_is_probeable_hardware() {
  local explicit="${VK_DRIVER_FILES:-${VK_ICD_FILENAMES:-}}"
  [[ -n "$explicit" ]] || return 1

  # An explicit loader override is authoritative. Do not infer hardware merely
  # from the variable being non-empty: stale paths and software ICDs are common
  # in cloud images. Require every referenced JSON to exist and reject known
  # software drivers, then require vulkaninfo to prove a real adapter.
  local path
  local old_ifs="$IFS"
  IFS=':'
  for path in $explicit; do
    [[ -n "$path" && -f "$path" && -r "$path" ]] || { IFS="$old_ifs"; return 1; }
    if [[ "${path,,}" =~ (lvp|lavapipe|swiftshader|software) ]] ||
       grep -Eiq 'lvp|lavapipe|swiftshader|software' "$path"; then
      IFS="$old_ifs"
      return 1
    fi
  done
  IFS="$old_ifs"
  vulkaninfo_has_hardware_adapter
}

has_hardware_vulkan_hint() {
  if [[ -n "${VK_DRIVER_FILES:-${VK_ICD_FILENAMES:-}}" ]]; then
    explicit_icd_is_probeable_hardware
    return
  fi

  # System ICD JSON files are only loader configuration, not evidence that the
  # corresponding physical device is mounted into this process. This matters in
  # cloud containers that retain NVIDIA/Intel JSON while exposing CUDA only.
  # Auto may select Vulkan only after vulkaninfo successfully enumerates a real
  # hardware adapter; otherwise it must remain able to fall through to CUDA.
  vulkaninfo_has_hardware_adapter
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
