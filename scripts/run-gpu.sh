#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
GALAXY_ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$GALAXY_ROOT"
GALAXY_CARGO=${GALAXY_CARGO:-cargo}
if ! command -v "$GALAXY_CARGO" >/dev/null 2>&1; then
  GALAXY_CARGO="${CARGO_HOME:-$HOME/.cargo}/bin/cargo"
fi
if [[ ! -x "$GALAXY_CARGO" ]] && ! command -v "$GALAXY_CARGO" >/dev/null 2>&1; then
  echo "Install Rust 1.85.1 with rustup first; see docs/GPU-RUNTIME.md." >&2
  exit 1
fi
exec "$GALAXY_CARGO" run --manifest-path runtime/Cargo.toml --release --locked -- "$@"
