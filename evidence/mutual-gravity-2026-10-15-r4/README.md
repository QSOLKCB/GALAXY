# Mutual-gravity CPU/f64 qualification, revision 4

This revision normalizes options before atomic-step validation and hardens evidence verification: duplicate JSON object keys are rejected after escape decoding, receipt hashing and parsing use one read buffer, and every summary field is regenerated from validated cases. It retains conservative rejection of underflowed nonzero products/components and exact workload/state-hash verification. There are now **87 automated verifier checks**. Published r1, r2 and r3 captures are preserved unchanged; their source hashes identify earlier implementations. Unsupported intermediate numerical scales fail closed, even where a final mathematical result might be representable.

This is a source-hash-bound **numerical capability result**, not a production or speedup claim. Historical GALAXY evidence is unchanged. The new solver uses JavaScript `Number` (binary64), planar positions and the Plummer-softened inverse-square kernel; this capture did not execute a GPU.

## Results

- All 27 standalone force fixtures passed, including unequal masses, clusters, coincidences, translated/rotated states, nondefault kernel parameters, theta-zero direct parity, and bounded-depth cases. Independent finite differences of a cubic scalar-potential expansion agreed with paired accelerations to a maximum absolute difference of **1.21e-10**.
- All 14 benchmark force states passed the inherited browser-reference gates: per-body RMS relative error **< 0.03**, worst relative error **< 0.25**. Observed maxima were **0.008773883 RMS** and **0.117169715 worst**.
- Maximum normalized net internal force across those 14 states was **4.19e-17**; the direct control reached **5.10e-17**. The 12 historical baseline states reproduced Barnes–Hut theta-0.5 residuals of **2.47e-4 to 8.81e-4**.
- The declared net-force tolerance is **5e-13**, not a universal machine-epsilon guarantee. The separate unequal-mass translation test at `(x + 10000, y - 20000)` reached **1.395e-13**, exposing finite-precision centre/offset cancellation while remaining within tolerance.
- Barnes–Hut theta was calibrated independently for each force state. All **14/14** achieved RMS errors matched within the explicit ratio band `[0.8, 1.25]`; only **7/14** also matched the 99th-percentile error within that band. This is an RMS-matched comparison, not full tail-distribution equivalence.
- Six N=128 trajectories used shared-step kick–drift–kick over time 0.064, with dt=0.001 and dt/2=0.0005 plus direct-force controls. Maximum mutual momentum drift was **1.06e-16** and ballistic centre-of-mass drift **4.35e-16**. Exact all-pairs Plummer energy, angular momentum and torque were evaluated at every step; extrema and sampled checkpoints are retained.

Mutual theta **0.3** was selected during a preceding pilot. These fixtures are qualification cases, **not held-out accuracy estimates**. Theta is not a universal error guarantee. No mean acceleration was subtracted. Linear-momentum balance does not guarantee angular-momentum or energy conservation: measured mutual torque was nonzero, and reducing dt usually left an approximation-related energy-error floor around 1e-6–1e-5. Direct-control energy drift decreased by approximately fourfold at dt/2.

## Whole-solve timing and memory

Both implementations ran in the **same native Node.js realm**. Each sample includes validation, allocation, fresh tree construction, moments, traversal and propagation. Two warmups preceded nine measured solves; method order alternated. Calibration and diagnostics were outside the timed interval. No forced garbage collection was used.

Selected median observations from this Node.js v24.19.0 host, in milliseconds:

| State, seed 303 | Mutual theta 0.3 | RMS-matched Barnes–Hut | Direct |
|---|---:|---:|---:|
| Disc, N=8 | 0.0122 | 0.0027 | 0.0011 |
| Disc, N=32 | 0.1368 | 0.0243 | 0.0048 |
| Disc, N=128 | 1.1650 | 0.2809 | 0.0439 |
| Disc, N=512 | 9.1596 | 3.6022 | 0.6663 |
| Disc, N=2048 | 51.1659 | 39.2720 | 10.2993 |
| Collision, N=2048 | 57.3104 | 29.0985 | 10.4210 |

The new lane was **slower in these observations**. Small-N overhead remains visible; no crossover, asymptotic scaling or portable speed benefit is established. Very small-N cases can be direct-equivalent, so their RMS matching is not a meaningful approximate-force comparison.

The receipt retains every timing sample, tree node/leaf counts, local-coefficient payload bytes, and before/after heap/RSS/ArrayBuffer observations. Process-memory deltas are noisy, can be negative due to garbage collection, and are neither isolated retained allocations nor peak memory. Coefficient payload bytes exclude objects and other tree/temporary allocations.

## Verify or reproduce

Run from the repository root:

```sh
node tests/mutual-gravity.mjs
node tests/mutual-gravity-evidence.mjs
node scripts/bench-mutual-gravity.mjs --verify evidence/mutual-gravity-2026-10-15-r4
```

The verifier checks source hashes, artifact hashes, schemas, finite JSON numbers, configuration, exact `(id,preset,count,seed)` workload sets with unique rows, regenerated initial-state hashes for all force/small-N/trajectory cases, nonnegative finite qualification metrics, force budgets and trajectory momentum/COM gates. The 87 synthetic-fixture tests include manifest-rehashed duplicate/substituted identities, invalid state hashes, malformed metrics, corrupted artifacts, plain/escaped/nested duplicate JSON keys in both documents, instrumented changing-read attacks, exact recomputation of every summary field, and refusal to overwrite an existing capture. It does not rerun noisy wall-time comparisons as CI pass/fail gates.

To capture a fresh result, use a directory that does not already exist:

```sh
node scripts/bench-mutual-gravity.mjs --output runs/mutual-reproduction-new
node scripts/bench-mutual-gravity.mjs --verify runs/mutual-reproduction-new
```

The runner refuses to overwrite an existing output directory. Its source identity is bound to the six source files listed in `manifest.json`; the upstream Barnes–Hut source pin is `e051324b1263f20ebb88d0e585992c7c1838504a`. Timings and memory observations naturally vary across runs and machines. The manifest hashes `receipt.json` and `tests.log`.

## Capture digests

SHA-256:

- `mutual-gravity.js`: `05da64e49fc1831f632892f084167e93d501d93721bc86b82fe9dacb1737caff`
- `receipt.json`: `d0da39bf028f78054a35fc1f99603e7af4a22dc8df6202f142d25e8329e97077`
- `manifest.json`: `274784d00d246234a32c1e536cf3c8ff1f0112fb02e352fec04978a3035a9856`

Hashes bind this record to source/artifact bytes; they are not a signed attestation or a substitute for rerunning the experiment.
