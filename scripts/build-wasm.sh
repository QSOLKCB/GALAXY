#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
cd "$(dirname "$0")/.."
node scripts/package-uff-data.mjs
cargo build --manifest-path rust/Cargo.toml --target wasm32-unknown-unknown --release --locked --offline
node scripts/package-wasm.mjs
