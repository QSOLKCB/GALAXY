# GALAXY

An offline spinning galaxy instrument built from the VORTEX 2.1.0 particle lab,
now driven by rotation-curve models and demonstration data from
[QSOL UFF](https://github.com/QSOLKCB/UFF). Change the mass model and watch the
orbital speeds and rotation curve respond, with the original visual mode still
available for comparison.

Open **`index.html`** directly in a modern browser. No install, server, CDN or
network connection is needed, including for the bundled Rust/WebAssembly engine.

**GitHub Pages:** <https://qsolkcb.github.io/GALAXY/> — updates after changes merge
and the Pages workflow deploys. In the repository's Pages settings, choose
**GitHub Actions** as the source if it is not already set.

## Native GPU runtime v0.3

[`runtime/`](runtime/) runs headless Rust compute jobs on local or cloud GPUs,
including Vulkan-capable NVIDIA instances on Vast.ai. It evolves actual resident
particle states and supports circular spin, perturbed leapfrog orbits, UFF
rotation-curve sweeps and compact-object diagnostics. Jobs produce PNG/CSV output,
an offline snapshot viewer and a run receipt identifying the adapter and inputs.

```bash
bash scripts/run-gpu.sh devices
bash scripts/run-gpu.sh verify
bash scripts/run-gpu.sh run --job runtime/jobs/spin-local.json --output runs/local-spin
```

The local preset uses 262,144 actual particles; the cloud preset uses 1,048,576.
The runtime cap is 8,388,608, subject to the adapter's storage-buffer limits.
See **[GPU runtime and Vast.ai runner instructions](docs/GPU-RUNTIME.md)** for
setup, Docker, SSH submission, example jobs, precision and validation details.
The browser engine's logical/sample counts below describe its separate path.

## UFF dynamics in v0.2

Select a **Rotation law** to drive each star's orbital rate from `V(R)/R`:

| Mode | Contribution and controls |
| --- | --- |
| UFF empirical v4 + baryons | UFF velocity scale, core radius and bounded shape β |
| Newtonian baryons | Signed gas contribution, disc and bulge mass-to-light ratios |
| NFW halo + baryons | Halo mass M₂₀₀ and concentration |
| Burkert halo + baryons | Central halo density and core radius |
| MOND / RAR | UFF's exponential acceleration relation and adjustable a₀ |
| Original visual rotation | The v0.1 authored rotation law and manual shear |

An optional weak-field central mass applies to all physical models, with the
MOND boost acting on the combined Newtonian input as in UFF. Defaults use UFF's
initial parameters; **no fitting is performed**. The initial mode is UFF empirical.

The live plot shows the selected total circular speed, baryons alone, and UFF's
six demo rows with their supplied error bars. Readouts show circular speed at
8 kpc, the corresponding orbital period, and model time. Inclination changes
the viewing angle; it does not change the deprojected rotation curve.

The full UFF `DEMO_GALAXY.csv` is bundled unchanged and identified by a pinned
source commit and SHA-256 receipt. It is demonstration input, not an identified
observational catalogue. See [UFF physics notes](docs/UFF-DYNAMICS.md) for
equations, units, source links and interpolation choices.

## Capacity

The logical population and rendered sample are separate quantities, as in VORTEX.
Increasing the logical count never allocates one object per logical star.

| Active engine | Maximum logical stars | Maximum rendered stars per frame |
| --- | ---: | ---: |
| JavaScript + Canvas or WebGL | 2²⁴ = 16,777,216 | 1,024 |
| Rust/Wasm + Canvas fallback | 2³² = 4,294,967,296 | 1,024 |
| Rust/Wasm + WebGL | 2³² = 4,294,967,296 | 65,536 |

The accelerated path defaults to **16,384 rendered stars**. **PUSH IT TO THE
LIMIT** selects the maximum population and sample supported by the active engine.
You can still select VORTEX's 256/512/1,024-particle budgets independently.

The largest sample contains **2 MiB of float32 star properties plus 256 KiB of
orbital rates: 2.25 MiB total**. WebGL holds GPU copies; Wasm allocator memory,
transient rebuild buffers, framebuffer memory, and browser overhead are additional.
The interface reports the two particle buffers, not total application memory.

Rust generates a deterministic sample with exact 64-bit intermediate indexing.
Orbital rates are recalculated when the mass model, stellar population or
sample changes. WebGL uploads these bounded buffers, then evolves every rendered
star in a vertex shader using one point draw call per frame. Neither
language choice nor logical indexing alone makes billions of stars visible.
The 2²⁴ JavaScript ceiling preserves the supplied VORTEX contract; it is not a
fundamental JavaScript integer limit. See [the engine notes](docs/ENGINE.md).

## Use the instrument

- Choose **Grand design**, **Pinwheel**, **Flocculent**, or **Edge-on** morphology.
- Adjust spiral arms, pitch, spread, bulge and thickness.
- In physical modes, the selected `V(R)/R` sets differential rotation. In
  **Original visual rotation**, the **Differential shear** slider controls
  winding; set it to zero for a stable rotating spiral pattern.
- Pause or reverse without resetting accumulated motion. **Reset time/phase**
  returns to the original spiral. Changing a mass model, mass parameter or bulge
  population restarts the clock so a fresh comparison starts from that spiral.
- **Timeless phase slice** freezes the animation clock and uses the offset
  slider. Physical modes display time in Myr; visual mode displays phase degrees.
- Drag to adjust inclination and position angle; scroll to zoom. The same
  adjustments have labelled keyboard-accessible controls. Double-click resets
  the view; Space pauses when a form control does not have focus.
- Save a PNG, record up to 30 seconds of WebM when the browser supports it, or
  save/load JSON settings including seed, population, clock, mass parameters and
  UFF source identity. v0.1 files load with their original visual rotation law.
- Reduced-motion preferences start the simulation paused. Hidden tabs do not
  accumulate a jump in animation time.

## Model boundary

This is a **deterministic circular-orbit visualization**. In UFF modes, physical
rotation curves set the angular speeds in kpc, km/s and Myr. Stars stay at fixed
radii; the distribution and vertical structure remain authored. The morphology
sliders control display geometry independently from the mass-to-light sliders.
It is not a self-consistent N-body evolution, a fit to an observed galaxy, or a
gas/stellar-formation solver. The empirical UFF and MOND options retain the model
definitions and scope documented by UFF.

Logical indices describe the population from which representative particles are
sampled. Only the **rendered** count is evaluated and drawn each frame. The FPS
display measures the active renderer on the current device. No frame-rate
guarantee is inferred from the source archive or sample-generation benchmarks.

## Development and checks

The complete Rust crate is included in [`rust/`](rust/), alongside the standalone
compiled [`wasm/galaxy_sampler.wasm`](wasm/galaxy_sampler.wasm) module and its
offline browser wrapper. You can test the sampler natively from the repository
root without starting a browser:

```bash
cargo test --manifest-path rust/Cargo.toml --locked --offline
cargo run --manifest-path rust/Cargo.toml --example sample --release --locked --offline -- 32 65536 303
cargo run --manifest-path rust/Cargo.toml --example rotation_curve --release --locked --offline
```

The example arguments are **logical exponent**, **sample count**, and **seed**.
The command above exercises 2³² logical stars and a 65,536-star sample. It prints
the actual buffer size, final logical ID and native generation time. Use
`24 1024 303` to compare with the legacy VORTEX budget.
The `rotation_curve` example prints all five physical models in km/s at the
six demo radii using the same Rust code as the browser module.

The generated Wasm payloads are committed so downloaded copies work offline.
To change the Rust sampler, install Rust with the pinned toolchain in
`rust-toolchain.toml`, then run:

```bash
cargo test --manifest-path rust/Cargo.toml --locked --offline
bash scripts/build-wasm.sh
node tests/smoke.mjs
node tests/physics.mjs
node tests/app.mjs
node tests/benchmark.mjs
node scripts/build-site.mjs
```

The packaged binary exposes the sampling API plus `configure_dynamics`,
`orbit_ptr`, `orbit_len`, and `circular_velocity` under ABI version 2. The browser
wrapper contains the same binary, encoded for direct `file://` use.

The crate has no third-party dependencies. Installing the Rust toolchain and its
Wasm standard library for the first time needs a connection; building thereafter
uses `--offline`. Running the already-packaged application does not need Rust.

CI regenerates the CSV tables, rebuilds with the pinned compiler, checks the
shipped payloads for drift, and tests **195 predictions generated by the original
UFF Python implementation** against JS and compiled Wasm. It also checks maximum
sample size, orbital units, settings migration, controls and fallbacks.
The application suite uses DOM/renderer adapters;
it does not assert browser shader compilation, video-encoder behavior or GPU FPS.
`tests/benchmark.mjs` measures **Wasm sample and orbital-rate generation only**.

An optional local server can serve the same files:

```bash
python3 -m http.server 8000
```

## Provenance and licensing

Adapted browser files retain VORTEX's **MPL-2.0** licence. The new Rust sampler
and build tooling use this repository's existing **Apache-2.0** licence.
See [NOTICE.md](NOTICE.md) for the file-level distinction and source attribution.
VORTEX reference images and historical performance reports are not redistributed.
