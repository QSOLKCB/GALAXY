# GALAXY worker-local SoA CPU experiment

PR #11 is the first end-to-end CPU experiment after the isolated SIMD work in PR #10. It does **not** replace `galaxy-cpu`.

The experimental binary is `worker_soa_probe`. Its purpose is to answer one question:

> Can the exact BAM-LUT runtime preserve GALAXY's checksum contract while replacing the full resident AoS with bounded worker-local SoA tiles, and does that shape improve end-to-end wall time and memory behaviour on real CPUs?

## Why this phase exists

The local Ryzen 9 5950X PR #10 evidence established a strong isolated contribution-hash result:

- generic median: `34,942,130 ns`
- native median: `10,742,437 ns`
- native speedup: `3.252719099x`
- median time reduction: `69.256491%`
- checksum: `e3f89b31c3f4c4e8`
- generic code generation: legacy SSE2 packed operations
- native code generation: AVX2/VEX packed operations
- EVEX/AVX-512: absent, as expected for Zen 3

That evidence was intentionally narrow. PR #11 moves the same exact batch hash into a real BAM-LUT projection path and makes resident generation, projection, contribution hashing, and deterministic worker reduction part of the timed region.

## Reference path

`--path reference` mirrors the current canonical BAM-LUT runtime shape:

- full resident `ReferenceParticle` vector;
- 40-byte particle layout;
- deterministic logical-u64 sampling;
- frame-major traversal;
- contiguous worker chunks;
- interpolated 16K BAM LUT;
- exact contribution hash;
- wrapping-u64 reduction.

The reference path exists only as the correctness/performance oracle inside the experiment. The canonical implementation remains `cpu-runtime/src/main.rs`.

## Worker-local SoA path

`--path soa` does not allocate the full resident particle vector. Each worker owns a bounded tile containing:

- `id_lo: u32`
- `id_hi: u32`
- `radius_q16: i32`
- `initial_bam: u32`
- `delta_bam: u32`

For each frame in the tile it also owns scratch arrays for:

- projected `x` word;
- projected `y` word;
- 64-bit contribution output.

The five persistent compact fields require 20 bytes per tiled particle. The x/y/output scratch adds 16 bytes per tiled particle. The working-set capacity therefore scales with `workers × tile`, rather than `resident`.

The tile loop is particle-local at tile granularity: generate one bounded particle tile, process all requested frames for that tile, reduce its contributions, then advance to the next particle tile.

The contribution stage calls the exported `galaxy_worker_soa_hash_batch` symbol. It uses the same source shape proven by PR #10 and contains no ISA-specific intrinsics. Generic versus host-native code generation remains a compiler/build concern.

## Exactness contract

The prototype must remain bit-exact with the canonical BAM-LUT path.

`worker_soa_probe verify` checks:

1. the reference resident layout is still 40 bytes;
2. the reference scalar checksum equals the reference parallel checksum;
3. worker-local SoA matches the reference checksum across awkward tile boundaries;
4. SoA checksum is invariant to worker partitioning;
5. the SoA batch hash matches the canonical contribution function across u64 boundary cases.

A performance result is invalid if any checksum differs.

## Timing scope

The primary receipt field is:

`timing_scope=resident-generation-plus-bam-lut-projection-plus-contribution-plus-worker-reduction`

Both paths include resident-state generation in the timed region.

The LUT itself is built outside the timed region because it is common immutable runtime infrastructure and is not affected by the AoS/SoA hypothesis.

Each path is warmed once before measured repeats. The receipt reports best and median nanoseconds.

## RSS evidence

On Linux, the binary records `/proc/self/status` `VmHWM` as `peak_rss_kib`.

This is useful because the sweep executes reference and SoA as separate processes. The reference process therefore exposes the full resident allocation in its high-water mark, while the SoA process exposes the bounded worker/tile working set.

On platforms without `/proc/self/status`, `peak_rss_kib` is `null`. Do not manufacture an RSS comparison from that field.

## Benchmark sweep

Run the bounded default matrix:

```sh
./scripts/bench-cpu-worker-soa.sh
```

Defaults:

- resident: `262144`
- frames: `8`
- repeats per case: `3`
- workers: `1 2 4 8 16 32`
- tiles: `1024 4096 16384`

For a stronger local confirmation on the Ryzen 9 5950X:

```sh
GALAXY_SOA_RESIDENT=1048576 \
GALAXY_SOA_FRAMES=8 \
GALAXY_SOA_REPEATS=5 \
GALAXY_SOA_WORKERS='1 2 4 8 16 32' \
GALAXY_SOA_TILES='1024 4096 16384 65536' \
./scripts/bench-cpu-worker-soa.sh
```

The runner builds the same source twice:

- generic target CPU;
- `-C target-cpu=native`.

For every worker/tile case it runs four configurations:

1. generic reference;
2. generic SoA;
3. native reference;
4. native SoA.

The ordering alternates between cases to reduce systematic thermal/order bias.

## Output

The runner writes a fresh evidence directory containing:

- `host.txt`
- `verify.txt`
- `matrix.tsv`
- `comparison.tsv`
- `summary.txt`
- one JSON receipt and stdout log per matrix cell
- generic/native disassembly of `galaxy_worker_soa_hash_batch` when `objdump` is available
- separate generic/native Cargo target directories

The output directory must be empty before the run starts. Old evidence is never silently overwritten or mixed into a new sweep.

### `matrix.tsv`

Records raw measurements:

- build (`generic` or `native`)
- path (`reference` or `soa`)
- requested/effective workers
- tile size
- median/best nanoseconds
- checksum
- peak RSS when available
- receipt path

### `comparison.tsv`

Pivots matching worker/tile cells and reports:

- generic algorithmic speedup: generic reference / generic SoA
- native algorithmic speedup: native reference / native SoA
- total speedup: generic reference / native SoA
- native gain within SoA: generic SoA / native SoA
- reference/SoA RSS values

## Acceptance gates

PR #11 should be treated as **PASS** only if a representative local sweep establishes all of the following:

1. every reference/generic/native/SoA cell has exactly the same checksum;
2. the result is stable across useful worker counts;
3. at least one practical tile range produces a repeatable **end-to-end** SoA win, not merely a faster isolated hash;
4. the winning tile size is not a single pathological outlier;
5. native SoA code generation retains useful vector instructions in `galaxy_worker_soa_hash_batch`;
6. Linux peak RSS does not regress unacceptably and ideally falls materially versus the full resident reference path;
7. worker-count scaling remains deterministic;
8. no conclusion is generalized beyond the measured host/configuration.

If timing improves but RSS or worker scaling regresses badly, the result is **HOLD**.

## Production boundary

This experiment deliberately avoids changing `galaxy-cpu`.

A later production PR may reuse the winning worker/tile shape only after this matrix is reviewed. That later change should retain the current implementation as a fallback until full runtime fixtures, worker counts, supported architectures, and memory evidence are green.

PR #11 therefore tests architecture, not deployment policy.
