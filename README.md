# GALAXY

[![Release](https://img.shields.io/badge/release-v0.4.0-2f81f7)](https://github.com/QSOLKCB/GALAXY/releases/tag/v0.4.0)
[![DOI](https://zenodo.org/badge/DOI/10.5281/zenodo.22756969.svg)](https://doi.org/10.5281/zenodo.22756969)

**GALAXY is an offline deterministic galaxy-dynamics instrument with browser, native CPU, and native GPU execution paths.**

It began as an adaptation of the VORTEX 2.1.0 particle lab and now combines:

- interactive rotation-curve visualization;
- UFF, Newtonian, NFW, Burkert, MOND/RAR, and authored visual rotation laws;
- deterministic Rust/WebAssembly sampling in the browser;
- native Rust CPU execution with Float/libm and BAM32/Q2.30 LUT backends;
- Vulkan/`wgpu` and NVIDIA CUDA compute paths;
- memory-bounded exact-u64 logical addressing;
- reproducible benchmark receipts, topology evidence, and archived scaling studies.

The current formal baseline is **v0.4.0**, archived at Zenodo as:

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
| Formal release | Immutable `v0.4.0`, commit `6f17a734b9241359d36a9bf3d208b8527a456327` |
| Archival record | Zenodo DOI `10.5281/zenodo.22756969` |
| Next CPU phase | Persistent workers, topology-aware scheduling, NUMA-aware placement, stronger receipt-native affinity provenance |

v0.4.0 is deliberately frozen as the **before-state** for that next CPU-runtime architecture phase.

---

## 1. Browser instrument

Open **`index.html`** directly in a modern browser. No install, local server, CDN, or network connection is required, including for the bundled Rust/WebAssembly engine.

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

See:

- [GPU runtime](docs/GPU-RUNTIME.md)
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

GALAXY is a **deterministic galaxy dynamics and visualization instrument**, not a self-consistent N-body code.

The browser instrument and native runtimes use prescribed rotation laws / gravitational potentials. The native runtimes evolve independent test particles. GALAXY does **not** currently implement:

- pairwise stellar forces;
- evolving self-gravity;
- hydrodynamics;
- gas evolution;
- star formation;
- a self-consistent evolving density field;
- cross-particle force coupling.

Large logical populations are deterministic address spaces from which bounded resident populations are sampled or tiled. A logical population of `u64::MAX` does **not** mean that 18.4 quintillion particles are simultaneously resident in memory.

Performance claims are similarly bounded: benchmark receipts establish behavior for the recorded source, workload, and environment. They do not establish universal Ryzen, EPYC, cloud, CPU-vs-GPU, NUMA, or memory-bandwidth claims.

---

## 6. Development and verification

The repository contains four main validation surfaces:

```text
browser / Wasm          -> JS application + physics + packaging checks
retro integer math      -> Rust + JS portability / vector checks
native CPU runtime      -> Linux / macOS / Windows correctness and receipts
native GPU / u64        -> Vulkan/CUDA host contracts and tiled-runtime checks
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
| [UFF-DYNAMICS.md](docs/UFF-DYNAMICS.md) | Rotation-law physics, units, demonstration-data provenance |
| [GPU-RUNTIME.md](docs/GPU-RUNTIME.md) | Native Rust/`wgpu` runtime and GPU execution |
| [CUDA-RUNTIME.md](docs/CUDA-RUNTIME.md) | CUDA backend, bootstrap, validation and claim boundaries |
| [U64-TILED-RUNTIME.md](docs/U64-TILED-RUNTIME.md) | Memory-bounded logical populations beyond 32-bit indexing |
| [CPU-RUNTIME.md](docs/CPU-RUNTIME.md) | Native CPU architecture, timing, receipts, correctness contract |
| [CPU-PORTABILITY.md](docs/CPU-PORTABILITY.md) | Cross-platform CPU validation and portability evidence |
| [RETRO-MATH.md](docs/RETRO-MATH.md) | BAM32, CORDIC, fixed-point and retro-computing reference math |
| [QBRAID-CPU-SCALING.md](docs/QBRAID-CPU-SCALING.md) | qBraid protocol plus completed EPYC 7763 scaling study |
| [QBRAID.md](QBRAID.md) | qBraid execution index for CPU and GPU studies |
| [NOTICE.md](NOTICE.md) | Attribution, provenance, and file-level licensing |

---

## 8. Roadmap from the v0.4.0 baseline

The next native-CPU phase is intentionally architectural rather than another uncontrolled worker-count increase.

Planned investigation areas are:

1. **persistent worker pools** — remove repeated worker construction from timed CPU execution;
2. **topology-aware scheduling** — make placement policy explicit rather than inferred from worker count;
3. **NUMA-aware placement** — test memory locality without assuming one universal affinity strategy;
4. **receipt-native affinity evidence** — record allowed CPU sets and topology evidence with the run itself;
5. **before/after validation** — compare against the frozen v0.4.0 Ryzen and EPYC baseline while requiring deterministic checksum parity.

The default scheduling policy should remain conservative until those alternatives are measured. The qBraid result shows that “more distinct physical cores” is not automatically equivalent to “faster” for this workload.

---

## 9. Provenance and licensing

GALAXY has file-level licensing rather than one blanket licence for every source file.

- Adapted VORTEX browser sources retain **MPL-2.0** notices.
- New Rust sampler, native CPU/GPU runtimes, build tooling, and associated Apache-licensed project code use **Apache-2.0**.
- QSOL UFF-derived implementation/data provenance is pinned and documented in [`data/uff/provenance.json`](data/uff/provenance.json) and [NOTICE.md](NOTICE.md).

The VORTEX reference photographs, artwork, and historical stress-test reports are not redistributed. Historical VORTEX measurements are not GALAXY benchmark evidence.

Copyright 2025–2026 Trent Slade / QSOL-IMC.

---

## Citation

If you use the v0.4.0 software/evidence baseline, cite:

> Slade, T. (2026). *GALAXY v0.4.0: Deterministic Native CPU Runtime and Scaling Evidence* (Version v0.4.0) [Computer software]. Zenodo. https://doi.org/10.5281/zenodo.22756969
