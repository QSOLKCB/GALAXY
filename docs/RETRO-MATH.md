# Retro-deterministic math reference layer

GALAXY keeps its existing UFF/Newtonian/NFW/Burkert/MOND physics, browser renderer,
Wasm sampler and native GPU runtimes unchanged. This layer adds a small,
integer-only reference vocabulary for deterministic cross-backend validation.

It takes inspiration from three historical/research ideas without copying their
renderers or source code:

- **Elite (1984)**: a compact three-word galaxy seed recurrence. GALAXY uses the
  recurrence as a procedural hierarchy and adds logarithmic jump-ahead with a
  3x3 matrix modulo 2^16.
- **Doom (1993)**: binary angles and fixed-point arithmetic. GALAXY uses a BAM32
  full turn, integer CORDIC trigonometry and Q16.16 projection as a reference
  oracle.
- **Quandoom (2024)**: reduce a large computation to a small, auditable set of
  deterministic primitives and precompute/validate what can be frozen.

References:

- https://fabiensanglard.net/doomIphone/doomClassicRenderer.php
- https://github.com/markmoxon/elite-source-code-bbc-micro-cassette
- https://arxiv.org/html/2412.12162v1

No third-party source code is copied into this implementation.

## Boundary: reference math, not replacement astrophysics

The retro layer does **not** replace circular velocity, UFF dynamics, leapfrog
integration, GPU float32 state, or the existing `split-u64-hash32-avalanche-v1`
global particle identity contract.

Its purposes are:

1. provide exact seed/jump vectors;
2. provide exact modular angle/reversal tests;
3. provide deterministic fixed-point projection vectors independent of libm;
4. expose a sector salt that can be mixed into existing particle generation
   without using the Elite recurrence as a particle identity;
5. give CPU, JavaScript, Wasm/GPU work a stable oracle.

The default browser visuals and saved-state behaviour do not change in this PR.

## Elite-style hierarchy with a u64-safe salt

The reference seed is three unsigned 16-bit words. A single twist is

```text
(a, b, c) -> (b, c, a + b + c mod 2^16)
```

The same update can be written

```text
        [0 1 0]
M   =   [0 0 1]
        [1 1 1]
```

so a distant state is `M^n seed` modulo 2^16. `elite_jump()` computes this by
exponentiation by squaring in O(log n).

The historical-style recurrence repeats in the low-32 sector domain. GALAXY
therefore **does not** use that state as the global identity. `sector_salt()`
folds the three-word state and independently hashes both the low and high
32-bit halves of the requested u64 sector index through GALAXY's existing
avalanche hash. A regression deliberately proves:

```text
sector_seed(seed, 0) == sector_seed(seed, 2^32)
sector_salt(seed, 0) != sector_salt(seed, 2^32)
```

This makes the historical recurrence useful as procedural grammar without
weakening the current non-aliasing model.

## Doom-style BAM32 and fixed-point oracle

One full turn occupies all 32 bits:

```text
0x00000000 =   0 degrees
0x40000000 =  90 degrees
0x80000000 = 180 degrees
0xc0000000 = 270 degrees
wrap         360 degrees
```

Angle advance and retreat use wrapping arithmetic. Therefore, for every accepted
tick count,

```text
retreat(advance(angle, delta, n), delta, n) == angle
```

bit-for-bit.

`sin_cos_q30()` uses 24 committed integer CORDIC stages. The output is signed
Q2.30. `project_q16()` multiplies a signed Q16.16 radius by that reference
trigonometry. The path intentionally avoids runtime `sin()`, `cos()` and floating
point so Rust and JavaScript can agree exactly.

This is a validation oracle, not a claim that integer CORDIC is faster than a
modern GPU's native trigonometric instructions.

## Shared golden vectors

`tests/retro-vectors.txt` is consumed by both:

```bash
node tests/retro-math.mjs
cargo run --manifest-path rust/Cargo.toml --example retro_vectors --locked --offline
```

The vectors include:

- repeated and jump-ahead Elite states;
- the 2^32 recurrence boundary;
- sector salts above 2^32;
- BAM cardinal and non-cardinal angles;
- Q16.16 projections;
- multi-million and >2^32 tick exact reversal.

Normal CI runs both consumers.

## Local NVIDIA GPU validation

After cloning the PR branch on an NVIDIA workstation:

```bash
bash scripts/test-retro-gpu-local.sh
```

The script refuses to pretend that CI/software Vulkan is local GPU evidence. It
requires `nvidia-smi`, runs both shared retro reference consumers, checks the
existing native GALAXY reference suite on the selected hardware adapter, checks
the >u32 Vulkan addressing verifier, then executes the normal `spin-local.json`
GPU workload into a fresh timestamped directory.

The default adapter selector is `NVIDIA`. Override it with:

```bash
GALAXY_GPU_ADAPTER=0 bash scripts/test-retro-gpu-local.sh
```

or a more specific case-insensitive adapter-name substring.

The script does **not** install drivers, CUDA, Vulkan or Rust. It is intended for
a prepared local GALAXY development machine.

Optional CUDA u64 verification can be enabled when the repository-local CUDA
Python environment already exists:

```bash
GALAXY_RETRO_TEST_CUDA=1 bash scripts/test-retro-gpu-local.sh
```

No performance improvement is claimed until hardware results are recorded.
