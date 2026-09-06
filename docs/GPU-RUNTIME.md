# Native GPU runtime

`runtime/` is a separate Rust v0.3 runtime for headless local and cloud compute.
It reuses the browser engine's pinned UFF equations and adds native GPU kernels,
job files, batch outputs and runners. The browser application and its offline
Wasm crate keep their existing interface and build.

## What runs

| Job | Computation | Output |
| --- | --- | --- |
| `spin`, `circular` | All particles initialized on the GPU; analytic UFF circular orbits evaluated at requested snapshot times | PNG frames, sampled position/velocity CSVs, offline `viewer.html` |
| `spin`, `leapfrog` | All particles stepped through a fixed planar UFF force field, with optional radial kicks | Same snapshots, with sampled angular-momentum drift |
| `curves` | Batched circular-speed evaluations over models, radii and one optional parameter sweep | `rotation-curves.csv`, acceleration and orbital period |
| `compact` | Batched signed-spin Kerr horizon, photon-orbit and ISCO radii | `compact-objects.csv`, plus CPU influence-radius and LQG scale bookkeeping |

Each run writes `job.json` with resolved defaults and `receipt.json` with its
status, adapter, arithmetic, source fingerprint, job hash and output hashes.
Only a successful run has `status: complete`. Failed runs return a nonzero exit
status and retain the receipt and any completed outputs. Output directories must
be new; an earlier run is never overwritten. Process kills can leave a `running`
receipt and should be treated as interrupted, not complete.
Artifact enumeration and hashing are part of finalization: a detected failure
records `status: failed`, the error and any completed simulation details.

## Local quick start

