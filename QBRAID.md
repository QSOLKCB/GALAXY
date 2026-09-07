# QBRAID.md

## Purpose

This file is an execution handoff for an AI agent running inside a qBraid GPU environment.

The mission is to validate and benchmark GALAXY on real qBraid GPU hardware, with special focus on the experimental 64-bit tiled runtime that can evolve one deterministic logical particle population beyond the 32-bit index space while keeping GPU-resident memory bounded.

Do not treat this as a generic CUDA benchmark. GALAXY currently uses the Rust `wgpu` stack and requires a real hardware backend exposed through Vulkan on Linux. `nvidia-smi` alone is not sufficient proof that the required compute backend is available.

The scientific and reproducibility contract matters more than obtaining a fast number.

---

## Repository state

Repository:

```text
https://github.com/QSOLKCB/GALAXY
```

The 64-bit tiled runtime is currently developed on:

```text
upgrade/u64-tiled-runtime
```

Associated pull request:

```text
https://github.com/QSOLKCB/GALAXY/pull/4
```

Before doing any compute work, record the exact commit:

```bash
git status --short --branch
git rev-parse HEAD
git log -1 --oneline
```

Do not silently switch branches, modify scientific parameters, or change the runtime implementation merely to make a benchmark pass.

If the branch has already been merged, use the merged `main` commit and record that SHA instead.

---

## Read these files first

Read, in this order:

```text
README.md
docs/GPU-RUNTIME.md
docs/U64-TILED-RUNTIME.md
runtime/jobs/beyond-u32.json
runtime/src/bin/galaxy-u64.rs
runtime/src/bin/u64_kernels.wgsl
```

The existing stable runtime is `galaxy-runtime`.

The wider-address experimental runtime is `galaxy-u64`.

Do not confuse their contracts.

---

## Core scientific boundary

The tiled execution scheme is valid for the current native spin workload because the particles are independent test particles evolving in the same fixed potential.

The current tiled mode does **not** implement:

- pairwise stellar forces;
- evolving self-gravity;
- hydrodynamics;
- star formation;
- an evolving density field shared between tiles;
- cross-tile particle interactions.

Therefore each tile can be evolved independently without changing the present mathematical model.

Do not generalize this argument to future coupled-particle modes.

The GPU orbit state remains `float32`. 64-bit addressing increases the addressable logical population; it does not increase floating-point numerical precision.

---

# Mission

Run the following progression on a real qBraid GPU, preserving evidence at each stage:

1. identify the hardware and software environment;
2. prove GALAXY sees a real hardware compute adapter;
3. run the stable runtime verification suite;
4. prove that the 64-bit runtime distinguishes particle identities across `2^32`;
5. run a small multi-tile smoke workload;
6. validate the supplied `beyond-u32.json` job;
7. only after all prior gates pass, execute the full beyond-`2^32` workload;
8. preserve the receipt, resolved job, terminal log, hashes, and machine information;
9. report measured results without extrapolating beyond what was executed.

Do not skip directly to the expensive run.

---

# 1. Hardware and environment preflight

Run:

```bash
pwd
uname -a
cat /etc/os-release || true
nvidia-smi || true
```

Then inspect Vulkan availability:

```bash
command -v vulkaninfo || true
ls -la /usr/share/vulkan/icd.d/ 2>/dev/null || true
vulkaninfo --summary 2>&1 | tee qbraid-vulkaninfo.txt
```

A useful qBraid machine for the existing Linux runtime must expose an actual NVIDIA Vulkan physical device.

A CUDA-capable GPU that does not expose Vulkan is **not yet a valid GALAXY hardware target** for this runtime.

If Vulkan is absent, do not rewrite GALAXY to CUDA without explicit user authorization. Report the environment limitation instead.

Do not count `llvmpipe`, `lavapipe`, SwiftShader, or another software adapter as hardware validation.

---

# 2. Build the repository exactly as pinned

From the GALAXY repository root:

```bash
cargo build --manifest-path runtime/Cargo.toml --release --locked
```

Do not update dependency versions or regenerate the lockfile unless compilation is impossible for a clearly documented reason and the user explicitly approves the change.

The existing stable runtime should remain the default Cargo binary.

---

# 3. Confirm GALAXY sees the qBraid GPU

Run:

```bash
runtime/target/release/galaxy-runtime devices | tee qbraid-devices.json
```

Expected characteristics for the selected adapter:

```text
backend: Vulkan
device_type: DiscreteGpu or another real hardware GPU class
software: false
```

Record:

- GPU model;
- device index;
- driver;
- driver version;
- reported storage-buffer limits;
- VRAM from `nvidia-smi`;
- whether multiple GPUs are visible.

The current runtime uses one adapter per process. Do not claim multi-GPU execution unless the implementation has actually been changed and validated for it.

