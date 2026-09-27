#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PYTHON_BIN=$(command -v python3 || true)
if [ -z "$PYTHON_BIN" ]; then
  echo "BH #2D hardware sweep: python3 is required" >&2
  exit 127
fi

if [ -x /usr/bin/env ]; then
  ENV_BIN=/usr/bin/env
elif [ -x /bin/env ]; then
  ENV_BIN=/bin/env
else
  echo "BH #2D hardware sweep: system env executable is required" >&2
  exit 127
fi

PATH_VALUE=${PATH-}
HOME_VALUE=${HOME-}
USER_VALUE=${USER-}
LOGNAME_VALUE=${LOGNAME-}
TMPDIR_VALUE=${TMPDIR-}
CARGO_HOME_VALUE=${CARGO_HOME:-$HOME_VALUE/.cargo}
RUSTUP_HOME_VALUE=${RUSTUP_HOME:-$HOME_VALUE/.rustup}

exec "$ENV_BIN" -i \
  GALAXY_BH2D_CLEAN_LAUNCH=1 \
  PATH="$PATH_VALUE" \
  HOME="$HOME_VALUE" \
  USER="$USER_VALUE" \
  LOGNAME="$LOGNAME_VALUE" \
  TMPDIR="$TMPDIR_VALUE" \
  CARGO_HOME="$CARGO_HOME_VALUE" \
  RUSTUP_HOME="$RUSTUP_HOME_VALUE" \
  "$PYTHON_BIN" -I "$SCRIPT_DIR/bench-bh2d-hardware.py" "$@"
