#!/usr/bin/env python3
"""Regenerate native GPU compact-object fixtures using the pinned UFF checkout."""
# SPDX-License-Identifier: Apache-2.0
import hashlib
import json
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
source = Path(sys.argv[1]).resolve()
provenance = json.loads((root / "data/uff/provenance.json").read_text())
assert subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=source, text=True).strip() == provenance["commit"]
for name in ["uff/compact.py", "uff/constants.py"]:
    assert hashlib.sha256((source / name).read_bytes()).hexdigest() == provenance["files"][name]
sys.path.insert(0, str(source))
from uff.compact import kerr_characteristic_radii, lqg_area_gap_m2

cases = []
for mass in [1.0, 1e4, 4.3e6, 1e10, 1e11]:
    for spin in [-0.998, -0.9, -0.001, -0.1, 0.0, 0.1, 0.001, 0.9, 0.998]:
        radii = kerr_characteristic_radii(mass, spin)
        cases.append({"mass_msun": mass, "spin": spin, "radii": [radii.horizon_rg,
            radii.photon_orbit_rg, radii.isco_rg, radii.gravitational_radius_kpc]})
destination = root / "runtime/tests/compact-reference.json"
destination.parent.mkdir(parents=True, exist_ok=True)
destination.write_text(json.dumps({"source_commit": provenance["commit"],
    "source_sha256": provenance["files"]["uff/compact.py"], "area_gap_m2": lqg_area_gap_m2(), "cases": cases}, indent=2) + "\n")
print(f"Saved {len(cases)} original UFF compact-object reference cases")
