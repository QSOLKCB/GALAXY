# QBRAID.md

## Purpose

This is the execution handoff for an AI agent running GALAXY on qBraid GPU hardware.

GALAXY's memory-bounded `galaxy-u64` workload now has two explicit NVIDIA-capable Linux paths:

- **Vulkan / Rust `wgpu`** when a real hardware Vulkan adapter and Cargo are available;
- **CUDA / CuPy RawKernel** when the session exposes CUDA/NVML but not Vulkan or Cargo.

The scientific and reproducibility contract is the same on both paths. Never silently fall back to CPU, never relabel one backend as the other, and never treat `nvidia-smi` alone as proof that a GALAXY kernel executed.

The CUDA backend was added specifically so a qBraid L4-style compute session can be a valid GALAXY target even when the managed environment withholds a Vulkan ICD.

---

## Repository state

Repository:

```text
https://github.com/QSOLKCB/GALAXY
```

Before compute work, record the exact revision:

```bash
git status --short --branch
git rev-parse HEAD
git log -1 --oneline
```

Do not silently switch branches, alter the scientific parameters, or reduce the canonical large job while continuing to describe it as the same benchmark.

---

## Read first

```text
README.md
docs/U64-TILED-RUNTIME.md
docs/CUDA-RUNTIME.md
runtime/jobs/beyond-u32.json
runtime/src/bin/galaxy-u64.rs
runtime/src/bin/u64_kernels.wgsl
runtime/cuda/galaxy_u64_cuda.py
scripts/run-u64.sh
```

The stable `galaxy-runtime` remains the Rust/`wgpu` runtime. The dual-backend work in this handoff applies first to the wider-address `galaxy-u64` spin workload.

---

## Scientific boundary

Tiling is valid for the present workload because particles are independent test particles evolving in one fixed UFF-derived potential.

The tiled mode does **not** implement:

- pairwise stellar forces;
- evolving self-gravity;
- hydrodynamics;
- star formation;
- an evolving density field shared between tiles;
- cross-tile particle interactions.

Tiles are therefore an execution partition, not an additional approximation to the current equations.

The particle/orbit state remains `float32`. The wider address space increases the logical population range; it does not increase numerical precision.

---

# 1. Hardware and backend preflight

Run:

```bash
pwd
uname -a
cat /etc/os-release || true
python3 --version || true
command -v cargo || true
nvidia-smi || true
command -v vulkaninfo || true
ls -la /usr/share/vulkan/icd.d/ 2>/dev/null || true
vulkaninfo --summary 2>&1 | tee qbraid-vulkaninfo.txt || true
```

Choose the backend from actual capabilities:

### Vulkan path

Use Vulkan only if GALAXY can see a real hardware Vulkan device. Do not count llvmpipe, lavapipe, SwiftShader, or another software adapter as hardware validation.

### CUDA path

If the NVIDIA GPU is visible through CUDA/NVML but Vulkan or Cargo is absent, **do not stop**. The CUDA backend is an authorized GALAXY hardware path for `galaxy-u64`.

Bootstrap the pinned runtime dependency once:

```bash
bash scripts/bootstrap-cuda.sh
```

This installs the supported CuPy CUDA toolkit wheel into the repository-local `.galaxy-cuda-python/` directory. It does not require root, Rust, Cargo, `nvcc`, Vulkan, or `vulkaninfo`.

GALAXY currently maps only CUDA 12.x and CUDA 13.x to pinned CuPy packages. If the driver advertises another major version, stop and report it rather than guessing a wheel.

---

# 2. Confirm the intended GPU

For a CUDA-only qBraid session:

```bash
bash scripts/run-u64.sh devices --backend cuda | tee qbraid-cuda-devices.json
```

For Vulkan:

```bash
bash scripts/run-u64.sh devices --backend vulkan | tee qbraid-vulkan-devices.json
```

