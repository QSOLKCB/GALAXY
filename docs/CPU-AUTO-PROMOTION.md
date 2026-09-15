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

It does not use CPU model-name tables. It starts from a deterministic calibration base on the current host, then expands resident calibration work only when required to preserve the requested workload's effective per-worker tile shape.

The baseline calibration policy is:

- base resident target: 65,536 particles;
- **requested frame depth is preserved exactly**;
- 3 repeats;
- resident count may expand above 65,536, never beyond the requested resident count, when worker partitioning would otherwise collapse an advertised tile to a smaller effective tile.

Candidates are:

1. canonical BAM-LUT reference execution;
2. spawned worker-local SoA at tile sizes 1,024, 4,096, 16,384, and 65,536, bounded by the requested resident count;
3. persistent worker-local SoA with `physical-first` scheduling at the same tile set;
4. persistent worker-local SoA with explicit logical/SMT scheduling when that resolves to a distinct worker count.

## Effective tile-shape calibration

A nominal `--tile 65536` is not a 65,536-particle worker tile if the worker owns only 256 particles. Auto therefore treats **effective per-worker tile size** as part of the configuration being calibrated.

For a resident population `R`, worker count `W`, and nominal tile `T`, the conservative effective tile recorded for calibration is:

```text
effective_tile = min(T, floor(R / W))
```

The runtime chooses one common calibration resident shape large enough that every advertised tile has the same effective per-worker size during calibration that it will have on the requested workload. If the requested workload itself is partition-limited for the largest tile, calibration uses the exact requested resident shape instead of pretending a smaller slice has equivalent cache/tile behavior.

The policy identifier is:

```text
tile_shape_policy = expand-resident-for-effective-tile-v1
```

Every tiled candidate records:

```text
calibration_workers
calibration_effective_tile_particles
requested_effective_tile_particles
tile_shape_match
```

A mismatch is a fail-closed tuning error. A candidate is never promoted on timing evidence collected at a different effective tile size from the requested execution.

This may make auto tuning more expensive on very large worker counts. That cost is intentional: choosing among large cache-sensitive tile sizes without actually exercising those tile sizes would be cheaper but scientifically misleading.

## Requested frame-depth calibration

Particle generation and worker-local tile fill happen once per particle before the frame loop, while BAM-LUT projection and contribution hashing repeat for every frame. A fixed shallow calibration such as four frames therefore changes the ratio between one-time per-particle work and repeated per-frame work.

PE #14 now preserves:

```text
calibration_frames = requested_frames
```

with policy identifier:

```text
frame_calibration_policy = preserve-requested-depth-v1
```

This means a deep requested run is ranked at the same frame depth rather than extrapolated from a cost mix that only exists for a shallow run. Resident work may still be bounded/expanded by the tile-shape policy above.

## Full-requested-workload score projection

Promotion compares like with like. A calibration median is **not** compared directly with a full-requested-pool startup cost.

Every candidate's calibration median is projected to the requested particle-frame work using:

```text
calibration_work = calibration_resident × calibration_frames
requested_work   = requested_resident × requested_frames

projected_median = ceil(
  calibration_median × requested_work / calibration_work
)
```

Because frame depth is preserved, the projection normally scales only the remaining resident-population difference while retaining the requested per-particle/per-frame cost mix.

The receipt identifies the projection model as:

```text
score_projection = linear-particle-frame-v1
```

The projection is an explicit host/workload tuning heuristic, not a theorem that runtime is perfectly linear. The selected full workload is still executed and checked against the independent oracle after selection.

Canonical and spawned-SoA candidates use the projected median directly as their score. Their normal per-execution setup is already included in the measured calibration median and is therefore carried by the same projection.

Persistent candidates add one extra term:

- steady-state median is measured on the tile-faithful, frame-faithful calibration shape and projected to requested particle-frame work;
- pool startup is measured separately with the **full requested resident count, worker policy, and candidate tile size**;
- persistent worker-local buffers are allocated and explicitly first-touched before workers report ready;
- that observed full-pool startup is amortized across the requested repeat count.

Thus:

```text
persistent_score = projected_median + full_pool_startup / requested_repeats
```

The full-pool probe constructs the actual persistent worker set and waits until every worker has allocated and first-touched its requested worker-local tile capacity and reported ready. LUT construction remains outside that startup timer, matching the established persistent runtime timing boundary.

The receipt preserves both the calibration-pool startup and the scored full-requested-pool startup for audit and records:

```text
persistent_startup_includes_buffer_first_touch = true
```

## Scheduling evidence and fallback

`physical-first` is a requested policy, not a guarantee that physical-core topology is available.

When physical-core detection succeeds, `physical-first` caps the persistent candidate at that detected physical-core count. When detection is unavailable, the runtime falls back to the logical worker limit.

Auto receipts therefore distinguish:

```text
requested_schedule
effective_schedule
schedule_fell_back_to_logical
```

The compatibility field `schedule` and the top-level `selected_schedule` report the **effective** schedule actually executed. On Windows, where physical-core probing is currently unavailable, a `physical-first` candidate therefore records logical execution plus an explicit fallback marker instead of claiming that a physical-core policy was applied.

## Promotion margin

The optimized candidate must beat canonical by at least 5% in the projected full-requested-workload score before promotion.