Select the intended GPU explicitly in later commands with `--adapter`.

---

# 4. Stable-runtime verification gate

Run on the selected real hardware adapter:

```bash
bash scripts/run-gpu.sh verify --adapter 0 | tee qbraid-stable-verify.txt
```

If the qBraid target GPU is not adapter `0`, use the actual index or an unambiguous model-name substring.

This verification must pass before the tiled runtime is trusted on the machine.

Do not add `--allow-software` for hardware validation.

A successful result should report:

```text
status: passed
gpu_checked: true
software: false
```

Preserve the complete output.

---

# 5. Prove the 2^32 boundary

Run the dedicated 64-bit boundary verifier on the same hardware:

```bash
bash scripts/run-u64.sh verify --adapter 0 | tee qbraid-u64-boundary.txt
```

The verifier is specifically intended to exercise identities around:

```text
4,294,967,295
4,294,967,296
4,294,967,297
```

The important claim is not merely that three hashes differ. The host and GPU addressing implementations must agree and the high 32-bit word must materially affect deterministic initialization.

Failure here invalidates any subsequent claim that GALAXY crossed the 32-bit particle-index boundary.

Do not proceed to the large job after a boundary-verification failure.

---

# 6. Multi-tile smoke run

Before spending meaningful GPU credits, create a small temporary job that crosses several resident tiles but completes quickly.

Example:

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
    "softening_kpc": 0.02
  }
}
JSON
```

Validate it:

```bash
bash scripts/run-u64.sh validate --job /tmp/qbraid-u64-smoke.json
```

Then execute it into a fresh output directory:

```bash
OUT="runs/qbraid-u64-smoke-$(date -u +%Y%m%dT%H%M%SZ)"
time bash scripts/run-u64.sh run \
  --adapter 0 \
  --job /tmp/qbraid-u64-smoke.json \
  --output "$OUT" 2>&1 | tee qbraid-u64-smoke.log
```

Require `status: complete` in its receipt before continuing.

Never reuse or overwrite an existing result directory.

---

# 7. Validate the supplied beyond-u32 workload

The canonical large job is:

```text
runtime/jobs/beyond-u32.json
```

It currently specifies:

```text
logical_particles = 4,303,355,904
tile_particles    = 8,388,608
tiles             = 513
steps             = 1,000
integrator        = leapfrog
```

This is deliberately one complete 8,388,608-particle resident tile beyond `2^32`.

The total requested leapfrog work is:

```text
4,303,355,904,000 particle-step updates
```

Validate before execution:

```bash
bash scripts/run-u64.sh validate \
  --job runtime/jobs/beyond-u32.json \
  | tee qbraid-beyond-u32-validate.txt
```

Do not modify this canonical job for the benchmark unless the user explicitly requests a different workload.

---

# 8. Full qBraid beyond-2^32 run

Only after every previous gate passes:

```bash
OUT="runs/qbraid-beyond-u32-$(date -u +%Y%m%dT%H%M%SZ)"

time bash scripts/run-u64.sh run \
  --adapter 0 \
  --job runtime/jobs/beyond-u32.json \
  --output "$OUT" 2>&1 | tee qbraid-beyond-u32.log
```

Do not terminate the run simply because GPU utilization varies during tile initialization, synchronization, sampling, or output work.

If the run fails, preserve the failed output directory and receipt. Do not delete failure evidence before understanding the error.

---

# 9. Evidence to preserve

At minimum retain:

```text
qbraid-vulkaninfo.txt
qbraid-devices.json
qbraid-stable-verify.txt
qbraid-u64-boundary.txt
qbraid-u64-smoke.log
qbraid-beyond-u32-validate.txt
qbraid-beyond-u32.log
```

And from the full run directory retain:

```text
job.json
receipt.json
all generated CSV/PNG artifacts
viewer.html
```

Also create a machine record:

```bash
{
  echo "UTC: $(date -u --iso-8601=seconds)"
  echo "COMMIT: $(git rev-parse HEAD)"
  echo
  uname -a
  echo
  cat /etc/os-release || true
  echo
  nvidia-smi || true
} > qbraid-machine.txt
```

Hash the evidence:

```bash
sha256sum \
  qbraid-machine.txt \
  qbraid-vulkaninfo.txt \
  qbraid-devices.json \
  qbraid-stable-verify.txt \
  qbraid-u64-boundary.txt \
  qbraid-beyond-u32-validate.txt \
  qbraid-beyond-u32.log \
  > qbraid-evidence.sha256
