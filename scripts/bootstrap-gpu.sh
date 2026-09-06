#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Opt-in bootstrap for an existing Ubuntu/Debian GPU container; no host drivers.
set -euo pipefail
if [[ $(id -u) != 0 ]]; then
  echo "Bootstrap installs container packages and must run as root. For local setup use docs/GPU-RUNTIME.md." >&2
  exit 1
fi
apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates curl build-essential libvulkan1 vulkan-tools pkg-config
GALAXY_CARGO_DIR="${CARGO_HOME:-$HOME/.cargo}"
if [[ ! -x "$GALAXY_CARGO_DIR/bin/rustup" ]]; then
  GALAXY_INSTALLER=$(mktemp)
  trap 'rm -f "$GALAXY_INSTALLER"' EXIT
  curl --proto '=https' --tlsv1.2 -fsS https://sh.rustup.rs -o "$GALAXY_INSTALLER"
  sh "$GALAXY_INSTALLER" -y --profile minimal --default-toolchain 1.85.1
fi
"$GALAXY_CARGO_DIR/bin/rustup" toolchain install 1.85.1 --profile minimal
echo "Container build tools ready. NVIDIA Vulkan driver libraries must be exposed by the host at instance creation."