`--backend auto` is available, but for benchmark evidence prefer an explicit backend.

A valid CUDA hardware entry should identify:

```text
backend: CUDA
software: false
```

Record the GPU model, device index, driver/runtime versions, compute capability, VRAM, and whether multiple devices are visible.

The current runtime uses one GPU per process. Do not claim multi-GPU execution.

---

# 3. Host contract gate

Before spending GPU credits, validate the exact addressing and job contract:

```bash
python3 tests/test_cuda_u64.py
bash scripts/run-u64.sh verify --backend cuda --cpu | tee qbraid-u64-host-boundary.txt
bash scripts/run-u64.sh validate --backend cuda \
  --job runtime/jobs/beyond-u32.json \
  | tee qbraid-beyond-u32-validate.txt
```

The canonical large job must resolve to:

```text
logical_particles = 4,303,355,904
tile_particles    = 8,388,608
tiles             = 513
steps             = 1,000
particle_updates  = 4,303,355,904,000
```

---

# 4. Hardware 2^32 boundary proof

On CUDA:

```bash
bash scripts/run-u64.sh verify \
  --backend cuda \
  --adapter 0 \
  | tee qbraid-u64-boundary.txt
```

The verifier exercises:

```text
4,294,967,295
4,294,967,296
4,294,967,297
```

Success requires distinct deterministic addresses and CUDA initialization agreeing with the host reference within the documented float32 tolerance.

Require:

```text
status: passed
backend: CUDA
gpu_boundary_checked: true
software: false
```

Failure here invalidates any subsequent claim that the CUDA runtime crossed the 32-bit particle-index boundary.

---

# 5. Multi-tile CUDA smoke run

Create a cheap smoke job before the expensive benchmark:

```bash
cat > /tmp/qbraid-u64-smoke.json <<'JSON'
{
  "schema_version": 1,
  "task": {
    "physics": {
      "model": "nfw",
      "black_hole_million": 4.3
    },
    "integrator": "leapfrog",
    "logical_particles": 196609,
    "tile_particles": 65536,
    "seed": 303,
    "steps": 16,
    "dt_myr": 0.05,
    "snapshot_every": 8,
    "snapshot_limit": 1024,
    "radial_kick_kms": 20.0,
    "softening_kpc": 0.02,
    "image_size": 128
  }
}
JSON

bash scripts/run-u64.sh validate --backend cuda --job /tmp/qbraid-u64-smoke.json
OUT="runs/qbraid-u64-smoke-$(date -u +%Y%m%dT%H%M%SZ)"
time bash scripts/run-u64.sh run \
  --backend cuda \
  --adapter 0 \
  --job /tmp/qbraid-u64-smoke.json \
  --output "$OUT" 2>&1 | tee qbraid-u64-smoke.log
```

Require `status: complete` in its receipt before continuing.

Never reuse or overwrite an existing result directory.

---

# 6. Full beyond-2^32 run

Only after the host gate, hardware boundary verifier, and smoke run pass:

```bash
OUT="runs/qbraid-beyond-u32-$(date -u +%Y%m%dT%H%M%SZ)"
time bash scripts/run-u64.sh run \
  --backend cuda \
  --adapter 0 \
  --job runtime/jobs/beyond-u32.json \
  --output "$OUT" 2>&1 | tee qbraid-beyond-u32.log
```

Do not terminate a valid run merely because GPU utilization varies during initialization, sampling, synchronization, or output work.

If it fails, preserve the failed output directory and receipt.

---

# 7. Evidence to preserve

At minimum retain:

```text
qbraid-machine.txt
qbraid-vulkaninfo.txt
qbraid-cuda-devices.json
qbraid-u64-host-boundary.txt
qbraid-u64-boundary.txt
qbraid-u64-smoke.log
qbraid-beyond-u32-validate.txt
qbraid-beyond-u32.log
```

And from the completed run directory:

