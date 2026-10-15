# Mutual-gravity CPU/f64 qualification, revision 2

This revision records the atomic-step and extreme-scale kernel fixes, exact workload/state-hash verification, and 51 automated verifier contract checks. The published first capture in `../mutual-gravity-2026-10-15/` is preserved unchanged; its source hashes identify the earlier implementation.

This is a source-hash-bound **numerical capability result**, not a production or speedup claim. Historical GALAXY evidence is unchanged. The new solver uses JavaScript `Number` (binary64), planar positions and the Plummer-softened inverse-square kernel; this capture did not execute a GPU.

## Results

- All 27 standalone force fixtures passed, including unequal masses, clusters, coincidences, translated/rotated states, nondefault kernel parameters, theta-zero direct parity, and bounded-depth cases. Independent finite differences of a cubic scalar-potential expansion agreed with paired accelerations to a maximum absolute difference of **1.21e-10**.
- All 14 benchmark force states passed the inherited browser-reference gates: per-body RMS relative error **< 0.03**, worst relative error **< 0.25**. Observed maxima were **0.008773883 RMS** and **0.117169715 worst**.
- Maximum normalized net internal force across those 14 states was **4.31e-17**; the direct control reached **5.10e-17**. The 12 historical baseline states reproduced Barnes–Hut theta-0.5 residuals of **2.47e-4 to 8.81e-4**.
- The declared net-force tolerance is **5e-13**, not a universal machine-epsilon guarantee. The separate unequal-mass translation test at `(x + 10000, y - 20000)` reached **1.395e-13**, exposing finite-precision centre/offset cancellation while remaining within tolerance.
- Barnes–Hut theta was calibrated independently for each force state. All **14/14** achieved RMS errors matched within the explicit ratio band `[0.8, 1.25]`; only **7/14** also matched the 99th-percentile error within that band. This is an RMS-matched comparison, not full tail-distribution equivalence.
- Six N=128 trajectories used shared-step kick–drift–kick over time 0.064, with dt=0.001 and dt/2=0.0005 plus direct-force controls. Maximum mutual momentum drift was **1.06e-16** and ballistic centre-of-mass drift **4.35e-16**. Exact all-pairs Plummer energy, angular momentum and torque were evaluated at every step; extrema and sampled checkpoints are retained.

Mutual theta **0.3** was selected during a preceding pilot. These fixtures are qualification cases, **not held-out accuracy estimates**. Theta is not a universal error guarantee. No mean acceleration was subtracted. Linear-momentum balance does not guarantee angular-momentum or energy conservation: measured mutual torque was nonzero, and reducing dt usually left an approximation-related energy-error floor around 1e-6–1e-5. Direct-control energy drift decreased by approximately fourfold at dt/2.

## Whole-solve timing and memory

Both implementations ran in the **same native Node.js realm**. Each sample includes validation, allocation, fresh tree construction, moments, traversal and propagation. Two warmups preceded nine measured solves; method order alternated. Calibration and diagnostics were outside the timed interval. No forced garbage collection was used.

Selected median observations from this Node.js v24.19.0 host, in milliseconds:

| State, seed 303 | Mutual theta 0.3 | RMS-matched Barnes–Hut | Direct |
|---|---:|---:|---:|
| Disc, N=8 | 0.0171 | 0.0051 | 0.0020 |
| Disc, N=32 | 0.1829 | 0.0436 | 0.0073 |
| Disc, N=128 | 1.7159 | 0.5187 | 0.0720 |
| Disc, N=512 | 13.7175 | 5.6460 | 1.1152 |
| Disc, N=2048 | 71.8830 | 67.9403 | 15.7480 |
| Collision, N=2048 | 70.8800 | 53.0314 | 17.6995 |

The new lane was **slower in these observations**. Small-N overhead remains visible; no crossover, asymptotic scaling or portable speed benefit is established. Very small-N cases can be direct-equivalent, so their RMS matching is not a meaningful approximate-force comparison.

The receipt retains every timing sample, tree node/leaf counts, local-coefficient payload bytes, and before/after heap/RSS/ArrayBuffer observations. Process-memory deltas are noisy, can be negative due to garbage collection, and are neither isolated retained allocations nor peak memory. Coefficient payload bytes exclude objects and other tree/temporary allocations.

## Verify or reproduce

Run from the repository root:

```sh
node tests/mutual-gravity.mjs
node tests/mutual-gravity-evidence.mjs
node scripts/bench-mutual-gravity.mjs --verify evidence/mutual-gravity-2026-10-15-r2
```

The verifier checks source hashes, artifact hashes, schemas, finite JSON numbers, configuration, exact `(id,preset,count,seed)` workload sets with unique rows, regenerated initial-state hashes for all force/small-N/trajectory cases, nonnegative finite qualification metrics, force budgets and trajectory momentum/COM gates. The 51 synthetic-fixture tests include manifest-rehashed duplicate/substituted identities, invalid state hashes, malformed metrics, corrupted artifacts and refusal to overwrite an existing capture. It does not rerun noisy wall-time comparisons as CI pass/fail gates.

To capture a fresh result, use a directory that does not already exist:

```sh
node scripts/bench-mutual-gravity.mjs --output runs/mutual-reproduction-new
node scripts/bench-mutual-gravity.mjs --verify runs/mutual-reproduction-new
```

The runner refuses to overwrite an existing output directory. Its source identity is bound to the six source files listed in `manifest.json`; the upstream Barnes–Hut source pin is `e051324b1263f20ebb88d0e585992c7c1838504a`. Timings and memory observations naturally vary across runs and machines. The manifest hashes `receipt.json` and `tests.log`.
