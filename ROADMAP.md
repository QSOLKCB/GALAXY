# GALAXY Roadmap — Memory First, Everything Else Deferred

GALAXY v0.5.0 freezes the completed CPU optimization ladder:

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

**Only PE #15 is active. Everything else is deferred.**

The detailed pre-v0.5.0 roadmap remains permanently recoverable from the immutable `v0.5.0` tag. Main now records the actual execution order instead of presenting every interesting idea as simultaneous work.

---

# PE #15 — Memory Wall: Stream → Reduce → Discard

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

# Deferred backlog — do not implement until PE #15 closes

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

Deferral means **no implementation work and no promotion work** on these items while PE #15 is active, unless the roadmap is explicitly changed again.

The previous detailed designs are preserved by the immutable `v0.5.0` source archive and tag; they are not discarded, only postponed.