```text
job.json
receipt.json
all generated CSV/PNG artifacts
viewer.html
```

Create the machine record:

```bash
{
  echo "UTC: $(date -u --iso-8601=seconds)"
  echo "COMMIT: $(git rev-parse HEAD)"
  echo
  uname -a
  echo
  cat /etc/os-release || true
  echo
  python3 --version || true
  echo
  nvidia-smi || true
} > qbraid-machine.txt
```

Hash the evidence you actually generated.

---

# 8. What the receipt must establish

For a CUDA run, report at least:

- exact GALAXY commit SHA recorded separately in the machine evidence;
- GPU model;
- backend `CUDA`;
- `software: false`;
- CUDA driver/runtime and compute capability;
- logical particle count;
- resident tile particle count;
- tile count;
- integrator and integration steps;
- total particle-step updates;
- initialization kernel time;
- integration kernel time;
- total execution wall time;
- measured particle-step throughput;
- job SHA-256;
- runtime source SHA-256;
- final receipt status.

Prefer receipt kernel timings over shell `time` when discussing GPU compute throughput. Shell time remains useful for end-to-end timing.

Do not infer FLOP/s without a separately reviewed operation count.

---

# 9. Vulkan versus CUDA comparisons

The two backends use the same logical job and address contract but are separate implementations.

Do not expect bit-identical float32 trajectories across unrelated GPU architectures or math libraries. Require bounded numerical agreement and preserve the backend identity in every comparison.

The CUDA leapfrog kernel may fuse multiple ordered per-particle steps inside one CUDA thread for a frame interval. This is valid for the current independent-particle fixed-potential model and is recorded in the receipt. It must be re-reviewed if future physics introduces particle coupling.

---

# 10. Cost discipline

The user is spending finite GPU credits.

Therefore:

- run host checks before GPU checks;
- bootstrap CUDA dependencies once per environment;
- run the boundary proof before the smoke job;
- run the smoke job before the canonical workload;
- do not launch duplicate large jobs;
- do not leave an expensive GPU instance idle after evidence is collected;
- do not start extra comparison runs merely for curiosity.

Scientific evidence per credit is the objective.

---

# 11. Failure policy

If a stage fails:

1. preserve the exact command and complete error output;
2. preserve any generated receipt or partial artifacts;
3. classify the failure as environment, dependency/bootstrap, CUDA visibility, Vulkan visibility, compilation/JIT, validation, numerical, memory, or runtime-related;
4. prefer the smallest corrective action;
5. rerun the failed gate before proceeding;
6. never weaken validation just to turn a failure green.

Specifically prohibited shortcuts:

- silently falling back to CPU;
- presenting software Vulkan as hardware execution;
- using `nvidia-smi` alone as proof GALAXY ran on CUDA;
- reducing the canonical logical population and still calling it the beyond-`2^32` benchmark;
- changing seed or physics without recording it;
- relabelling CUDA as Vulkan or Vulkan as CUDA;
- deleting failed receipts;
- claiming multi-GPU execution when one adapter was used;
- claiming N-body dynamics or self-consistent galaxy evolution.

---

## Definition of success on a CUDA-only qBraid L4

A strong successful result is:

1. `nvidia-smi` identifies the intended L4;
2. `devices --backend cuda` reports the L4 with `backend: CUDA` and `software: false`;
3. the host u64 contract tests pass;
4. the CUDA `2^32` boundary verifier passes;
5. the multi-tile smoke run completes;
6. `runtime/jobs/beyond-u32.json` validates unchanged;
7. the full canonical run completes with:

```text
logical_particles = 4,303,355,904
particle_updates   = 4,303,355,904,000
status             = complete
```

8. receipt and evidence hashes are preserved.

Only then state that the selected qBraid GPU executed GALAXY's deterministic tiled population beyond the 32-bit particle-index space.

Be aggressive with the hardware, conservative with the claims, and meticulous with the evidence.
