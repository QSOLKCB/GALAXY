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

resolve_repo_path() {
  local value="$1"
  case "$value" in
    /*) printf '%s\n' "$value" ;;
    *) printf '%s/%s\n' "$ROOT" "$value" ;;
  esac
}

argument_value() {
  local wanted="$1"
  local i
  for ((i=0; i<${#ARGS[@]}; i++)); do
    case "${ARGS[$i]}" in
      "$wanted")
        if ((i + 1 < ${#ARGS[@]})); then
          printf '%s\n' "${ARGS[$((i+1))]}"
          return 0
        fi
        return 1
        ;;
      "$wanted="*)
        printf '%s\n' "${ARGS[$i]#*=}"
        return 0
        ;;
    esac
  done
  return 1
}

cuda_python() {
  local python="${GALAXY_CUDA_PYTHON:-python3}"
  if ! command -v "$python" >/dev/null 2>&1; then
    echo "Python 3 is required for the CUDA backend." >&2
    return 1
  fi
  local site
  site="$(resolve_repo_path "${GALAXY_CUDA_SITE:-.galaxy-cuda-python}")"
  export GALAXY_BACKEND_REQUESTED="$BACKEND"
  if [[ -d "$site" ]]; then
    PYTHONPATH="$site${PYTHONPATH:+:$PYTHONPATH}" \
      exec "$python" runtime/cuda/galaxy_u64_cuda.py "${ARGS[@]}"
  fi
  exec "$python" runtime/cuda/galaxy_u64_cuda.py "${ARGS[@]}"
}

rewrite_vulkan_receipt() {
  local receipt="$1"
  local temp="${receipt}.backend.$$"
  local add_selected=1
  if grep -Fq '"backend_selected"' "$receipt"; then
    add_selected=0
  fi
  if ! awk -v requested="$BACKEND" -v add_selected="$add_selected" '
    /"backend_selected"[[:space:]]*:/ {
      sub(/"backend_selected"[[:space:]]*:[[:space:]]*"[^"]*"/, "\"backend_selected\": \"vulkan\"")
      print
      next
    }
    /"backend_requested"[[:space:]]*:/ {
      indent=$0
      sub(/[^ ].*/, "", indent)
      if (add_selected == 1) {
        print indent "\"backend_selected\": \"vulkan\"," 
      }
      sub(/"backend_requested"[[:space:]]*:[[:space:]]*"[^"]*"/, "\"backend_requested\": \"" requested "\"")
      found=1
      print
      next
    }
    { print }
    END { if (!found) exit 42 }
  ' "$receipt" >"$temp"; then
    rm -f "$temp"
    echo "GALAXY: Vulkan receipt is missing backend_requested provenance." >&2
    return 1
  fi
  mv "$temp" "$receipt"
}

vulkan_runtime() {
  if [[ ! -x "$GALAXY_CARGO" ]] && ! command -v "$GALAXY_CARGO" >/dev/null 2>&1; then
    echo "Rust/Cargo is unavailable for the Vulkan backend." >&2
    return 1
  fi
  export GALAXY_BACKEND_REQUESTED="$BACKEND"

  local rc
  if "$GALAXY_CARGO" run \
    --manifest-path runtime/Cargo.toml \
    --release \
    --locked \
    --bin galaxy-u64 \
    -- "${ARGS[@]}"; then
    rc=0
  else
    rc=$?
  fi

  if [[ "${ARGS[0]:-}" == "run" ]]; then
    local output=""
    output="$(argument_value --output || true)"
    if [[ -n "$output" && -f "$output/receipt.json" ]]; then
      rewrite_vulkan_receipt "$output/receipt.json" || return 1
    fi
  fi
  return "$rc"
}

vulkaninfo_has_hardware_adapter() {
  command -v vulkaninfo >/dev/null 2>&1 || return 1
  vulkaninfo --summary 2>/dev/null |
    grep -Eiq 'deviceName.*(NVIDIA|AMD|Intel|Apple)|GPU[0-9].*(NVIDIA|AMD|Intel|Apple)'
}

rust_vulkan_has_hardware_adapter() {
  if [[ ! -x "$GALAXY_CARGO" ]] && ! command -v "$GALAXY_CARGO" >/dev/null 2>&1; then
    return 1
  fi
  local output
  if ! output="$("$GALAXY_CARGO" run \
      --quiet \
      --manifest-path runtime/Cargo.toml \
      --release \
      --locked \
      --bin galaxy-u64 \
      -- devices 2>/dev/null)"; then
    return 1
  fi
  printf '%s\n' "$output" | awk '
    /^[[:space:]]*\{/ { inside=1; vulkan=0; hardware=0 }
    inside && /"backend"[[:space:]]*:[[:space:]]*"Vulkan"/ { vulkan=1 }
    inside && /"software"[[:space:]]*:[[:space:]]*false/ { hardware=1 }
    inside && /^[[:space:]]*\}/ {
      if (vulkan && hardware) found=1
      inside=0
    }
    END { exit found ? 0 : 1 }
  '
}

probe_hardware_vulkan() {
  vulkaninfo_has_hardware_adapter || rust_vulkan_has_hardware_adapter
}

explicit_icd_is_probeable_hardware() {
  local explicit="${VK_DRIVER_FILES:-${VK_ICD_FILENAMES:-}}"
  [[ -n "$explicit" ]] || return 1

  # An explicit loader override is authoritative. Stale paths and software ICDs
  # are common in cloud images, so validate the referenced JSONs and then prove
  # that a real adapter is actually enumerable by vulkaninfo or GALAXY itself.
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
  probe_hardware_vulkan
}

has_hardware_vulkan_hint() {
  if [[ -n "${VK_DRIVER_FILES:-${VK_ICD_FILENAMES:-}}" ]]; then
    explicit_icd_is_probeable_hardware
    return
  fi

  # System ICD JSON is loader configuration, not hardware evidence. Prefer the
  # cheap vulkaninfo probe, but do not require that optional utility: when it is
  # missing or fails, ask GALAXY's own Rust/wgpu device enumeration instead.
  probe_hardware_vulkan
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
      exit 0
    fi
    echo "GALAXY: no usable hardware Vulkan+Cargo path detected; selecting CUDA." >&2
    echo "GALAXY: use --backend vulkan or --backend cuda to force a backend." >&2
    cuda_python
    ;;
esac
