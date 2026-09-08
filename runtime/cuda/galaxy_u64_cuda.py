#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Strict entrypoint for GALAXY's CUDA u64 runtime.

The implementation lives beside this file so submitted JSON can be parsed with
fail-closed duplicate-key handling before any defaults, validation, or execution.
"""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path
from typing import Any

IMPLEMENTATION_PATH = Path(__file__).with_name("galaxy_u64_cuda_impl.py")
_SPEC = importlib.util.spec_from_file_location("_galaxy_u64_cuda_impl", IMPLEMENTATION_PATH)
if _SPEC is None or _SPEC.loader is None:
    raise RuntimeError(f"Unable to load CUDA runtime implementation: {IMPLEMENTATION_PATH}")
_impl = importlib.util.module_from_spec(_SPEC)
sys.modules[_SPEC.name] = _impl
_SPEC.loader.exec_module(_impl)

# Preserve the implementation's public and test-facing helper surface. Functions
# keep their implementation-module globals, which we patch below where needed.
for _name in dir(_impl):
    if not _name.startswith("__"):
        globals()[_name] = getattr(_impl, _name)


def _reject_duplicate_object_pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"Duplicate JSON key: {key}")
        result[key] = value
    return result


def load_job(path: Path) -> dict[str, Any]:
    if path.stat().st_size > 65_536:
        raise ValueError("Job JSON must be at most 64 KiB")
    with path.open("r", encoding="utf-8") as handle:
        value = json.load(handle, object_pairs_hook=_reject_duplicate_object_pairs)
    return _impl.validate_job(value)


# All implementation paths that call load_job resolve it from the implementation
# module's globals. Replace that binding so CLI validate/run are strict too.
_impl.load_job = load_job

# The strict entrypoint is executable runtime source and must contribute to the
# same provenance digest as the implementation, UFF data, bootstrap and router.
_entrypoint = Path(__file__).resolve()
if _entrypoint not in _impl.RUNTIME_SOURCE_PATHS:
    _impl.RUNTIME_SOURCE_PATHS = (_entrypoint, *_impl.RUNTIME_SOURCE_PATHS)
RUNTIME_SOURCE_PATHS = _impl.RUNTIME_SOURCE_PATHS


if __name__ == "__main__":
    raise SystemExit(_impl.main())
