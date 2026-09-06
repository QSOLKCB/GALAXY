# GALAXY

An offline spinning galaxy instrument built from the VORTEX 2.1.0 particle lab.
Spiral arms, a central stellar bulge and a sparse halo replace VORTEX's
gate–centre–mouth paths. Change morphology, spin, shear, inclination, density
and starlight while the galaxy runs.

Open **`index.html`** directly in a modern browser. No install, server, CDN or
network connection is needed, including for the bundled Rust/WebAssembly engine.

**GitHub Pages:** <https://qsolkcb.github.io/GALAXY/> — becomes available after
this implementation is merged and the Pages workflow deploys. In the repository's
Pages settings, choose **GitHub Actions** as the source if it is not already set.

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

The largest sample contains **2 MiB of float32 particle data**. WebGL holds a
GPU copy; Wasm allocator memory, transient rebuild buffers, framebuffer memory,
and the browser's own overhead are additional. The interface reports the particle
buffer, not total application memory.

Rust generates a deterministic sample with exact 64-bit intermediate indexing.
WebGL uploads it once per population, seed or budget change, then evolves every
rendered star in a vertex shader using one point draw call per frame. Neither
language choice nor logical indexing alone makes billions of stars visible.
The 2²⁴ JavaScript ceiling preserves the supplied VORTEX contract; it is not a
fundamental JavaScript integer limit. See [the engine notes](docs/ENGINE.md).

## Use the instrument

- Choose **Grand design**, **Pinwheel**, **Flocculent**, or **Edge-on** morphology.
- Adjust spiral arms, pitch, spread, bulge and thickness.
- **Differential shear** makes inner stars orbit faster and gradually winds the
  arms. Set it to **zero** for a stable rotating spiral pattern.
- Pause or reverse without resetting the accumulated motion. **Reset phase**
  returns to the original spiral arrangement.
- **Timeless phase slice** freezes the animation clock and uses the phase slider.
- Drag to adjust inclination and position angle; scroll to zoom. The same
  adjustments have labelled keyboard-accessible controls. Double-click resets
  the view; Space pauses when a form control does not have focus.
- Save a PNG, record up to 30 seconds of WebM when the browser supports it, or
  save/load JSON settings including the seed, logical population and clock.
- Reduced-motion preferences start the simulation paused. Hidden tabs do not
  accumulate a jump in animation time.

## Model boundary

This is a **deterministic kinematic visualization**, not a gravitational N-body
solver or a model fitted to a particular observed galaxy. Disc stars start on
seeded logarithmic arms and orbit at fixed radii using an authored softened
rotation curve. The bulge and halo are visual populations. No pairwise gravity,
dark-matter inference, gas dynamics, star formation or accretion is computed.

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
```

The example arguments are **logical exponent**, **sample count**, and **seed**.
The command above exercises 2³² logical stars and a 65,536-star sample. It prints
the actual buffer size, final logical ID and native generation time. Use
`24 1024 303` to compare with the legacy VORTEX budget.

The generated Wasm payloads are committed so downloaded copies work offline.
To change the Rust sampler, install Rust with the pinned toolchain in
`rust-toolchain.toml`, then run:

```bash
cargo test --manifest-path rust/Cargo.toml --locked --offline
bash scripts/build-wasm.sh
node tests/smoke.mjs
node tests/app.mjs
node tests/benchmark.mjs
node scripts/build-site.mjs
```

The packaged binary exposes `memory`, `abi_version`, `max_rendered`, `generate`,
`buffer_ptr`, and `buffer_len`. The browser wrapper contains the same binary,
encoded as a classic script for direct `file://` use.

The crate has no third-party dependencies. Installing the Rust toolchain and its
Wasm standard library for the first time needs a connection; building thereafter
uses `--offline`. Running the already-packaged application does not need Rust.

CI rebuilds the module with the pinned compiler, checks the shipped payload for
drift, tests JS/Wasm agreement and maximum sample size, and checks application
controls and fallback behavior. The application suite uses DOM/renderer adapters;
it does not assert browser shader compilation, video-encoder behavior or GPU FPS.
`tests/benchmark.mjs` measures **Wasm sample generation only**.

An optional local server can serve the same files:

```bash
python3 -m http.server 8000
```

## Provenance and licensing

Adapted browser files retain VORTEX's **MPL-2.0** licence. The new Rust sampler
and build tooling use this repository's existing **Apache-2.0** licence.
See [NOTICE.md](NOTICE.md) for the file-level distinction and source attribution.
VORTEX reference images and historical performance reports are not redistributed.
