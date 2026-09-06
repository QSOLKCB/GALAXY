# Engine and reproducibility contract

Version 0.2 adds the [UFF rotation-curve dynamics](UFF-DYNAMICS.md). The original
visual rotation remains selectable; its formula below applies only in that mode.

## From VORTEX to GALAXY

| VORTEX 2.1.0 mechanism | GALAXY adaptation |
| --- | --- |
| Logical mouths separate from a bounded rendered sample | Logical stellar population separate from rendered stars |
| Equal-interval logical-index mapping | Same mapping, followed by an integer hash for stellar properties |
| Canvas particle budget of 256/512/1,024 | Retained as the low-cost and fallback budgets |
| Gate → exact centre → matching mouth | Fixed-radius orbits in a spiral disc, bulge and halo |
| Animated or manually scrubbed phase | Continuous accumulated motion plus a timeless phase slice |
| Local PNG, WebM and JSON capture | Retained; JSON additionally preserves logical count and clock |

The supplied archive contains JavaScript, not Rust. The Rust/Wasm module is a new
component. The historical 2²⁵ allocation failure in VORTEX's stress test does
not establish an inherent JavaScript number/index ceiling. GALAXY conservatively
retains the legacy JS limit while testing a larger contract in its Rust path.

## Population and sampling

For logical population `L`, rendered count `S` and sample index `i`:

```text
logical_id(i) = floor(i * L / S),  0 <= i < S <= L
```

`L <= 2^32`; `S <= 65,536`. Rust uses a u64 intermediate product and converts
the resulting identity to u32. The final identity remains below `L`, including
when `L = 2^32`. The Wasm ABI receives `L` as an integral f64 so passing 2^32
cannot truncate to zero at a u32 boundary.

Eight independently salted 32-bit integer hashes produce 24-bit fractions,
stored as eight float32 values per sampled star: radial variate, arm/angle
variate, scatter, height, population class, size, light and tint. JS uses
`Math.imul`; Rust uses wrapping multiplication. The fractions are exactly
representable in float32. Tests compare the complete fallback sample and
representative extreme indices across JS and the compiled Wasm module.

No full logical-population array is constructed. Changing a render budget
changes the representative indices; properties of the same logical ID and seed
remain stable. Statistical hash sampling does not promise exact bulge/halo
fractions in small samples.

The owned Rust vector is bounded by `S * 8 * 4` bytes. Rebuilds can temporarily
hold both old and new vectors. JS reacquires `memory.buffer` and the exported
pointer after every generation because Wasm memory growth invalidates old views.
A separate float32 orbital-rate vector is recalculated when the model or sample
changes. Both vectors are reacquired after Wasm configuration, uploaded with
`STATIC_DRAW`, and total 2.25 MiB at maximum size. Animation changes uniforms only.

## Original visual motion and shared projection

Disc radius is `r = 0.06 + 0.92 u^1.4` in display units. Its initial angle is:

```text
theta0 = arm * 2*pi / arms + log(r / 0.1) / tan(pitch) + scatter_offset
omega(r) = 0.32 * ((1 - shear) + shear / sqrt(r*r + 0.12*0.12))
theta = theta0 + effective_phase * omega(r)
```

Bulge and halo use separate compact and extended radial distributions and
uniform initial azimuths. All radii stay fixed and nonzero. Their thicknesses
create depth when the disc is inclined. Inclination projects `(y,z)` before the
position angle rotates the screen plane. Canvas and GLSL implement the same
equations, with normal floating-point differences in transcendental functions.
Float32 shader precision means screenshots are not promised to be pixel-identical
across devices or after very long accumulated phases.

The accumulated animation clock integrates speed and direction. Pausing changes
neither the clock nor the displayed position. A speed or direction edit affects
future clock increments. Individual frame deltas are capped at 50 ms; hidden
tabs reset their timestamp. This is an animation clock, not wall-clock physical
time. A phase slice uses only the phase slider and leaves the animation clock
available when returning to animation.

In both motion families, differential rotation can wind the initial spiral arms. That is intentional; no
unimplemented density-wave dynamics are implied. In original visual mode, zero shear gives rigid pattern
rotation for a persistent visual spiral. Physical modes use the selected UFF
rotation curve directly and hide the manual shear control.

## Rendering and fallback

The WebGL renderer draws one point per sampled star, in one `drawArrays(POINTS)`
call. A radial point shader provides starlight with additive blending. There are
no duplicated background stars, temporal particle substitutions or undisclosed
extra rendered populations. Density changes exposure scaling to limit saturation.

Without Wasm, controls clamp to 2²⁴ logical / 1,024 rendered, even if WebGL works.
Without WebGL, Canvas renders at most 1,024 stars with cached colour strings and a
reused projection object. If Rust remains available, the Canvas sample can still
represent a 2³² logical population. WebGL context loss stops the current recording
and switches to a fresh Canvas element with the smaller sample.

The payload is base64 in an external classic script, avoiding fetch and file-URL
CORS restrictions when opening `index.html` directly. It needs no `eval`, no
worker, no remote package, and no cross-origin-isolation headers. CSP explicitly
permits Wasm compilation using `wasm-unsafe-eval`. Compilation failure uses the
bounded JavaScript fallback and is reported in the interface.

## State and validation

JSON carries `application`, `version`, `physicsSource`, `settings`, and `clock`.
The source field binds a v0.2 recipe to the bundled UFF commit/data hash. v0.1
recipes restore legacy dynamics explicitly. Supported fields
are allowlisted and numerically bounded. Imported populations/budgets are clamped
to current engine capability and any reduction is reported. A bad file leaves
the running state untouched. Exporting does not reduce the logical population to
the rendered sample. A settings file is a recipe, not a dump of all logical stars.

Native Rust tests cover indexing, rejection, buffer capacity and model behavior.
Independent UFF Python reference predictions check the physical equations in
JavaScript and the compiled Wasm binary, including their units. Node tests run
the shipped Wasm binary, check CPU orbital invariants and clock behavior, and
exercise the actual application using event/DOM/render adapters. These tests do
not substitute for a browser/device GPU benchmark or claim a performance result
on the user's hardware. The benchmark script reports sample/orbital-rate
generation timing and memory, including its Node version.
