#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

cargo build --manifest-path runtime/Cargo.toml --release --locked --bin galaxy-u64
exec runtime/target/release/galaxy-u64 "$@"
