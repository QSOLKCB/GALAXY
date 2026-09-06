# SPDX-License-Identifier: Apache-2.0
"""Generate test vectors by executing the pinned UFF implementation itself.

Usage: python scripts/generate-uff-reference.py /path/to/UFF
Requires UFF's Python dependencies, only for refreshing this development fixture.
"""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
if len(sys.argv) != 2:
    raise SystemExit(__doc__)
source = Path(sys.argv[1]).resolve()
provenance = json.loads((ROOT / "data/uff/provenance.json").read_text())
head = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
if head != provenance["commit"]:
    raise SystemExit("Use the UFF commit recorded in data/uff/provenance.json")
for name, expected in provenance["files"].items():
    if hashlib.sha256((source / name).read_bytes()).hexdigest() != expected:
        raise SystemExit(f"Source content mismatch: {name}")
sys.path.insert(0, str(source))
import numpy as np
from uff.data import load_galaxy_csv
from uff.models import ModelOptions, build_model

data = load_galaxy_csv(source / "DEMO_GALAXY.csv")
radii = [0.18, 0.3, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 5.0, 8.0, 12.0, 13.2, 30.0]
defaults = dict(diskML=0.5, bulgeML=0.7, blackHoleMillion=0.0, uffVInf=120.0,
                uffCore=3.0, uffBeta=0.0, haloLogMass=11.5, haloConcentration=10.0,
                burkertLogDensity=7.5, burkertCore=5.0, mondA0=1.2)
settings_cases = [defaults, {**defaults, "diskML": 0.8, "bulgeML": 1.1,
    "blackHoleMillion": 4.3, "uffVInf": 210, "uffCore": 0.7, "uffBeta": 0.4,
    "haloLogMass": 11.8, "haloConcentration": 17, "burkertLogDensity": 7.8,
    "burkertCore": 1.2, "mondA0": 1.5},
    {**defaults, "blackHoleMillion": 1000, "uffBeta": -1, "uffCore": 100,
     "haloLogMass": 14.5, "haloConcentration": 40, "burkertCore": 100, "mondA0": 0.03}]
cases = []
for settings in settings_cases:
    options = ModelOptions(fit_stellar_mass_to_light=False,
        disk_mass_to_light=settings["diskML"], bulge_mass_to_light=settings["bulgeML"],
        smbh_mass_msun=settings["blackHoleMillion"] * 1e6, a0_m_s2=settings["mondA0"] * 1e-10)
    mapping = dict(v_inf_kms=settings["uffVInf"], uff_core_kpc=settings["uffCore"],
        uff_beta=settings["uffBeta"], log10_m200=settings["haloLogMass"],
        c200=settings["haloConcentration"], log10_rho0=settings["burkertLogDensity"],
        core_radius_kpc=settings["burkertCore"])
    predictions = {}
    for name in ["baryons", "nfw", "burkert", "mond-rar", "uff-empirical"]:
        model = build_model(name, data, options)
        params = {key: mapping[key] for key in model.parameter_names}
        predictions[name] = model.predict(np.array(radii), params).tolist()
    cases.append(dict(settings=settings, predictions=predictions))
result = dict(source_commit=head, data_sha256=provenance["files"]["DEMO_GALAXY.csv"],
    generation="Pinned UFF build_model().predict(); fixed M/L, distance=1, no inclination nuisance scaling, no fitting",
    radii_kpc=radii, cases=cases)
(ROOT / "tests/uff-reference.json").write_text(json.dumps(result, indent=2) + "\n")
print(f"Generated {len(cases) * 5 * len(radii)} independent UFF reference predictions.")
