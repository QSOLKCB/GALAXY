#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${GALAXY_CUDA_SITE:-$ROOT/.galaxy-cuda-python}"
PYTHON="${GALAXY_CUDA_BOOTSTRAP_PYTHON:-python3}"
CUPY_VERSION="${GALAXY_CUPY_VERSION:-14.2.0}"

if ! command -v "$PYTHON" >/dev/null 2>&1; then
  echo "GALAXY CUDA bootstrap requires Python 3." >&2
  exit 1
fi
if ! command -v nvidia-smi >/dev/null 2>&1; then
  echo "nvidia-smi is unavailable; this bootstrap is only for NVIDIA CUDA targets." >&2
  exit 1
fi
if ! nvidia-smi >/dev/null 2>&1; then
  echo "nvidia-smi cannot access an NVIDIA GPU. Fix GPU visibility before installing CUDA dependencies." >&2
  exit 1
fi

CUDA_MAJOR="$(
  nvidia-smi 2>/dev/null |
    sed -n 's/.*CUDA Version: \([0-9][0-9]*\)\..*/\1/p' |
    head -n1
)"
case "$CUDA_MAJOR" in
  13)
    PACKAGE="cupy-cuda13x[ctk]==${CUPY_VERSION}"
    ;;
  12)
    PACKAGE="cupy-cuda12x[ctk]==${CUPY_VERSION}"
    ;;
  *)
    echo "Could not map the driver-supported CUDA major (${CUDA_MAJOR:-unknown}) to a pinned CuPy wheel." >&2
    echo "GALAXY currently supports CUDA 12.x and 13.x through CuPy ${CUPY_VERSION}." >&2
    exit 1
    ;;
esac

mkdir -p "$TARGET"
echo "Installing $PACKAGE into $TARGET"
"$PYTHON" -m pip install --upgrade --target "$TARGET" "$PACKAGE"

PYTHONPATH="$TARGET${PYTHONPATH:+:$PYTHONPATH}" "$PYTHON" - <<'PY'
import cupy as cp
print("CuPy:", cp.__version__)
print("CUDA driver:", cp.cuda.runtime.driverGetVersion())
print("CUDA runtime:", cp.cuda.runtime.runtimeGetVersion())
print("CUDA devices:", cp.cuda.runtime.getDeviceCount())
for i in range(cp.cuda.runtime.getDeviceCount()):
    with cp.cuda.Device(i):
        p = cp.cuda.runtime.getDeviceProperties(i)
        name = p["name"]
        if isinstance(name, bytes):
            name = name.decode(errors="replace").rstrip("\0")
        free, total = cp.cuda.runtime.memGetInfo()
        print(f"[{i}] {name} · {total/2**30:.2f} GiB total · {free/2**30:.2f} GiB free")
PY

echo
echo "CUDA backend ready."
echo "Run: bash scripts/run-u64.sh devices --backend cuda"