```

If additional evidence files exist, include them too.

---

# 10. What to report from receipt.json

For the completed full run, extract and report at least:

- exact GALAXY commit SHA;
- qBraid GPU model;
- backend and driver;
- `software` flag;
- logical particle count;
- resident tile particle count;
- tile count;
- integrator;
- integration steps;
- total particle-step updates;
- initialization wall time;
- synchronized integration/compute wall time;
- total execution wall time;
- measured particle-step throughput;
- sample count;
- maximum sampled angular-momentum drift if present;
- job SHA-256;
- runtime source SHA-256;
- final receipt status.

Prefer receipt timings over shell `time` when discussing GPU compute throughput. Shell time is still useful as end-to-end timing.

Do not infer FLOP/s from particle-step throughput unless an explicit, reviewed operation count for the kernel is supplied.

---

# 11. Optional performance comparison

If budget permits, compare the qBraid result against the known local RTX 5060 Ti baseline using identical workload definitions.

Known local evidence established approximately:

```text
RTX 5060 Ti
8,388,608 resident particles
10,000 leapfrog steps
83,886,080,000 particle-step updates
~13.85 s synchronized GPU compute
~6.05 billion particle-step updates/s
```

Treat this as a comparison baseline, not as a qBraid acceptance threshold.

Different GPUs, drivers, and transcendental implementations may produce small float32 differences.

Reproducibility here means identified inputs, deterministic addressing, bounded numerical tolerance, explicit provenance, and preserved receipts. It does not imply bit-identical results across unrelated GPU architectures.

---

# 12. Cost discipline

The user is spending finite qBraid hardware credits.

Therefore:

- perform all cheap validation before the full benchmark;
- do not rebuild repeatedly without a reason;
- reuse the Cargo build cache within the same instance;
- do not launch duplicate monster jobs concurrently;
- do not leave an expensive GPU instance idle after evidence has been collected;
- do not start a second large benchmark merely for curiosity without asking the user.

Scientific evidence per credit is the objective, not maximum credit consumption.

---

# 13. Failure policy

If any stage fails:

1. preserve the exact command and complete error output;
2. preserve any generated receipt or partial artifacts;
3. identify whether the failure is environment, Vulkan exposure, adapter limits, compilation, validation, numerical, or runtime-related;
4. prefer the smallest corrective action;
5. rerun the failed gate before proceeding;
6. do not weaken validation to turn a failure green.

Specifically prohibited shortcuts:

- silently falling back to CPU;
- passing `--allow-software` and presenting it as hardware execution;
- reducing the logical particle count and still calling it the beyond-`2^32` benchmark;
- changing the seed or physics without recording it;
- using CUDA visibility alone as proof of GALAXY GPU execution;
- deleting failed receipts;
- claiming multi-GPU execution when one adapter was used;
- claiming N-body dynamics or self-consistent galaxy evolution.

---

# 14. If qBraid exposes CUDA but not Vulkan

Stop before the expensive run and report:

```text
GPU hardware is visible through CUDA/NVML, but the current GALAXY Linux runtime cannot see a hardware Vulkan adapter.
```

Collect:

```bash
nvidia-smi
ls -la /usr/share/vulkan/icd.d/ 2>/dev/null || true
vulkaninfo --summary 2>&1 || true
runtime/target/release/galaxy-runtime devices 2>&1 || true
```

Do not implement a CUDA backend as an unrequested workaround.

A CUDA port would be a separate engineering change requiring review and numerical equivalence testing against the existing Rust/WGSL implementation.

---

# 15. If multiple beastly GPUs are available

Choose one explicit GPU first and establish a single-device baseline.

Prefer a datacenter accelerator with strong FP32 throughput and a healthy driver stack, but note that the current tile size uses only a bounded fraction of large VRAM capacities.

For this workload, raw compute throughput may matter more than additional memory once the resident tile comfortably fits.

Do not assume an H100/H200/B200 will automatically outperform every other option without measuring the actual GALAXY kernel.

The current runtime does not automatically partition one logical job across multiple GPUs.

A future multi-GPU mode should preserve the same global-ID and receipt semantics and assign disjoint tile ranges explicitly.

---

# 16. Definition of success

The strongest successful qBraid result is:

1. a real qBraid hardware GPU is selected with `software: false`;
2. stable GALAXY GPU verification passes;
3. the dedicated `2^32` boundary verifier passes on that GPU;
4. the multi-tile smoke job completes;
5. `runtime/jobs/beyond-u32.json` validates unchanged;
6. the full run completes with:

```text
logical_particles = 4,303,355,904
particle_updates   = 4,303,355,904,000
status             = complete
```

7. the receipt and all evidence hashes are preserved.

Only then is it appropriate to state that the selected qBraid GPU executed GALAXY's deterministic tiled population beyond the 32-bit particle-index space.

---

## Final instruction to the qBraid AI agent

Be aggressive with the hardware, conservative with the claims, and meticulous with the evidence.

The purpose of the beastly machine is not to make GALAXY look impressive. It is to find out exactly what GALAXY can do.