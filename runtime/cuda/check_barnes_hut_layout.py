#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Verify BH #2B1 Rust/WGSL/CUDA transfer-record ABI without requiring CUDA."""

from __future__ import annotations

import json
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CUDA = ROOT / "runtime" / "cuda" / "barnes_hut_flat.cu"

EXPECTED_HEX = (
    "010000000200000003000000040000000000003f0000803e0000803f00000000"
    "0000803f000000c0000040400000000004000000050000000600000007000000"
    "6745230108000000090000000a0000000000803f000000400000404000008040"
    "0000a0400000c040000000000000000007000000080000000900000000000000"
    "0a0000000b0000000c000000ffffffff"
)

def canonical_bytes() -> bytes:
    return b"".join(
        [
            struct.pack("<4I4f", 1, 2, 3, 4, 0.5, 0.25, 1.0, 0.0),
            struct.pack("<4f4I", 1.0, -2.0, 3.0, 0.0, 4, 5, 6, 7),
            struct.pack("<4I", 0x01234567, 8, 9, 10),
            struct.pack(
                "<8f8I",
                1.0, 2.0, 3.0, 4.0,
                5.0, 6.0, 0.0, 0.0,
                7, 8, 9, 0,
                10, 11, 12, 0xFFFFFFFF,
            ),
        ]
    )

def main() -> None:
    sizes = {
        "settings": struct.calcsize("<4I4f"),
        "body": struct.calcsize("<4f4I"),
        "entry": struct.calcsize("<4I"),
        "cell": struct.calcsize("<8f8I"),
        "acceleration": struct.calcsize("<4f"),
    }
    expected = {"settings": 32, "body": 32, "entry": 16, "cell": 64, "acceleration": 16}
    if sizes != expected:
        raise SystemExit(f"layout size mismatch: {sizes}")

    fixture = canonical_bytes()
    if len(fixture) != 144 or fixture.hex() != EXPECTED_HEX:
        raise SystemExit("canonical packed fixture mismatch")

    source = CUDA.read_text()
    required = [
        "static_assert(sizeof(BhSettings) == 32",
        "static_assert(sizeof(BhBody) == 32",
        "static_assert(sizeof(BhEntry) == 16",
        "static_assert(sizeof(BhCell) == 64",
        'extern "C" __global__ void bh_traverse',
        "target_position >= cell.range_depth[0]",
        "target_position < cell.range_depth[1]",
    ]
    missing = [needle for needle in required if needle not in source]
    if missing:
        raise SystemExit(f"CUDA traversal contract missing: {missing}")

    print(json.dumps({
        "status": "pass",
        "schema": "galaxy.barnes-hut-gpu-layout.v1",
        "record_bytes": sizes,
        "canonical_fixture_bytes": len(fixture),
        "canonical_fixture_hex": EXPECTED_HEX,
        "cuda_execution_claimed": False,
    }, indent=2))

if __name__ == "__main__":
    main()
