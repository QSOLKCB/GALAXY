# GALAXY

[![Release](https://img.shields.io/badge/release-v0.7.0-2f81f7)](https://github.com/QSOLKCB/GALAXY/releases/tag/v0.7.0)
[![DOI](https://zenodo.org/badge/DOI/10.5281/zenodo.22756969.svg)](https://doi.org/10.5281/zenodo.22756969)

**GALAXY is an offline deterministic galaxy-dynamics instrument with browser, native CPU, and native GPU execution paths.**

It began as an adaptation of the VORTEX 2.1.0 particle lab and now combines:

- interactive rotation-curve visualization;
- UFF, Newtonian, NFW, Burkert, MOND/RAR, and authored visual rotation laws;
- deterministic Rust/WebAssembly sampling in the browser;
- native Rust CPU execution with Float/libm and BAM32/Q2.30 LUT backends;
- Vulkan/`wgpu` and NVIDIA CUDA compute paths;
- memory-bounded exact-u64 logical addressing;
- reproducible benchmark receipts, topology evidence, and archived scaling studies;
- an opt-in Barnes–Hut resident self-gravity laboratory with a direct-force oracle and live quadtree visualization;
- a GPU-oriented BH #2A Morton/Z-order + flat-cell CPU substrate with repeatable topology receipts;
- a BH #2B1 explicit f32/u32 transfer ABI with executable Vulkan/WGSL flat-tree traversal and matched CUDA traversal source;
- BH #2B2 multi-step resident self-gravity with persistent GPU state, GPU kick/drift/final-kick kernels, and an explicit CPU tree-rebuild boundary;
- BH #2C correctness-first GPU tree construction, removing host particle/tree rebuilds from the evolution loop for bounded resident workloads.

The current immutable software release is **v0.7.0**:

<https://github.com/QSOLKCB/GALAXY/releases/tag/v0.7.0>

The v0.4.0 native-CPU evidence baseline remains archived at Zenodo as:

> Slade, T. (2026). *GALAXY v0.4.0: Deterministic Native CPU Runtime and Scaling Evidence* (Version v0.4.0) [Computer software]. Zenodo. https://doi.org/10.5281/zenodo.22756969

**GitHub Pages:** <https://qsolkcb.github.io/GALAXY/>

---

## Project status

| Layer | Current state |
| --- | --- |
| Browser instrument | Offline HTML/CSS/JS + bundled Rust/Wasm; no server or CDN required |
| Browser logical population | Up to `2^32 = 4,294,967,296` logical stars on the Rust/Wasm path |
| Browser rendered sample | Up to 65,536 stars per frame on Rust/Wasm + WebGL |
| Native CPU runtime | Exact positive-u64 logical population, bounded resident sample up to 16,777,216 particles |
| CPU projection backends | `float-libm` and `bam-lut-q30` |
| Native GPU runtime | Rust/`wgpu`/Vulkan plus NVIDIA CUDA/CuPy RawKernel |
| Wide logical addressing | `split-u64-hash32-avalanche-v1` across CPU and wide-address GPU paths |
| Barnes–Hut self-gravity | Separate resident planar N-body reference, exact O(N²) oracle, leapfrog integration, browser tree overlay |
| Barnes–Hut flat substrate | Stable 32-bit Morton ordering, pointer-free flat cells, bottom-up aggregates, flat traversal receipts |
| Barnes–Hut GPU traversal | CPU-built flat tree packed to frozen f32/u32 records; Vulkan/WGSL traversal verified against the full flat CPU oracle plus bounded direct-force probes; CUDA ABI/source parity |
| Barnes–Hut GPU evolution | Persistent f32 GPU state; GPU force + leapfrog kick/drift/final-kick; BH #2B2 host rebuild oracle plus BH #2C device-only rebuild path |
| Barnes–Hut GPU tree build | GPU bounds, Morton generation, deterministic bitonic ordering, flat topology and aggregates; correctness-first cap 4,096 |
| Formal release | Immutable `v0.7.0`, commit `7fe4dc63d40bb4afbb93f51c07d05b49f21e9756` |
| CPU baseline archival record | Zenodo DOI `10.5281/zenodo.22756969` |
| Active experimental phase | BH #2D parallel GPU tree construction and hardware performance validation |

v0.6.0 froze the Stream → Reduce → Discard CPU memory-wall result. **v0.7.0 freezes the Barnes–Hut correctness ladder through BH #2C**, including the CPU oracle, flat-tree substrate, executable GPU traversal, persistent GPU leapfrog evolution, and device-side tree construction.

The earlier v0.4.0 archive remains the **before-state** for the CPU architecture ladder.

---

## 1. Browser instrument

Open **`index.html`** directly in a modern browser. No install, local server, CDN, or network connection is required, including for the bundled Rust/WebAssembly engine.

The separate **`barnes-hut.html`** entrypoint is an opt-in resident self-gravity lab with a live quadtree overlay and direct-force error probes. It does not change the default rotation-law instrument.

The browser instrument lets you change morphology, mass model, viewing geometry, and time while watching the galaxy and its rotation curve respond together.

### Rotation laws

| Mode | Contribution and controls |
| --- | --- |
| UFF empirical v4 + baryons | UFF velocity scale, core radius, and bounded shape β |
| Newtonian baryons | Signed gas contribution, disc and bulge mass-to-light ratios |
| NFW halo + baryons | Halo mass M₂₀₀ and concentration |
| Burkert halo + baryons | Central halo density and core radius |
| MOND / RAR | UFF exponential acceleration relation and adjustable a₀ |
| Original visual rotation | Authored visual rotation law and manual shear |

An optional weak-field central mass applies to all physical models. Defaults use the UFF demonstration parameters; **no fitting is performed**.

The bundled `data/uff/DEMO_GALAXY.csv` is demonstration input, not an identified observational catalogue. See [UFF dynamics](docs/UFF-DYNAMICS.md) for equations, units, provenance, and interpolation choices.

### Browser capacity

Logical population and rendered population are separate quantities. Increasing the logical count does **not** allocate one object per logical star.

| Active engine | Maximum logical stars | Maximum rendered stars per frame |
| --- | ---: | ---: |
| JavaScript + Canvas or WebGL | `2^24 = 16,777,216` | 1,024 |
| Rust/Wasm + Canvas fallback | `2^32 = 4,294,967,296` | 1,024 |
| Rust/Wasm + WebGL | `2^32 = 4,294,967,296` | 65,536 |

The accelerated browser path defaults to **16,384 rendered stars**. The largest rendered sample uses about **2.25 MiB** for the core star-property and orbital-rate buffers, excluding Wasm allocator, framebuffer, transient, browser, and GPU-copy overhead.

See [engine notes](docs/ENGINE.md) for the browser execution contract.

---

## 2. Native CPU runtime

[`cpu-runtime/`](cpu-runtime/) is a separate headless Rust execution path. It is not a browser fallback and it does not replace the GPU runtime.

The CPU runtime separates:

```text
exact positive-u64 logical population
              |
              v
bounded resident particle sample
              |
              v
GALAXY physical angular rates
              |
       +------+------+
       |             |
  float/libm     BAM32 + Q2.30 LUT
       |             |
       +------+------+
              |
              v
deterministic scalar / parallel checksum stream
```

The logical population may span the complete positive `u64` range:

```text
18,446,744,073,709,551,615
```

while the resident sample is bounded to **16,777,216** particles in v0.4.0. The full logical population is never allocated.

Logical IDs use exact host-side proportional mapping and the named identity contract:

```text
split-u64-hash32-avalanche-v1
```

Both halves of the 64-bit logical ID participate in the mixer, including across the `2^32 - 1`, `2^32`, and `2^32 + 1` boundary.

### CPU projection backends

- **`float-libm`** — native `f64::sin_cos()` projection.
- **`bam-lut-q30`** — BAM32 phase accumulation with a 16,384-entry interpolated Q2.30 lookup table generated from the integer CORDIC reference path.

The current deterministic LUT diagnostic evaluates 8,193 angles and reports a sampled maximum absolute Q30 error of **255**. That is a sampled implementation diagnostic, not a proven global bound over all `2^32` BAM angles.

### Parallel execution contract

v0.4.0 intentionally uses Rust standard-library scoped threads rather than Rayon or a persistent worker framework.

```text
effective_workers = min(requested_workers,
                        resident_particles,
                        available_parallelism,
                        256)
```

Resident slices are divided into stable contiguous chunks, results are combined deterministically in worker order, and scalar/parallel checksums must match exactly.

Backend timing uses the named schedule:

```text
interleaved-alternating-v1
```

which alternates complete Float/LUT and scalar/parallel paths across repeats to reduce systematic ordering bias.

### CPU quick start

```sh
cargo test --manifest-path cpu-runtime/Cargo.toml --locked --offline
cargo build --manifest-path cpu-runtime/Cargo.toml --release --locked --offline

cpu-runtime/target/release/galaxy-cpu verify --workers 32

cpu-runtime/target/release/galaxy-cpu bench \
  --logical 18446744073709551615 \
  --resident 8388608 \
  --frames 8 \
  --workers 32 \
  --repeats 5 \
  --seed 303 \
  --receipt runs/cpu-runtime-local/receipt.json
```

See [native CPU runtime](docs/CPU-RUNTIME.md), [CPU portability](docs/CPU-PORTABILITY.md), and [retro integer math](docs/RETRO-MATH.md).

---

## 3. Native GPU runtime

[`runtime/`](runtime/) runs headless compute jobs on local or cloud GPUs. It evolves actual resident particle state and supports circular spin, perturbed leapfrog orbits, model comparisons, UFF rotation-curve sweeps, compact-object diagnostics, and memory-bounded wide-address workloads.

Two NVIDIA-capable Linux paths are maintained:

- **Vulkan / Rust `wgpu`** when a real hardware Vulkan adapter is available;
- **CUDA / CuPy RawKernel** when CUDA/NVML is available, including managed environments where Vulkan is not exposed.

Basic local flow:

```bash
bash scripts/run-gpu.sh devices
bash scripts/run-gpu.sh verify
bash scripts/run-gpu.sh run \
  --job runtime/jobs/spin-local.json \
  --output runs/local-spin
```

The wide-address tiled runtime can represent logical populations beyond `2^32` without allocating the full logical population at once. Tiling is valid for the current independent-particle fixed-potential workload because there are no cross-particle forces between tiles.

The Barnes–Hut GPU path is separate and resident-only.

BH #2B1 consumes the BH #2A CPU-built flat tree through an explicit f32/u32 ABI and executes force traversal on Vulkan/WGSL.

BH #2B2 adds persistent GPU state and multi-step leapfrog evolution while retaining an explicit CPU tree-rebuild boundary.

BH #2C removes that per-step host rebuild boundary for bounded resident workloads by constructing the Morton-ordered flat tree directly on the GPU.

```bash
cargo run --manifest-path runtime/Cargo.toml --locked --bin galaxy-bh-gpu -- \
  --particles 256 --theta 0.5 --allow-software \
  --receipt runs/barnes-hut-gpu/receipt.json

cargo run --manifest-path runtime/Cargo.toml --locked --bin galaxy-bh-evolve -- \
  --preset disc --particles 256 --steps 4 --dt-myr 0.01 \
  --theta 0.5 --allow-software \
  --receipt runs/barnes-hut-evolve/receipt.json

cargo run --manifest-path runtime/Cargo.toml --locked --bin galaxy-bh-gpu-tree -- \
  --preset disc --particles 128 --steps 3 --dt-myr 0.01 \
  --theta 0.5 --allow-software \
  --receipt runs/barnes-hut-gpu-tree/receipt.json

python3 runtime/cuda/check_barnes_hut_layout.py
```

The BH #2B2 path keeps acceleration and state buffers resident across force/integration stages while using an explicit host rebuild boundary.

BH #2C removes that per-step host boundary: the persistent drifted state feeds GPU bounds, Morton generation, deterministic ordering, flat-cell construction, aggregate construction, and Barnes–Hut traversal directly.

The v0.7.0 BH #2C builder is deliberately correctness-first:

- 4,096-body resident cap;
- serialized GPU bounds reduction;
- parallel Morton generation;
- deterministic GPU bitonic ordering by `(Morton code, body index)`;
- parallel target-position assignment;
- serialized level-order topology construction;
- serialized bottom-up aggregate construction;
- zero host particle readbacks during evolution steps;
- zero CPU tree rebuilds during evolution steps.

This establishes device ownership and correctness. It is **not** presented as a production GPU-tree performance architecture.

`--allow-software` enables verification through software Vulkan and must not be interpreted as hardware GPU performance evidence.

The CUDA checker freezes ABI/source parity without claiming CUDA execution where no NVIDIA CUDA run occurred.

See:

- [GPU runtime](docs/GPU-RUNTIME.md)
- [Barnes–Hut self-gravity](docs/BARNES-HUT.md)
- [CUDA runtime](docs/CUDA-RUNTIME.md)
- [u64 tiled runtime](docs/U64-TILED-RUNTIME.md)
- [qBraid CPU/GPU execution index](QBRAID.md)

---

## 4. v0.4.0 reproducibility milestone

v0.4.0 formalizes the first archived native-CPU performance baseline for GALAXY.

### Ryzen 9 5950X local validation

The local validation reached the **16,777,216-particle resident cap** with deterministic scalar/parallel checksum parity.

A primary 8,388,608-resident / 32-worker result measured:

| Backend | Scalar median | Parallel median | Measured speedup |
| --- | ---: | ---: | ---: |
| Float | 1.623 s | 97.37 ms | 16.67× |
| BAM-LUT | 504.6 ms | 84.56 ms | 5.97× |

The local matrix showed Float continuing to benefit through 32 workers while BAM-LUT plateaued substantially earlier at the larger resident sizes. That observation motivated the wider cloud replication.

### qBraid / Azure EPYC 7763 replication

The completed qBraid experiment ran on an Azure-hosted guest exposing:

```text
AMD EPYC 7763 64-Core Processor
96 online logical CPUs
48 exposed cores
2 SMT threads per exposed core
2 NUMA nodes
192 MiB aggregate L3 reported by lscpu
~377 GiB RAM
```

For an 8,388,608-resident workload with 8 frames, 5 repeats, and seed 303, the canonical unpinned sweep produced:

| Workers | Float parallel | Float speedup | BAM-LUT parallel | BAM-LUT speedup |
| ---: | ---: | ---: | ---: | ---: |
| 32 | 121.02 ms | 20.30× | 45.06 ms | 16.65× |
| **48** | **77.07 ms** | **31.71×** | **31.46 ms** | **24.10×** |
| 64 | 92.23 ms | 26.65× | 38.58 ms | 19.53× |
| 96 | 81.49 ms | 29.92× | 35.52 ms | 21.22× |

**48 workers was the best measured unpinned point for both backends.** Scaling was not monotonic beyond that point, and all tested worker counts preserved deterministic backend checksums.

### Topology / affinity probe

A repeat-matched 48-worker study then compared the unrestricted run with explicit affinity configurations:

| 48-worker configuration | Float parallel | BAM-LUT parallel |
| --- | ---: | ---: |
| Unpinned | **77.22 ms** | **31.38 ms** |
| NUMA node 0 only | 78.12 ms | 33.01 ms |
| NUMA node 1 only | 77.10 ms | 34.33 ms |
| One SMT thread per exposed core across both NUMA nodes | 115.54 ms | 47.84 ms |

The cross-NUMA one-thread-per-core run was roughly **50% slower** for both backends, while node-local 48-thread runs remained close to the unpinned baseline.

This is strong evidence that the workload is **topology-sensitive in that environment**. It is consistent with NUMA locality, first-touch placement, remote-memory traffic, cache effects, or bandwidth interactions, but the experiment did not collect hardware memory-traffic counters and therefore does **not** isolate the underlying mechanism.

The historical `galaxy.cpu-runtime-receipt.v1` schema does not encode Linux affinity. Exact mask-to-receipt provenance for the completed runs is preserved separately in [`AFFINITY-MANIFEST.md`](evidence/qbraid/EPYC7763-20260914/AFFINITY-MANIFEST.md). Future affinity runs are required to capture the kernel-visible `Cpus_allowed_list` inside the constrained process.

### Archived evidence

The qBraid evidence is preserved in:

```text
evidence/qbraid/EPYC7763-20260914/
```

The original compressed evidence bundle SHA-256 is:

```text
5c0474b9537a0ee34493a58c22b368f4846ad12f41633430d4444a678561e87a
```

The immutable GitHub release is:

<https://github.com/QSOLKCB/GALAXY/releases/tag/v0.4.0>

The formal software record is:

<https://doi.org/10.5281/zenodo.22756969>

---

## 5. Scientific boundary

GALAXY is a **deterministic galaxy dynamics and visualization instrument with two deliberately separate dynamics families**.

The established browser, CPU, and GPU rotation-law runtimes use prescribed gravitational fields and evolve independent test particles. Their huge logical populations remain deterministic address spaces from which bounded resident populations are sampled or tiled.

The opt-in Barnes–Hut laboratory is different: it implements pairwise-derived evolving self-gravity approximately through a resident quadtree, with an exact O(N²) force oracle for verification.

Every resident body contributes to the coupled force field, so this path does **not** claim that independent logical-u64 tiles can stand in for one mutually interacting population.

The Barnes–Hut correctness ladder in v0.7.0 now includes:

```text
BH #1   recursive CPU Barnes–Hut + direct-force oracle
   |
BH #2A  deterministic Morton / flat-tree CPU reference
   |
BH #2B1 GPU transfer ABI + executable force traversal
   |
BH #2B2 persistent GPU leapfrog with host tree rebuild
   |
BH #2C  GPU-built flat tree + device-only evolution rebuild loop
```

GALAXY still does **not** implement:

- hydrodynamics;
- gas evolution;
- star formation;
- a fully self-consistent baryonic/dark-matter density solver;
- distributed mutually interacting logical-u64 populations;
- production-quality parallel GPU tree construction.

A logical population of `u64::MAX` does **not** mean that 18.4 quintillion mutually interacting particles are resident in memory.

Performance claims are similarly bounded: benchmark receipts establish behavior for the recorded source, workload, and environment. They do not establish universal Ryzen, EPYC, cloud, CPU-vs-GPU, NUMA, CUDA, Vulkan, or memory-bandwidth claims.

The v0.7.0 GPU Barnes–Hut CI evidence was collected through Mesa software Vulkan and is correctness evidence, **not hardware GPU performance evidence**.

---

## 6. Development and verification

The repository contains eight main validation surfaces:

```text
browser / Wasm             -> JS application + physics + packaging checks
Barnes–Hut self-gravity    -> JS + native Rust tree math against direct O(N²) forces
retro integer math         -> Rust + JS portability / vector checks
native CPU runtime         -> Linux / macOS / Windows correctness and receipts
native GPU / u64           -> Vulkan/CUDA host contracts and tiled-runtime checks
Barnes–Hut GPU traversal   -> packed ABI + WGSL execution against flat/direct CPU oracles
Barnes–Hut GPU evolution   -> persistent GPU state + multi-step leapfrog
Barnes–Hut GPU tree build  -> device-only rebuild loop checked against BH #2A/B2B2 oracles
```

Useful local checks include:

```bash
# Browser / Wasm
cargo test --manifest-path rust/Cargo.toml --locked --offline
bash scripts/build-wasm.sh
node tests/smoke.mjs
node tests/physics.mjs
node tests/app.mjs
node tests/benchmark.mjs
node tests/barnes-hut.mjs

# Barnes-Hut native reference
cargo test --manifest-path nbody/Cargo.toml --locked --offline
cargo run --manifest-path nbody/Cargo.toml --locked --offline -- verify
cargo run --manifest-path nbody/Cargo.toml --locked --offline -- verify-flat

# Barnes-Hut GPU traversal / evolution / tree construction
cargo test --manifest-path runtime/Cargo.toml --locked
python3 runtime/cuda/check_barnes_hut_layout.py

cargo run --manifest-path runtime/Cargo.toml --locked --bin galaxy-bh-gpu -- \
  --particles 256 --theta 0.5 --allow-software

cargo run --manifest-path runtime/Cargo.toml --locked --bin galaxy-bh-evolve -- \
  --particles 128 --steps 3 --dt-myr 0.01 --allow-software

cargo run --manifest-path runtime/Cargo.toml --locked --bin galaxy-bh-gpu-tree -- \
  --particles 128 --steps 3 --dt-myr 0.01 --allow-software

# Native CPU
sh scripts/test-cpu-runtime.sh

# Retro CPU portability
sh scripts/test-retro-cpu.sh

# CUDA host-side tests
python3 tests/test_cuda_u64.py
```

The root Rust toolchain is pinned in `rust-toolchain.toml`. Generated Wasm payloads are committed so the browser application remains directly usable offline.

---

## 7. Documentation map

| Document | Purpose |
| --- | --- |
| [ENGINE.md](docs/ENGINE.md) | Browser engine, logical/rendered population separation, Wasm/WebGL path |
| [BARNES-HUT.md](docs/BARNES-HUT.md) | Resident self-gravity contract, direct-force oracle, flat-tree and GPU evolution/tree-build boundaries |
| [UFF-DYNAMICS.md](docs/UFF-DYNAMICS.md) | Rotation-law physics, units, demonstration-data provenance |
| [GPU-RUNTIME.md](docs/GPU-RUNTIME.md) | Native Rust/`wgpu` runtime, GPU execution, Barnes–Hut traversal/evolution/tree construction |
| [CUDA-RUNTIME.md](docs/CUDA-RUNTIME.md) | CUDA backend, bootstrap, validation and claim boundaries |
| [U64-TILED-RUNTIME.md](docs/U64-TILED-RUNTIME.md) | Memory-bounded logical populations beyond 32-bit indexing |
| [CPU-RUNTIME.md](docs/CPU-RUNTIME.md) | Native CPU architecture, timing, receipts, correctness contract |
| [CPU-PORTABILITY.md](docs/CPU-PORTABILITY.md) | Cross-platform CPU validation and portability evidence |
| [RETRO-MATH.md](docs/RETRO-MATH.md) | BAM32, CORDIC, fixed-point and retro-computing reference math |
| [QBRAID-CPU-SCALING.md](docs/QBRAID-CPU-SCALING.md) | qBraid protocol plus completed EPYC 7763 scaling study |
| [QBRAID.md](QBRAID.md) | qBraid execution index for CPU and GPU studies |
| [NOTICE.md](NOTICE.md) | Attribution, provenance, and file-level licensing |

---

## 8. Roadmap

The v0.4.0 → v0.6.0 CPU architecture ladder is frozen.

**v0.7.0 freezes the Barnes–Hut correctness ladder through BH #2C:**

```text
PR #18  BH #1   resident Barnes–Hut CPU reference
PR #19  BH #2A  Morton / flat-tree substrate
PR #20  BH #2B1 GPU transfer ABI + force traversal
PR #21  BH #2B2 persistent multi-step GPU evolution
PR #22  BH #2C  GPU tree construction
```

The next experimental rung is **BH #2D — parallel GPU tree construction**.

BH #2C proves that bounds, Morton generation, deterministic spatial ordering, flat topology, bottom-up aggregates, traversal, and multi-step integration can remain on the device without per-step host particle readback or CPU tree reconstruction.

BH #2D can now optimize that verified device-owned boundary:

```text
parallel bounds reduction
  -> scalable radix / Morton ordering
  -> parallel range and topology construction
  -> parallel cell aggregates
  -> real hardware GPU measurements
```

The frozen correctness oracles remain:

- BH #1 direct-force and recursive CPU reference;
- BH #2A deterministic f64 flat-tree implementation;
- BH #2B2 host-built GPU evolution;
- BH #2C correctness-first GPU-built tree.

Performance promotion should occur only after the parallel alternatives reproduce the established force, trajectory, deterministic-ordering, and receipt evidence.

The historical native-CPU investigation areas were:

1. **persistent worker pools** — remove repeated worker construction from timed CPU execution;
2. **topology-aware scheduling** — make placement policy explicit rather than inferred from worker count;
3. **NUMA-aware placement** — test memory locality without assuming one universal affinity strategy;
4. **receipt-native affinity evidence** — record allowed CPU sets and topology evidence with the run itself;
5. **before/after validation** — compare against the frozen v0.4.0 Ryzen and EPYC baseline while requiring deterministic checksum parity.

The default scheduling policy should remain conservative until alternatives are measured. The qBraid result shows that “more distinct physical cores” is not automatically equivalent to “faster” for this workload.

---

## 9. Provenance and licensing

GALAXY has file-level licensing rather than one blanket licence for every source file.

- Adapted VORTEX browser sources retain **MPL-2.0** notices.
- New Rust sampler, native CPU/GPU runtimes, Barnes–Hut implementations, build tooling, and associated Apache-licensed project code use **Apache-2.0**.
- QSOL UFF-derived implementation/data provenance is pinned and documented in [`data/uff/provenance.json`](data/uff/provenance.json) and [NOTICE.md](NOTICE.md).

The VORTEX reference photographs, artwork, and historical stress-test reports are not redistributed. Historical VORTEX measurements are not GALAXY benchmark evidence.

Copyright 2025–2026 Trent Slade / QSOL-IMC.

---

## Citation

If you use the v0.4.0 software/evidence baseline, cite:

> Slade, T. (2026). *GALAXY v0.4.0: Deterministic Native CPU Runtime and Scaling Evidence* (Version v0.4.0) [Computer software]. Zenodo. https://doi.org/10.5281/zenodo.22756969

For the current software release, use:

> Slade, T. (2026). *GALAXY v0.7.0: Barnes–Hut Self-Gravity and GPU Tree Construction* (Version v0.7.0) [Computer software]. GitHub. https://github.com/QSOLKCB/GALAXY/releases/tag/v0.7.0
