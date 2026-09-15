# CPU host-aware promotion and tuning (PE #14)

PE #14 adds a calibrated execution-selection layer above the canonical, spawned worker-local SoA, and persistent worker-pool CPU paths established by PRs #11-#13.

The new commands are:

```sh
galaxy-cpu verify-auto [--workers N]

galaxy-cpu bench-auto \
  [--logical U64] [--resident N] [--frames N] [--workers N] \
  [--repeats N] [--seed U32] [--receipt PATH]
```

`bench-auto` is an explicit promotion surface. The existing manual controls remain unchanged:

- `bench` — canonical reference/oracle path;
- `bench-soa` — spawned worker-local SoA;
- `bench-soa-pool` — persistent worker-local SoA with explicit scheduling and tile selection.

Because those manual surfaces already exist, `bench-auto` rejects `--path`, `--tile`, and `--schedule` rather than mixing manual and automatic policy.

## Selection policy

The versioned policy is `calibrated-host-auto-v1`.

It does not use CPU model-name tables. Instead it calibrates a bounded deterministic slice of the requested workload on the current host.

The calibration slice is capped at:

- 65,536 resident particles;
- 4 frames;
- 3 repeats.

Candidates are:

1. canonical BAM-LUT reference execution;
2. spawned worker-local SoA at tile sizes 1,024, 4,096, 16,384, and 65,536, bounded by the calibration resident count;
3. persistent worker-local SoA with `physical-first` scheduling at the same tile set;
4. persistent worker-local SoA with explicit logical/SMT scheduling when that resolves to a distinct worker count.

Persistent candidates use a split scoring boundary:

- steady-state median time comes from the bounded calibration slice;
- pool startup comes from a separate **full-requested-pool startup probe** using the requested resident count, worker policy, and candidate tile size;
- the observed full-pool startup is amortized across the requested full-workload repeat count.

The full-pool probe constructs the actual persistent worker set and waits until every worker has allocated its requested worker-local tile capacity and reported ready. LUT construction remains outside that startup timer, matching the established persistent runtime timing boundary.

This prevents a large full workload from being promoted on the strength of an artificially cheap 65,536-particle calibration-pool startup. The receipt retains both the bounded calibration-pool startup and the scored full-requested-pool startup for audit. Spawned and canonical candidates include their normal per-execution costs in their measured medians.

## Promotion margin

The optimized candidate must beat canonical calibration by at least 5% before promotion.

If the measured advantage is smaller, auto selects canonical. This prevents noisy near-ties from changing execution architecture.

The margin is recorded as:

```text
promotion_margin_basis_points = 500
```

## Fail-closed parity

PE #14 adds an independent streaming canonical BAM-LUT oracle. It uses the established particle-generation, LUT interpolation, and scalar contribution contract but does not allocate the full resident AoS array and does not use the SIMD batch hash.

The oracle is identified as:

```text
streaming-canonical-bam-lut-v1
```

Every calibration candidate must match the calibration oracle exactly. A single mismatch aborts selection.

After selection, the same oracle is run over the full requested workload. The selected full-workload execution must match exactly or `bench-auto` exits with an error.

There is no silent checksum fallback.

## Receipt

Auto receipts use:

```text
schema = galaxy.cpu-runtime-auto-receipt.v1
runtime = galaxy-cpu
execution_mode = host-auto
selection_policy = calibrated-host-auto-v1
canonical_oracle = streaming-canonical-bam-lut-v1
parity_fail_closed = true
persistent_startup_score_scope = full-requested-pool
```

The receipt records:

- detected logical and physical topology;
- bounded calibration shape;
- every calibration candidate, timing score, worker count, tile and checksum;
- bounded calibration-pool startup for persistent candidates;
- full-requested-pool startup used for persistent scoring;
- promotion margin;
- selected engine, scheduling mode, tile and worker count;
- tuning time;
- actual selected full-run pool startup when applicable;
- full oracle time and checksum;
- selected checksum and exact parity marker;
- final best/median timing and RSS evidence.

## Claim boundary

The selected configuration is host- and workload-specific evidence derived from a bounded calibration slice plus an observed full-requested-pool startup probe. It is not a universal CPU ranking and does not establish that one tile or scheduling mode is globally optimal.

PE #14 deliberately avoids model-name heuristics and preserves all manual execution commands so the automatic policy can be audited against explicit alternatives.

## PASS boundary

PE #14 passes only if:

1. the streaming oracle matches the established canonical reference in tests;
2. every calibration candidate matches the oracle exactly;
3. persistent promotion accounts for observed startup of the full requested pool shape;
4. the selected full workload matches the full oracle exactly;
5. near-ties remain canonical because of the 5% promotion margin;
6. material measured wins may promote to spawned or persistent SoA;
7. topology evidence remains explicit and fail-soft;
8. existing canonical and manual optimized commands remain unchanged;
9. native CI passes on Linux x86-64, Linux ARM64, macOS ARM64, and Windows x86-64.