If the measured/projected advantage is smaller, auto selects canonical. This prevents noisy near-ties from changing execution architecture.

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

## Tuning latency

`tuning_ns` covers the complete execution-policy decision beginning **before topology detection**. This matters because Linux topology detection reads process/sysfs state and macOS invokes `sysctl`, and that result directly changes which candidates and worker counts are calibrated.

The receipt records topology cost separately as:

```text
topology_detection_ns
```

The calibration-oracle portion is also recorded separately as:

```text
calibration_oracle_ns
```

Both are included inside `tuning_ns`. The later full-workload oracle remains separate as `full_oracle_ns` because it validates the selected execution rather than choosing the candidate.

## RSS evidence scope

Linux RSS evidence comes from `/proc/self/status` `VmHWM`. That value is a **process-wide monotonic high-water mark**: once an earlier calibration candidate or full-pool startup probe raises it, it cannot later be interpreted as the isolated peak of the selected engine.

The auto receipt therefore does not expose the ambiguous generic `peak_rss_kib` field. Instead it records:

```text
rss_metric = linux-vmhwm-process-high-water-mark-when-available
rss_scope = whole-auto-invocation
auto_invocation_peak_rss_kib = <VmHWM or null>
selected_run_peak_rss_available = false
selected_run_peak_rss_kib = null
```

This value may include memory used by topology-driven calibration, candidate execution, full-pool startup probes, the full oracle, and the selected run. It is valid as the high-water mark of the **entire `bench-auto` process invocation**, not as selected-engine memory evidence.

Selected-engine memory comparisons must therefore use an isolated/manual execution surface such as `bench`, `bench-soa`, or `bench-soa-pool`, or a future subprocess/isolation mechanism that can obtain an independent high-water mark.

## Receipt

Auto receipts use:

```text
schema = galaxy.cpu-runtime-auto-receipt.v1
runtime = galaxy-cpu
execution_mode = host-auto
selection_policy = calibrated-host-auto-v1
canonical_oracle = streaming-canonical-bam-lut-v1
parity_fail_closed = true
score_projection = linear-particle-frame-v1
tile_shape_policy = expand-resident-for-effective-tile-v1
frame_calibration_policy = preserve-requested-depth-v1
persistent_startup_score_scope = full-requested-pool
persistent_startup_includes_buffer_first_touch = true
rss_scope = whole-auto-invocation
selected_run_peak_rss_available = false
```

The receipt records:

- detected logical and physical topology;
- topology-detection time;
- 65,536-particle calibration base and the actual tile-faithful calibration resident count;
- requested frame depth and identical calibration frame depth;
- calibration and requested particle-frame work units;
- calibration-oracle time and complete tuning time;
- every calibration candidate's calibration median and projected median;
- requested/effective schedule and fallback state;
- calibration and requested worker counts;
- nominal tile plus calibration/requested effective tile sizes and exact match marker;
- candidate checksum and projected score;
- calibration-pool startup for persistent candidates;
- full-requested-pool startup used for persistent scoring, including explicit buffer first-touch;
- promotion margin;
- selected engine, effective scheduling mode, requested scheduling mode, tile and worker count;
- selected effective tile-shape evidence;
- actual selected full-run pool startup when applicable;
- full oracle time and checksum;
- selected checksum and exact parity marker;
- final best/median timing;
- whole-auto-invocation `VmHWM` when available, explicitly **not** an isolated selected-run peak RSS.

## Claim boundary

The selected configuration is host- and workload-specific evidence derived from a deterministic tile-faithful and frame-faithful calibration shape, an explicit particle-frame score projection, and—where applicable—an observed full-requested-pool startup probe that includes worker-local buffer first-touch. It is not a universal CPU ranking and does not establish that one tile or scheduling mode is globally optimal.

Auto RSS is likewise invocation-scoped evidence. Because Linux `VmHWM` is process-wide and monotonic, PE #14 does not claim an isolated memory footprint for the selected engine from the in-process tuning run.

PE #14 deliberately avoids model-name heuristics and preserves all manual execution commands so the automatic policy can be audited against explicit alternatives.

## PASS boundary

PE #14 passes only if:

1. the streaming oracle matches the established canonical reference in tests;
2. every calibration candidate matches the oracle exactly;
3. every tiled candidate is measured at the same effective per-worker tile size it will have on the requested workload;
4. calibration preserves the requested frame depth;
5. every candidate score is expressed on the same requested particle-frame scale before promotion;
6. persistent promotion accounts for observed startup of the full requested pool shape **including worker-local buffer first-touch**;
7. physical-first fallback is reported as logical execution when physical topology is unavailable;
8. tuning time starts before topology detection and separately records topology and calibration-oracle components;
9. the selected full workload matches the full oracle exactly;
10. near-ties remain canonical because of the 5% promotion margin;
11. material measured/projected wins may promote to spawned or persistent SoA;
12. auto RSS is labeled as whole-invocation high-water evidence and never presented as an isolated selected-run peak;
13. public auto help is sourced from the live policy text rather than a duplicate dispatcher string;
14. existing canonical and manual optimized commands remain unchanged;
15. native CI passes on Linux x86-64, Linux ARM64, macOS ARM64, and Windows x86-64.
