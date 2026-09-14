# GALAXY native CPU runtime

PR #8 introduces a separate, headless Rust CPU execution path for GALAXY. It is
not a browser fallback and it does not modify the Vulkan/CUDA runtime. The goal
is to test a topology that is natural for CPUs after PR #7 showed that the
BAM32 interpolated lookup-table projection can substantially outperform native
floating-point `sin_cos()` in the bounded projection microbenchmark.

## Architecture

The runtime separates three quantities that the browser previously coupled more
tightly:

```text
exact u64 logical population
          |
          v
bounded resident sample
          |
          v
GALAXY physical angular rates
          |
          +---- float/libm reference projection
          |
          +---- BAM32 + 16K interpolated Q2.30 LUT
          |
          v
deterministic scalar or scoped-worker checksum stream
```

The full logical population is never allocated. `logical_population` may span the
complete positive `u64` range; the resident sample is bounded to 16,777,216
particles in this first implementation.

Logical indices use `u128` host arithmetic for exact proportional mapping. The
per-particle random lanes use the same named address contract as GALAXY's native
u64 Vulkan/CUDA runtime:

```text
split-u64-hash32-avalanche-v1
```

Both the low and high 32-bit halves of every logical ID participate in the lane
mixer, including across `2^32 - 1`, `2^32`, and `2^32 + 1`.

## Physics boundary

The resident set reuses `galaxy-sampler::physics` directly. The current prototype
uses GALAXY's default UFF parameters, the existing `star_radius()` mapping, and
`angular_rate()` with the same bounded bulge/shear defaults used by the visual
model. This avoids creating a second independent astrophysics implementation.

The CPU runtime does **not** turn GALAXY into a self-consistent N-body solver.
Particles remain independent test particles in the existing fixed-potential
model family.

## Projection backends

Two CPU paths are measured over exactly the same resident particles and frame
count:

- `float-libm`: native `f64::sin_cos()` projection;
- `bam-lut-q30`: BAM32 phase accumulation plus the 16,384-entry interpolated LUT
  introduced by PR #7.

The LUT is generated from the 24-stage integer CORDIC oracle. The runtime reports
the maximum deterministic Q2.30 LUT error against that oracle.

## Parallel execution contract

The runtime intentionally uses only the Rust standard library. It does not use
Rayon or create a persistent framework-specific scheduler.

Worker resolution is:

```text
effective_workers = min(requested_workers,
                        resident_particles,
                        available_parallelism,
                        256)
```

`std::thread::available_parallelism()` is treated as an environment capacity
signal, not proof of achieved multicore speedup. When the effective worker count
is one, execution degrades to the exact scalar path.

For more than one effective worker, the resident slice is divided into stable,
contiguous chunks. `std::thread::scope()` executes those chunks and results are
combined in worker order through a wrapping additive checksum. Scalar and
parallel checksums must match exactly.

A receipt sets `effective_multicore_claim = true` only when all of these are true:

1. the runtime exposes more than one available CPU;
2. more than one worker is actually selected;
3. scalar and parallel checksums match exactly; and
4. the measured parallel median is faster than the scalar median on that host.

This is environment-specific implementation evidence, not a universal scaling
claim and not direct hardware-counter proof of simultaneous core occupancy.

## Historical provenance boundary

The scheduling design was informed by earlier QSOL work but is implemented here
from scratch against current GALAXY contracts.

- QEC v170.2.0 established the rule that a requested worker count alone does not
  prove effective multicore execution.
- QEC v170.2.1 validated environment-specific multicore evidence only alongside
  observed worker/thread behaviour and measured speedup.
- A user-supplied historical NEXUS source archive was inspected for its scheduling
  semantics: `available_parallelism()`, deterministic contiguous chunking,
  scoped standard-library threads, bounded worker counts, and scalar fallback.
  The archive SHA-256 supplied during PR development was:
  `d354b0e8edb6eca41d78f967369d1294a5ae9d494e1c10979320c738bdc0e30a`.
  NEXUS is **not** a dependency of this runtime and no deleted-repository source
  file is copied into GALAXY.
- GLUBALL remains part of the current QSOL project lineage, but PR #8 does not
  copy GLUBALL source or create a build dependency on it.

GALAXY remains the implementation authority for this runtime.

## Local use

Run the verification suite and the default 1,048,576-resident benchmark:

```sh
sh scripts/test-cpu-runtime.sh
```

The script is POSIX `sh` syntax. Useful overrides include:

```sh
GALAXY_CPU_RESIDENT=8388608 \
GALAXY_CPU_FRAMES=8 \
GALAXY_CPU_REPEATS=7 \
GALAXY_CPU_WORKERS=32 \
sh scripts/test-cpu-runtime.sh
```

Or invoke the binary directly:

```sh
cargo run --manifest-path cpu-runtime/Cargo.toml --release --locked --offline -- \
  bench \
  --logical 18446744073709551615 \
  --resident 1048576 \
  --frames 8 \
  --workers 32 \
  --repeats 5 \
  --receipt runs/cpu-runtime-local/receipt.json
```

The receipt schema is:

```text
galaxy.cpu-runtime-receipt.v1
```

It records architecture, OS, exact logical population, resident population,
requested/available/effective workers, scalar/parallel timings, checksums,
measured speedups, LUT error, and bounded claim flags.

## What this PR does not claim

- It does not prove that every machine benefits from parallel execution.
- It does not prove that CPU projection is faster than GALAXY's GPU runtime.
- It does not render pixels or replace the browser/WebGL renderer yet.
- It does not allocate the logical `u64` population.
- It does not import or revive NEXUS.
- It does not change the existing Vulkan/CUDA kernels or browser sampler.

The immediate purpose is to establish a clean, measurable native CPU execution
backend before considering rasterisation, persistent worker pools, SIMD, or a
larger end-to-end CPU renderer.
