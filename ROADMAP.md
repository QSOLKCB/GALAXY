# GALAXY Roadmap — Evidence First, Self-Gravity Next

GALAXY v0.6.0 freezes the completed CPU optimization and memory-wall ladder:

```text
PR #10  SIMD/autovectorization feasibility
PR #11  worker-local bounded SoA execution
PR #12  guarded production integration
PR #13  persistent workers + topology-aware scheduling
PR #14  calibrated host-aware promotion with fail-closed parity
```

The governing rule remains:

> **Do not store what can be regenerated exactly. Do not materialize what can be reduced exactly. Do not transfer what can be summarized exactly.**

## Current status

**PE #15 is frozen in immutable v0.6.0. The active experimental phase is BH #1: resident Barnes–Hut self-gravity.**

The existing prescribed-field, logical-u64, CPU and GPU execution contracts remain intact. Self-gravity is a separate resident execution family because forces couple bodies across the complete resident set.

---

# BH #1 — Resident Barnes–Hut Self-Gravity Reference

## Goal

Establish a correctness-first mutually coupled N-body surface before any GPU promotion.

The first rung contains:

- deterministic planar quadtree construction;
- mass and centre-of-mass aggregation;
- classical `s / d < theta` opening control;
- explicit rejection of aggregate cells containing the target;
- Plummer-style softening;
- kick–drift–kick leapfrog integration;
- independent O(N²) direct-force verification;
- browser quadtree visualization and deterministic error probes;
- native receipts that state the resident-only claim boundary.

### Acceptance

1. `theta = 0` agrees with the independent direct-force oracle to floating reduction-order roundoff.
2. The default `theta = 0.5` deterministic fixture stays inside explicit RMS and worst-relative error gates.
3. Coincident bodies terminate under a bounded tree depth and remain finite with positive softening.
4. The established UFF/test-particle browser and runtime interfaces are unchanged by default.
5. No logical-u64 tiling claim is made for mutually coupled self-gravity.
6. GPU speed claims are deferred until a GPU implementation is measured against this frozen CPU/direct reference.

## BH #2 — Deferred GPU tree work

After BH #1 closes, investigate:

```text
bounding box
  -> Morton/Z-order keys
  -> stable spatial ordering
  -> flat tree / aggregate construction
  -> GPU traversal
  -> leapfrog update
```

Tree construction and traversal timing must be reported separately. Third-party benchmark numbers are research context, not GALAXY evidence.

See `docs/BARNES-HUT.md` and `nbody/`.

---

# Historical PE #15 — Memory Wall: Stream → Reduce → Discard

## Goal

Reduce GALAXY's hot CPU working set while preserving exact BAM-LUT behaviour and deterministic wrapping-`u64` evidence.

The phase attacks two concrete sources of avoidable storage:

1. the per-particle `u64` contribution array that exists only to be summed and discarded;
2. the 128 KiB full sine+cosine 16K Q2.30 LUT.

It also tests whether smaller worker microtiles improve cache behaviour without paying an unacceptable scheduling or loop-overhead penalty.

## 15A — Fuse contribution generation and reduction

Current optimized tile shape:

```text
persistent fields   20 B / particle
x/y scratch          8 B / particle
contribution array   8 B / particle
-----------------------------------
                     36 B / particle
```

Candidate:

```text
persistent fields   20 B / particle
x/y scratch          8 B / particle
hash/reduce output   0 B / particle
-----------------------------------
                     28 B / particle
```

The candidate must compute the exact existing contribution hash and fold it directly into a wrapping-`u64` checksum. The full contribution array is not allowed in fused mode.

### Acceptance

- fused result exactly equals materialized batch + reduction;
- worker completion order cannot alter the checksum;
- native disassembly remains inspectable;
- memory accounting records the removed 8 bytes per tiled particle;
- no production promotion until measured wall time is acceptable.

## 15B — Cache-sized microtiles

Required sweep:

```text
1024
512
256
128
64
```

Each microtile is generated once, consumed across all requested frames, reduced, and discarded before the next tile is generated.

Do not regenerate particle fields once per frame.

At 32 workers the declared fused tile capacities are:

| Microtile | Fused tile bytes |
|---:|---:|
| 1024 | 917,504 |
| 512 | 458,752 |
| 256 | 229,376 |
| 128 | 114,688 |
| 64 | 57,344 |

The winner is measured, not guessed. A smaller tile that destroys throughput is not automatically better.

## 15C — Exact cosine-only LUT

The current full table stores 16,384 cosine and 16,384 sine `i32` values: 131,072 bytes.

A pure quarter-turn phase identity is not assumed to reproduce the integer CORDIC table bit-for-bit. The candidate therefore stores one full cosine table plus compact exact sine corrections and rejects construction if reconstruction is not exact.

Target representation implemented by the probe:

```text
cosine samples        16,384 × i32
sine correction       16,384 × i16
```

Declared storage: 98,304 bytes.

## 15D — Exact quarter-wave LUT

Store a 4,097-entry canonical quarter-cosine table and reconstruct quadrants. Any integer-CORDIC asymmetry is retained through an exact correction palette and one-byte correction codes for cosine and sine.

The representation must fail closed unless:

- all corrections fit the declared signed width;
- the shared correction palette fits one-byte indices;
- all 16,384 reconstructed cosine/sine sample pairs equal the canonical table exactly;
- interpolation remains exact for representative non-table BAM angles.

Actual storage is receipt evidence, not a predeclared performance claim.

## 15E — Evidence matrix

The experiment crosses:

```text
reduction:
  materialized
  fused

LUT:
  full
  cosine-corrected
  quarter-corrected

microtile:
  1024
  512
  256
  128
  64
```

Every matrix cell runs in an isolated process for meaningful Linux `VmHWM` evidence.

Record:

```text
checksum
median_ns
best_ns
peak_rss_kib
lut_storage_bytes
working_bytes_per_tiled_particle
worker_microtile_capacity_bytes
algorithmic_working_set_bytes
workers
microtile
reduction mode
LUT mode
```

## PE #15 PASS boundary

PASS requires:

1. exact canonical checksum parity everywhere;
2. exact LUT sample reconstruction for compressed modes;
3. exact fused/materialized contribution parity;
4. materially lower algorithmic working set;
5. no material wall-time regression for the candidate proposed for production;
6. isolated-process RSS evidence where the OS exposes it;
7. no host-independent performance claim;
8. the frozen v0.5.0 canonical/manual paths remain available until a later explicit promotion PR.

The first implementation is an **experimental probe**, not a silent change to `galaxy-cpu`.

See `docs/CPU-MEMORY-WALL.md` and `scripts/bench-cpu-memory-wall.sh`.

---

# Historical PE #15 deferred backlog

All of the following remain useful research directions, but they are explicitly deferred:

- heterogeneous simultaneous CPU + GPU execution;
- GPU stream → reduce → discard and compact readback;
- further procedural-state elimination beyond the PE #15 microtile experiment;
- NUMA-local persistent pools, affinity experiments, and huge-page tests;
- multi-GPU deterministic sharding;
- broader precomputation experiments;
- inline integer CORDIC as a production candidate;
- output/image accumulation without particle materialization;
- double-buffered host/device transfer pipelines;
- symmetry/orbit compression;
- memory-budgeted automatic execution selection.

These items were deferred while PE #15 was active. v0.6.0 closed that phase; any future work now requires an explicit phase with its own evidence boundary.

The previous detailed designs are preserved by the immutable `v0.5.0` source archive and tag; they are not discarded, only postponed.
