# Mutual-gravity CPU/f64 qualification, revision 6

This revision independently recomputes every force/small-N scientific record, all 41 Barnes–Hut calibration candidates per state, and all six full KDK trajectory records from deterministic fixtures. It validates timing medians/p90 against their raw samples. The runner snapshots all six executable project sources before evaluating its implementation, then replays the harness/helper from those exact UTF-8 bytes as native-realm ESM modules. Child tests also execute retained source bytes; the browser-realm test uses the captured solver source. Both solvers retain private export namespaces and bypass CommonJS/ESM file caches. Source hashes must still match before and after execution. The Node runtime and small file-entry bootstrap are the trust root.

There are **166 automated verifier checks**, including internally consistent but false scientific records, source/cache substitutions, and incorrect success-marker counts. The exact expected completion count is source-bound and asserted by the suite. Published r1–r5 captures remain unchanged. The solver conservatively rejects unsupported/subnormal intermediate scales and ill-conditioned moment contractions rather than promising correct results for every finite JavaScript input.

This is a source-hash-bound **numerical capability result**, not a production or speedup claim. Historical GALAXY evidence is unchanged. The new solver uses JavaScript `Number` (binary64), planar positions and the Plummer-softened inverse-square kernel; this capture did not execute a GPU.

## Results

- All 27 standalone force fixtures passed, including unequal masses, clusters, coincidences, translated/rotated states, nondefault kernel parameters, theta-zero direct parity, and bounded-depth cases. Independent finite differences of a cubic scalar-potential expansion agreed with paired accelerations to a maximum absolute difference of **1.21e-10**.
- All 14 benchmark force states passed the inherited browser-reference gates: per-body RMS relative error **< 0.03**, worst relative error **< 0.25**. Observed maxima were **0.008773883 RMS** and **0.117169715 worst**.
- Maximum normalized net internal force across those 14 states was **3.44e-17**; the direct control reached **5.10e-17**. The 12 historical baseline states reproduced Barnes–Hut theta-0.5 residuals of **2.47e-4 to 8.81e-4**.
- The declared net-force tolerance is **5e-13**, not a universal machine-epsilon guarantee. The separate unequal-mass translation test at `(x + 10000, y - 20000)` reached **1.395e-13**, exposing finite-precision centre/offset cancellation while remaining within tolerance.
- Barnes–Hut theta was calibrated independently for each force state. All **14/14** achieved RMS errors matched within the explicit ratio band `[0.8, 1.25]`; only **7/14** also matched the 99th-percentile error within that band. This is an RMS-matched comparison, not full tail-distribution equivalence.
- Six N=128 trajectories used shared-step kick–drift–kick over time 0.064, with dt=0.001 and dt/2=0.0005 plus direct-force controls. Maximum mutual momentum drift was **1.06e-16** and ballistic centre-of-mass drift **4.35e-16**. Exact all-pairs Plummer energy, angular momentum and torque were evaluated at every step; extrema and sampled checkpoints are retained.

Mutual theta **0.3** was selected during a preceding pilot. These fixtures are qualification cases, **not held-out accuracy estimates**. Theta is not a universal error guarantee. No mean acceleration was subtracted. Linear-momentum balance does not guarantee angular-momentum or energy conservation: measured mutual torque was nonzero, and reducing dt usually left an approximation-related energy-error floor around 1e-6–1e-5. Direct-control energy drift decreased by approximately fourfold at dt/2.

## Whole-solve timing and memory

Both implementations ran in the **same native Node.js realm**, compiled from retained, pre-hashed source byte buffers with private export namespaces. No CommonJS cache or separate vm context supplied either solver. Each sample includes validation, allocation, fresh tree construction, moments, traversal and propagation. Two warmups preceded nine measured solves; method order alternated. Calibration and diagnostics were outside the timed interval. No forced garbage collection was used.

Selected median observations from this Node.js v24.19.0 host, in milliseconds:

| State, seed 303 | Mutual theta 0.3 | RMS-matched Barnes–Hut | Direct |
|---|---:|---:|---:|
| Disc, N=8 | 0.0121 | 0.0027 | 0.0012 |
| Disc, N=32 | 0.1574 | 0.0242 | 0.0044 |
| Disc, N=128 | 2.1675 | 0.2944 | 0.0453 |
| Disc, N=512 | 13.2984 | 3.5275 | 0.6599 |
| Disc, N=2048 | 73.4803 | 38.6102 | 10.2881 |
| Collision, N=2048 | 67.4240 | 28.3204 | 10.1842 |

The new lane was **slower in these observations**. Small-N overhead remains visible; no crossover, asymptotic scaling or portable speed benefit is established. Very small-N cases can be direct-equivalent, so their RMS matching is not a meaningful approximate-force comparison.

The receipt retains every timing sample, tree node/leaf counts, local-coefficient payload bytes, and before/after heap/RSS/ArrayBuffer observations. Process-memory deltas are noisy, can be negative due to garbage collection, and are neither isolated retained allocations nor peak memory. Coefficient payload bytes exclude objects and other tree/temporary allocations.

## Verify or reproduce

Run from the repository root:

```sh
node tests/mutual-gravity.mjs
node tests/mutual-gravity-evidence.mjs
node scripts/bench-mutual-gravity.mjs --verify evidence/mutual-gravity-2026-10-15-r6
```

The verifier checks source/artifact hashes, schemas, finite JSON numbers, exact configuration and workload tuple sets, uniqueness and regenerated state hashes. It rejects duplicate decoded JSON object keys (including escaped-equivalent keys), and hashes/parses a single receipt buffer. It recomputes all force/error/balance/stat/tree records, the full calibration grid and selected result, full trajectory diagnostics/checkpoints/timestep controls, every summary field, and timing medians/p90. Qualification still applies the original force budgets and momentum/COM gates. Private immutable caches avoid repeating deterministic solves within one verification process; returned copies cannot modify the cache.

The 166 verifier checks include manifest-rehashed scientific forgeries, wrong success counts with matching output hashes, incomplete/failed commands, mutated source bytes, stale caches, changing-read attacks, timing-summary tampering and existing-output refusal. Timings and memory samples themselves remain observations, with no speed gate. Verification recomputes deterministic scientific quantities but does not rerun the noisy timing experiment.

Scientific records are compared exactly. This capture used **Node.js v24.19.0**. Different Node/V8 versions or architectures may produce different floating-point rounding and fail exact verification even when numerical budgets pass. Node 22 CI is a separate portability check; this local receipt does not establish its outcome. Reproduce under the recorded runtime when investigating such a mismatch rather than widening tolerances silently.

To capture a fresh result, use a directory that does not already exist:

```sh
node scripts/bench-mutual-gravity.mjs --output runs/mutual-reproduction-new
node scripts/bench-mutual-gravity.mjs --verify runs/mutual-reproduction-new
```

The runner refuses to overwrite an existing output directory. Its source identity is bound to the six source files listed in `manifest.json`; the upstream Barnes–Hut source pin is `e051324b1263f20ebb88d0e585992c7c1838504a`. Timings and memory observations naturally vary across runs and machines. The manifest hashes `receipt.json` and `tests.log`. In this revision, `tests.log` is structured JSON containing both commands, exit codes, signals, errors and exact stdout/stderr strings; the receipt records matching per-command output hashes. Each run also records the retained-source ESM stdin execution mode, and successful completion markers are required, including the exact source-bound verifier count. Artifact hashes and log agreement do not authenticate when or where timings, memory observations or logged test executions occurred. They are unsigned observational evidence, not an independently signed execution attestation. Recomputed deterministic scientific records have the stronger verification described above.

## Capture digests

SHA-256:

- `mutual-gravity.js`: `9ba0f8ae8ea84d60647598f6d92af3fd5983dbb47390bf1059aaa6888a6e8696`
- `receipt.json`: `17d0daca672f882c396e553a5ca49eac727248cdf2feee1a288f40737316c92c`
- `manifest.json`: `57d1dc8df53b30b44e8570d94a1cb68429b8426ba0021522bffc76f8be5eec5d`

Hashes bind this record to source/artifact bytes; they are not a signed attestation or a substitute for rerunning the experiment.