Install Rust through [rustup](https://rustup.rs/) and a working native GPU driver.
The repository pins Rust 1.85.1. Linux uses Vulkan; Windows can use Vulkan or
D3D12, and macOS uses Metal through [wgpu](https://github.com/gfx-rs/wgpu/tree/v24.0.5).
The Linux path is the cloud target. Windows/Metal hardware execution has not been
validated by the Linux CI gate.

On Ubuntu, with the GPU vendor driver already installed:

```bash
sudo apt-get update
sudo apt-get install build-essential libvulkan1 vulkan-tools pkg-config
```

From the GALAXY repository root:

```bash
bash scripts/run-gpu.sh devices
bash scripts/run-gpu.sh verify
bash scripts/run-gpu.sh run --job runtime/jobs/spin-local.json --output runs/local-spin
```

The script builds the release binary with the committed lockfile. The first
build downloads its dependencies. Later runs reuse the build. For direct use:

```bash
cargo build --manifest-path runtime/Cargo.toml --release --locked
runtime/target/release/galaxy-runtime --help
runtime/target/release/galaxy-runtime validate --job runtime/jobs/perturbed-orbits.json
```

Open `runs/local-spin/viewer.html` locally to play the exported snapshots.
The simulation does not require a window system, browser, CUDA toolkit or Python.
The SSH runner requires Python 3.12+ on the machine launching it.

Use `--adapter 1` to select the index listed by `devices`, or `--adapter NVIDIA`
to select the first matching name. An existing index takes precedence; other
values match names, so `--adapter 4090` selects a matching RTX 4090.
A process uses one adapter; separate processes
can target different indices, including cards with identical names. No automatic
multi-GPU partitioning or cross-device synchronization is implemented.

`--cpu` explicitly selects the native reference. Missing GPU support is an error;
the runtime never silently reports a CPU job as GPU work. `--allow-software`
permits a software adapter such as Mesa llvmpipe for verification and records it
as software in the receipt. It is not a hardware-performance measurement.

## Ready-to-run jobs

| File in `runtime/jobs/` | Default workload |
| --- | --- |
| `spin-local.json` | 262,144 particles, circular motion, 100 Myr |
| `spin-vast.json` | 1,048,576 particles, circular motion, central mass 4.3 million M☉, 250 Myr |
| `perturbed-orbits.json` | 262,144 particles, NFW + baryons, leapfrog, 20 km/s radial kick, 50 Myr |
| `model-comparison.json` | Five models × 1,024 radii |
| `uff-sweep.json` | 65 UFF β values × 1,024 radii |
| `compact-objects.json` | 128 masses × 128 signed spins |

Copy a job file, edit its parameters, then run `validate` before submitting a
large workload. Unknown keys and unsupported model names are rejected. A schema-1
job has `schema_version: 1` and a `task` object with `kind: spin`, `curves` or
`compact`. The resolved output of `validate` lists every default for that job.

The `physics` object uses snake_case names: `model`, `disk_ml`, `bulge_ml`,
`black_hole_million`, `uff_v_inf`, `uff_core`, `uff_beta`, `halo_log_mass`,
`halo_concentration`, `burkert_log_density`, `burkert_core`, `mond_a0`.
The five model names are `baryons`, `nfw`, `burkert`, `mond-rar`, `uff-empirical`.
Parameter units and ranges follow the [UFF dynamics notes](UFF-DYNAMICS.md),
except the native API also allows zero stellar mass-to-light coefficients.
`mond_a0` is in units of 10⁻¹⁰ m/s²; central mass is in millions of solar masses.

Curve grids are linear in radius. Optional sweeps use one of the physical
parameter names above (excluding `model`) with `min`, `max`, `count`. Choose
models that use that parameter; other models intentionally produce repeated
curves. A one-element grid uses its minimum. Compact-object grids are logarithmic
in mass and linear in spin, with `|spin| <= 0.998`.

## Local NVIDIA container

Build from the repository root:

```bash
docker build -f runtime/Dockerfile -t galaxy-runtime:0.3.0 .
docker run --rm --gpus all \
  -e NVIDIA_DRIVER_CAPABILITIES=graphics,utility,compute \
  galaxy-runtime:0.3.0 devices
mkdir -p runs
docker run --rm --gpus all \
  -e NVIDIA_DRIVER_CAPABILITIES=graphics,utility,compute \
  -v "$PWD/runs:/workspace/runs" \
  galaxy-runtime:0.3.0 run \
  --job /opt/galaxy/jobs/spin-vast.json --output /workspace/runs/container-spin
```

The host must have the NVIDIA driver and Container Toolkit configured. Vulkan
needs the `graphics` driver capability; exposing CUDA alone is insufficient.
See [NVIDIA's container-driver documentation](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/docker-specialized.html).
The image contains the executable, example jobs and dependency licence notices.
It does not install a host kernel driver or include a software Vulkan fallback.

## Vast.ai through SSH

Use an existing Linux GPU instance with SSH access and Vulkan driver support.
In its template's Docker options, set this **before creating the instance**:

```text
-e NVIDIA_DRIVER_CAPABILITIES=graphics,utility,compute
```

Changing that variable inside an already-started container cannot inject missing
host driver libraries. `nvidia-smi` working proves CUDA/NVML visibility, not Vulkan
availability. Confirm an actual adapter with `devices` and then run `verify`.

[Vast's execution-environment documentation](https://docs.vast.ai/guides/instances/docker-environment)
describes Docker options, launch modes and external SSH port mapping. Use the host
and SSH port displayed by your instance. Add your SSH key and connect once with
normal SSH to establish and verify its host key. The runner retains SSH's host-key
checks and uses batch authentication; it never uploads the private key.

```bash
python3 scripts/vast-runner.py \
  --host YOUR_INSTANCE_HOST --port YOUR_SSH_PORT \
  --identity ~/.ssh/id_ed25519 \
  --job runtime/jobs/spin-vast.json \
  --output runs/vast-spin --bootstrap
```

`--bootstrap` explicitly installs container build packages and the pinned Rust
toolchain on an Ubuntu/Debian instance where you are root. Omit it once these
exist. The runner uploads only the runtime's source/build inputs and job file,
builds and executes remotely, then retrieves the results under
`runs/vast-spin/results/`. `runner.log` preserves up to 16 MiB of combined remote
stdout/stderr, streamed in bounded binary chunks even without newlines. If that
limit is exceeded, the runner stops its SSH process, records a failed run and
retains the partial log. `runner.json` records
the remote directory, archive/job hashes, exit status and retrieval result.
A shared `galaxy-build-cache` under the remote root reuses compiled dependencies.

Add `--dry-run` to print the commands without contacting the instance. Use
`--adapter INDEX_OR_NAME` when selecting among available devices. `--remote-root`
changes the default `/workspace` location. The local runner intentionally has
no API key, offer search, rental creation, image publication or instance-deletion
operation. It also works with another provider's ordinary Linux SSH GPU host.

If SSH disconnects or the local runner is interrupted, the remote job may still
be running. The recorded remote directory is retained for recovery; inspect it
over SSH before rerunning. Stop or destroy your rented instance separately when
finished. Completed results and the shared build cache remain on its disk until
you remove them or destroy the instance.

For a prebuilt image on Vast, publish the supplied Dockerfile to a registry you
control, select that image, and use Entrypoint mode with `galaxy-runtime run ...`.
In SSH/Jupyter mode Vast replaces the image entrypoint; run the installed
`galaxy-runtime` executable inside the instance. Keep result files on persistent
storage or retrieve them before destroying the instance.

## Physics and numerical behavior

The source remains UFF commit
[`596cd73`](https://github.com/QSOLKCB/UFF/tree/596cd732df61587aa1a9801cad1ec13483b1347f),
with the unmodified demonstration component curves. GPU data tables are generated
from that CSV during the Rust build. Component velocities use linear interpolation
and constant endpoint extension outside 0.5–12 kpc. The initial authored particle
distribution spans approximately 0.18–13.2 kpc.

Circular jobs use `theta(t) = theta0 + omega * t`, with
`omega = V(R) / R` converted from km/s/kpc to radians/Myr. They evaluate only the
requested snapshot times: increasing `steps` changes model time, and does not
pretend to perform intermediate numerical steps. `particle_updates` counts
actual particle evaluations. The sampled frame count and synchronized compute
wall time are reported separately from total execution/output time.
The GPU evaluates circular phase in wrapped turns using split time/rate products,
then supplies an angle in `[-pi, pi]` to the trigonometric functions. The time split
retains 48 significant bits and avoids forming a large float32 phase, including
at the maximum admitted 200,000 Myr duration. No extra particle state is needed.

Leapfrog uses kick–drift–kick integration in the x/y plane:

```text
Rsoft = sqrt(x² + y² + epsilon²)
acceleration = -V(Rsoft)² / Rsoft² * (x, y)       [converted to kpc/Myr²]
vhalf = v + dt/2 * acceleration(x)
xnext = x + dt * vhalf
vnext = vhalf + dt/2 * acceleration(xnext)
```

The softened field is an explicit numerical extension of UFF's circular-speed
law. Initial tangential speed matches that softened force at the actual radius;
an optional radial kick creates noncircular trajectories. Particle heights remain
authored and static. The mass distribution is fixed; there are no pairwise stellar
forces, self-gravity updates, hydrodynamics or evolving density solver.

`dt_myr` is a numerical step for leapfrog. Choose a step well below the shortest
orbital timescale and compare runs at dt and dt/2 when changing masses, softening
or kicks. The sampled angular-momentum drift is a diagnostic, not a guarantee of
orbit accuracy or energy conservation. The bounded parameter ranges do not make
every admitted timestep accurate.

GPU equations and state use float32. Stable small-argument series avoid cancellation
in UFF, NFW, Burkert and MOND denominators. Burkert retains UFF's original leading
term below `R/core = 0.001`, then uses a higher-order continuation below 0.5.
CPU reference equations use float64 with float32 particle storage. GPU devices
can differ in transcendental rounding: source/recipe/output hashes support
reproduction and identification, not a claim of bit-identical cross-GPU results.
Phase wrapping preserves the precision of each stored angular rate. Independently
initialized CPU/GPU rates can still differ slightly, producing phase drift over
many revolutions. Long circular verification therefore compares float64 propagation
of the identical stored orbital parameters; the existing short-run checks also
compare independent CPU/GPU initialization.

Compact kernels implement UFF's Kerr radii, with signed spin relative to the
equatorial orbit. `r_g = GM/c²`; the Schwarzschild horizon is `2 r_g`.
Sphere-of-influence radius and `Delta / R_ISCO²` are evaluated in float64 on the
CPU. The latter uses UFF's `gamma = 0.2375` area-gap convention, and is scale
bookkeeping only. It is not fed back into galaxy forces or presented as an LQG
effective spacetime. UFF sky-lattice and formal-assurance workloads are not ported.

## Capacity and outputs

Native `particles` is the number of **actual resident states computed**, not the
browser's logical population. Each particle occupies 32 bytes:
`(initial_radius, initial_angle, static_z, angular_rate)` and `(x,y,vx,vy)`.
The software cap is 8,388,608 states (256 MiB), further bounded by the selected
adapter's single-storage-buffer limit. An adapter exposing 128 MiB supports at
most 4,194,304 states. Allocator, driver, input/output buffers and host RAM are
additional. Allocation limits are checked before creating buffers.

Snapshots contain at most 65,536 evenly indexed particles; all resident particles
are evolved. The GPU gathers this sample before readback, using overflow-safe
integer indexing. `snapshot_limit` controls CSV/image density, not the simulation
population. PNG rendering is performed on the CPU after readback; the image is a
particle-density preview with an authored palette, not photometry. CSV positions
and velocities have kpc and km/s units. At most 256 snapshots are admitted per job;
`snapshot_every: 0` writes only initial/final snapshots.

Each curves/compact job admits at most 1,048,576 evaluations. The SSH runner caps
the compressed result download at 4 GiB before writing excess bytes to disk and
separately caps extracted result files at 4 GiB. Before tar parsing, a guarded
gzip reader rejects requests above 64 KiB and limits all headers, extended
metadata and padding consumed by the parser to 16 MiB. Only validated regular
file payloads receive additional byte allowance; files are copied in 64 KiB
chunks before another header is parsed. Forward skips also spend the budget,
and sparse files are rejected. This bounds PAX/GNU metadata even when it is
hidden from member iteration. Archives admit at most 4,096
members, including empty files and directories, and at most 4,096 destination
paths, including implicit parent directories. Both counts are checked before
creating each entry. Exceeding the download limit
terminates the SSH transfer, records a failed run and removes the temporary
download. Larger retained remote outputs can be retrieved manually.
There is no automatic resume/checkpoint import or encoded video output
in this runtime version; the offline viewer plays the PNG sequence.

Artifact receipt hashes read each file in 64 KiB chunks and obtain byte counts
from file metadata, so finalizing a large CSV does not allocate another full copy.

## Validation

```bash
cargo test --manifest-path runtime/Cargo.toml --locked
python3 runtime/tests/test_runner.py
bash scripts/run-gpu.sh verify
```

`verify` executes the compiled GPU kernels against 195 predictions from UFF's
original Python galaxy models and 45 original compact-object cases. It checks
both orbital integrators/directions and an irregular 262,147-particle gather
whose naive index product would overflow u32. Tolerances are reported with results:
2e-4 relative for circular velocities, 8e-5 for Kerr diagnostics, and
3e-4 × (1 + |reference|) for stored orbital components.

Native tests cover source equations, job validation, timestep convergence,
angular momentum, CLI outputs and preservation of previous runs. Runner tests
exercise transfer/retrieval and failure status through mocked SSH; they do not
claim an actual paid cloud execution. CI executes the compute kernels through
explicitly enabled Mesa software Vulkan and builds/tests the container image.
Real GPU throughput and Vast host compatibility must be measured on the selected
machine with `devices`, `verify`, and its job receipt.

The runtime source fingerprint covers compiled local source, generated-data inputs,
reference fixtures, toolchain specification and Cargo lockfile. Build dependencies
are resolved through the lockfile; the browser's dependency-free Rust crate remains
separate. To regenerate the compact fixture from the pinned UFF checkout:

```bash
python3 scripts/generate-compact-reference.py /path/to/UFF
```
