# GALAXY CUDA backend

GALAXY's 64-bit tiled spin runtime has two hardware execution paths on Linux:

- **Vulkan / `wgpu`** — the original Rust `galaxy-u64` binary.
- **CUDA / CuPy RawKernel** — `runtime/cuda/galaxy_u64_cuda.py` plus `runtime/cuda/galaxy_u64_cuda_impl.py`.

The CUDA path exists for NVIDIA cloud sessions that expose CUDA/NVML but do not expose a Vulkan ICD. This is common in compute-oriented containers and managed notebook GPU environments. It does **not** silently emulate Vulkan and it does not treat `nvidia-smi` alone as proof that a GALAXY kernel executed.

## Scope

The CUDA backend implements the current `galaxy-u64` **spin** contract:

- the same `schema_version = 1` job JSON;
- the same bounded resident tile maximum of 8,388,608 particles;
- the same deterministic `split-u64-hash32-avalanche-v1` address mixer;
- the same full logical `u64` particle IDs and exact host sample mapping;
- the same circular and leapfrog equations used by `u64_kernels.wgsl`;
- float32 particle state and float64 host diagnostics;
- new output directories, CSV/PNG snapshots, `viewer.html`, `job.json`, and `receipt.json`;
- explicit adapter identity, backend, timing, source hash, job hash, and artifact hashes.

It does not add self-gravity, pairwise particle forces, hydrodynamics, star formation, cross-tile communication, or multi-GPU scheduling.

The stable `galaxy-runtime` curve/compact-object workload remains on the Rust/`wgpu` path. CUDA support is intentionally introduced first for the >`u32` tiled spin workload that needs cloud accelerator portability.

## Why CuPy RawKernel

The target problem is not merely "install Vulkan." Some hosted NVIDIA sessions expose the CUDA driver to the notebook/container while withholding graphics-driver capabilities and the Vulkan ICD.

CuPy can JIT the checked-in CUDA kernel source with NVRTC. With CuPy 14's CUDA toolkit extras, the target machine can supply only:

- Python 3;
- an NVIDIA driver visible to the session;
- a supported CUDA 12.x or 13.x GPU.

Rust, Cargo, `nvcc`, Vulkan, and `vulkaninfo` are not required to execute the CUDA backend.

The repository pins the bootstrap to CuPy 14.2.0. `scripts/bootstrap-cuda.sh` installs either `cupy-cuda12x[ctk]` or `cupy-cuda13x[ctk]` into a repository-local `.galaxy-cuda-python/` directory according to the CUDA version advertised by `nvidia-smi`.

## Bootstrap

On a CUDA-only qBraid/Vast.ai-style machine:

```bash
nvidia-smi
bash scripts/bootstrap-cuda.sh
```

The bootstrap prints CuPy, CUDA driver/runtime versions, visible devices, and VRAM.

No root access is required for the Python package installation.

## Backend selection

`scripts/run-u64.sh` accepts:

```text
--backend auto
--backend vulkan
--backend cuda
```

`auto` is the default.

On Linux, `auto` prefers the existing Rust/Vulkan runtime only when Cargo is available and a real hardware Vulkan adapter is successfully probed. If `--adapter` is supplied, that adapter query is included in the Vulkan probe; an unrelated Vulkan-capable device cannot cause `auto` to reject a requested CUDA-only adapter. Otherwise the router selects CUDA. Backend selection is printed to stderr and receipts distinguish the requested mode from the selected backend.

You can force a backend:

```bash
bash scripts/run-u64.sh devices --backend cuda
bash scripts/run-u64.sh devices --backend vulkan
```

The environment equivalent is:

```bash
GALAXY_BACKEND=cuda bash scripts/run-u64.sh devices
```

## CUDA verification

Host-only address discriminator:

```bash
bash scripts/run-u64.sh verify --backend cuda --cpu
```

Hardware boundary verification:

```bash
bash scripts/run-u64.sh verify --backend cuda --adapter 0
```

The hardware verifier initializes the three IDs around the 32-bit wrap boundary:

```text
4294967295
4294967296
4294967297
```

and compares CUDA initialization against the host reference within the same bounded numerical tolerance used by the Rust tiled verifier.

A successful CUDA hardware result reports:

```text
status: passed
backend: CUDA
gpu_boundary_checked: true
software: false
```

## Run the canonical beyond-u32 job

Validate first:

```bash
bash scripts/run-u64.sh validate \
  --backend cuda \
  --job runtime/jobs/beyond-u32.json
```

Then use a fresh directory:

```bash
OUT="runs/cuda-beyond-u32-$(date -u +%Y%m%dT%H%M%SZ)"
time bash scripts/run-u64.sh run \
  --backend cuda \
  --adapter 0 \
  --job runtime/jobs/beyond-u32.json \
  --output "$OUT"
```

The canonical workload remains:

```text
logical_particles = 4,303,355,904
tile_particles    = 8,388,608
tiles             = 513
steps             = 1,000
particle_updates  = 4,303,355,904,000
```

Do not reduce those values and continue calling the result the canonical beyond-`2^32` run.

## CUDA execution schedule

For leapfrog jobs, the CUDA kernel fuses the requested repeated steps inside each independent particle thread for a tile/frame interval rather than launching one CUDA kernel per step.

That transformation is valid only because the current tiled model has no particle-particle or tile-coupled state. Each thread still performs the same ordered float32 kick-drift-kick updates for its own particle. The receipt records this schedule explicitly.

This is an execution optimization, not a new physical approximation. If coupled-particle physics is introduced later, the fused schedule must be re-reviewed.

## Numerical equivalence

Vulkan and CUDA are expected to agree within the documented float32 tolerance, not bit-for-bit across unrelated GPU architectures and math implementations.

For a new CUDA machine, require in order:

1. `nvidia-smi` sees the intended GPU;
2. `devices --backend cuda` reports `software: false`;
3. `verify --backend cuda --adapter ...` passes the `2^32` boundary check;
4. a small multi-tile smoke job completes;
5. only then run the canonical large workload.

Do not use a CPU result as a hardware benchmark and do not relabel a CUDA receipt as Vulkan.

## Vast.ai note

If the container is launched with NVIDIA graphics capability, the existing Vulkan backend can continue to work:

```text
NVIDIA_DRIVER_CAPABILITIES=graphics,utility,compute
```

If the selected Vast.ai image/provider path exposes only CUDA compute, use the CUDA backend instead of manufacturing or guessing a Vulkan ICD.

## Provenance

CUDA receipts identify:

- `"runtime": "galaxy-u64-cuda"`;
- `"backend_requested": "cuda"` for an explicitly forced CUDA run, or `"backend_requested": "auto"` when automatic routing selected CUDA;
- `"backend_selected": "cuda"`;
- adapter `"backend": "CUDA"`;
- CUDA driver and runtime versions;
- compute capability and VRAM;
- `runtime_source_sha256` over the CUDA entrypoint, implementation, pinned UFF data/provenance, bootstrap definition, and backend router;
- resolved `job_sha256`;
- artifact hashes;
- initialization and integration kernel timings.

Vulkan runs launched through `scripts/run-u64.sh` likewise preserve `backend_requested` as `auto` or `vulkan` and record `backend_selected: "vulkan"`. The Rust `runtime_source_sha256` includes the wrapper because the wrapper can materially rewrite that provenance evidence.

This keeps Vulkan and CUDA benchmark evidence comparable without pretending they are the same implementation.
