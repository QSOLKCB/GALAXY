# UFF rotation-curve dynamics

This integration uses QSOL UFF v5.3.0 at
[`596cd732df61587aa1a9801cad1ec13483b1347f`](https://github.com/QSOLKCB/UFF/tree/596cd732df61587aa1a9801cad1ec13483b1347f).
The galaxy-model layer retained from UFF v4 supplies the equations. The sky-lattice,
formal-assurance and compact-object quantum bookkeeping layers are not invoked.

## Source mapping

| Pinned UFF source | GALAXY use |
| --- | --- |
| [`uff/models.py`](https://github.com/QSOLKCB/UFF/blob/596cd732df61587aa1a9801cad1ec13483b1347f/uff/models.py) | UFF empirical, NFW, Burkert and MOND/RAR circular-speed laws |
| [`uff/data.py`](https://github.com/QSOLKCB/UFF/blob/596cd732df61587aa1a9801cad1ec13483b1347f/uff/data.py) | Component interpolation, signed gas, mass-to-light scaling |
| [`uff/constants.py`](https://github.com/QSOLKCB/UFF/blob/596cd732df61587aa1a9801cad1ec13483b1347f/uff/constants.py) | G and kpc/year conversions |
| [`uff/compact.py`](https://github.com/QSOLKCB/UFF/blob/596cd732df61587aa1a9801cad1ec13483b1347f/uff/compact.py) | Weak-field central mass term only |
| [`DEMO_GALAXY.csv`](https://github.com/QSOLKCB/UFF/blob/596cd732df61587aa1a9801cad1ec13483b1347f/DEMO_GALAXY.csv) | Unmodified six-row demonstration data |
| [`docs/MODELS.md`](https://github.com/QSOLKCB/UFF/blob/596cd732df61587aa1a9801cad1ec13483b1347f/docs/MODELS.md) | Model definitions and original literature references |

`data/uff/provenance.json` records the source commit and SHA-256 of each file.
The Apache notice is preserved in `data/uff/NOTICE`. No network fetch occurs at
runtime. `scripts/package-uff-data.mjs` creates the JS and Rust tables from the
same CSV and checks its recorded hash.

## Equations used

All velocities below are in km/s and radii in kpc. G is converted from UFF's SI
constants to kpc (km/s)² / M☉. Component **velocities** are interpolated linearly
before combination, matching UFF's `np.interp` behavior.

```text
Vbar² = max(0, Vgas |Vgas| + diskML Vdisk² + bulgeML Vbulge²)
Vcentral² = G Mcentral / R
Vtotal² = Vbar² + Vcentral² + Vextra²
```

Gas retains its sign. Stellar mass-to-light values multiply V², not V. The
central mass slider is in millions of solar masses; it is multiplied by 10⁶.

For UFF empirical v4, `x = R / core`:

```text
Vextra = Vscale sqrt(1 - atan(x)/x) exp(beta x/(1+x))
```

The true large-radius limit is `Vscale * exp(beta)`. Accordingly, the UI calls
Vscale the **velocity scale**, not the asymptotic speed when beta is nonzero.
At `x < 10⁻³`, GALAXY evaluates `1 - atan(x)/x` by its expansion
`x²/3 - x⁴/5 + x⁶/7` to avoid floating-point cancellation. This is a numerical
continuation of UFF's equation; it does not add a force or new model parameter.

For NFW:

```text
rhoCrit = 3 H0² / (8 pi G), H0 = 70 km/s/Mpc = 0.07 km/s/kpc
r200 = (3 M200 / (4 pi 200 rhoCrit))^(1/3)
f(x) = ln(1+x) - x/(1+x)
M(R) = M200 f(c R/r200) / f(c)
Vextra² = G M(R) / R
```

For Burkert, `x = R / core`:

```text
M(R) = pi rho0 core³ [ln((1+x)²(1+x²)) - 2 atan(x)]
Vextra² = G M(R) / R
```

Both use UFF's small-radius series branches. The UI density parameter is
log₁₀(rho0 / [M☉ kpc⁻³]); the halo mass is log₁₀(M200 / M☉).

MOND/RAR acts on the complete Newtonian input, including the central mass:

```text
VN² = Vbar² + Vcentral²
gN = VN² * 10⁶ / (R * KPC_TO_M)
y = gN / a0
nu(y) = 1 / (1 - exp(-sqrt(y)))
Vtotal = sqrt(VN² * nu(y))
```

`expm1` evaluates the denominator stably. The zero-acceleration limit returns
zero explicitly. No external-field proxy or exact QUMOND/AQUAL solver is added.

## From a rotation curve to moving stars

The display radius maps to `R = 12 r` kpc. The existing star generator covers
approximately 0.18–13.2 kpc, including its compact visual bulge and extended halo.
The UFF demo rows cover 0.5–12 kpc. Outside that interval, component velocities
retain the nearest endpoint value, exactly as UFF's `np.interp` does. The plot
shades these extensions. Halo and central-mass terms are evaluated at the actual
radius; they are not clamped to the data interval.

Physical angular rates are computed using:

```text
omega_rad_per_Myr = (V_km_per_s / R_kpc) * 1000 * seconds_per_Myr / metres_per_kpc
orbit_buffer_rate = omega_rad_per_Myr * 20
angle = initial_spiral_angle + effective_clock * orbit_buffer_rate
```

One internal clock unit represents 20 Myr. The existing speed control integrates
that clock and displays the corresponding nominal Myr/s; its default 0.35 is
7 Myr/s. As in v0.1, long frame deltas are capped at 50 ms and hidden tabs stop
advancing. Time is an accumulated simulation parameter, not elapsed wall time.
Reverse integrates negative model time; pause preserves it. The physical-mode
slice slider shows its offset in Myr, while legacy mode keeps phase degrees.

Mass-model edits reset clock and offset to zero to compare a new rotation law
from the initial spiral. Display orientation, pitch, spread and exposure do not
alter the physical mass model. The authored bulge fraction changes which radii
the sampled stars occupy, so changing it rebuilds rates and restarts the clock.
The disc and bulge mass-to-light controls independently change the mass model.
In original visual mode, changing shear rebuilds the orbital rates while
preserving the accumulated clock and phase offset, including paused and
timeless phase slices.

The plot's selected total curve and the orbital-rate buffer use the same
parameters and source data. Its baryon curve excludes the optional central mass;
the total includes it. Inclination is a view control, not an observational
distance/inclination fit. Data error bars are simply those in the demo CSV.
There is no parameter optimizer, chi-squared claim, or fitted galaxy identity.

## Performance and local Rust use

The original eight float32 seed properties remain unchanged. A separate float32
angular-rate buffer adds four bytes per rendered star: 65,536 stars use 2 MiB +
256 KiB = **2.25 MiB**, with GPU copies and allocator overhead additional.
Rates are computed on parameter/sample changes, never in the per-frame JS loop.
WebGL consumes `a_rate` in its single point draw. Canvas uses the same rates.

Wasm ABI version 2 retains the sampling API and adds:

- `configure_dynamics(model, diskML, bulgeML, centralMillion, uffVInf, uffCore,
  uffBeta, haloLogMass, haloConcentration, burkertLogDensity, burkertCore,
  mondA0, visualBulgeFraction, legacyShear)`;
- `orbit_ptr()` and `orbit_len()` for the resulting float32 rate buffer;
- `circular_velocity(radiusKpc, model, ...the same physical parameters)` for
  scalar diagnostics (without the visual bulge/shear arguments).

Model IDs are 0 legacy, 1 baryons, 2 NFW, 3 Burkert, 4 MOND/RAR, 5 UFF empirical.
`mondA0` is in units of 10⁻¹⁰ m/s². Invalid buffer requests clear the rate buffer
and return zero; invalid scalar velocity requests return NaN. Reseeding clears
old rates. A client must reacquire both memory views after configuration because
Wasm memory can grow. JavaScript uses the same formulas if Wasm is unavailable.
Legacy mode has no physical circular velocity: JavaScript's `velocityComponents`
and `velocityKms` throw `RangeError`, native Rust returns `None`, and Wasm's
`circular_velocity` returns NaN. Legacy angular-rate calculations remain available.

```bash
cargo test --manifest-path rust/Cargo.toml --locked --offline
cargo run --manifest-path rust/Cargo.toml --example rotation_curve --release --locked --offline
```

## Verification and saved recipes

`tests/uff-reference.json` contains 195 predictions produced by directly executing
the pinned UFF Python model builder over five models, three parameter sets and
13 radii. It is an independent reference, not generated from the new port.
Regenerate only from the recorded source checkout, with UFF's Python dependencies:

```bash
python scripts/generate-uff-reference.py /path/to/UFF
node tests/physics.mjs
```

Tests compare JS and compiled Wasm to those predictions, all 65,536 per-star
rates across all modes, units, signed gas, central-mass effects and the zero-UFF
limit. v0.2 recipes include the UFF source/data identity and all mass parameters.
v0.1 recipes explicitly restore original visual motion. Unknown v0.2 model names
or different source identities are rejected before changing the running state.

These are prescribed circular orbits in an authored stellar distribution, not
an N-body integration or self-consistent evolution of the density field.
