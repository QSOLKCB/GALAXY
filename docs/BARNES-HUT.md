# Barnes–Hut self-gravity phase

GALAXY now has two deliberately distinct dynamics families:

1. **Prescribed-field / test-particle modes** — the existing browser, CPU and GPU paths. These retain huge logical address spaces because individual resident particles do not affect one another.
2. **Resident self-gravity** — the Barnes–Hut laboratory. Every resident body's mass contributes to the force field, so the complete interacting resident set is explicit and cannot be substituted by independent logical-u64 tiles.

## Why Barnes–Hut

Direct gravitational N-body force evaluation requires O(N²) pair terms per force solve. Barnes–Hut recursively groups spatially nearby bodies and represents a sufficiently distant cell by its total mass at its centre of mass. For a cell width `s`, target-to-centre-of-mass distance `d`, and opening threshold `theta`, GALAXY accepts the aggregate when:

```text
s / d < theta
```

Smaller `theta` opens more cells and increases force work; `theta = 0` disables aggregate acceptance. The initial default is `theta = 0.5`, but it is a parameter, not a universal accuracy guarantee.

GALAXY also refuses to aggregate any cell whose square contains the target body. This makes self-force exclusion explicit instead of relying on one particular theta bound.

## Numerical model

The initial native reference is planar because GALAXY's existing orbit runtime is planar. The tree is therefore a quadtree. The force law is still the ordinary 3D point-mass inverse-square law evaluated at `z = 0`:

```text
r² = dx² + dy² + epsilon²

a = G M (dx, dy) / r³
```

`epsilon` is Plummer-style softening used to regularize close encounters. Integration uses kick–drift–kick leapfrog:

```text
v_half = v_n + 0.5 dt a(x_n)
x_next = x_n + dt v_half
rebuild tree at x_next
v_next = v_half + 0.5 dt a(x_next)
```

The tree is rebuilt for each force state. Duplicate/coincident bodies cannot cause unbounded subdivision because leaf capacity and maximum depth are explicit.

## Verification ladder

The Barnes–Hut path is not allowed to prove itself only against its own traversal.

The native and browser references include:

- a separate direct O(N²) force evaluator;
- theta-zero traversal checks against direct summation, allowing only floating reduction-order roundoff;
- theta-0.5 RMS and worst-relative acceleration checks on deterministic fixtures;
- coincident-body depth/finite-force tests;
- seeded-state reproducibility checks;
- deterministic direct-force probes exposed in the browser visualizer.

A future GPU implementation should preserve this CPU/direct reference as its correctness oracle.

## Visualization

`barnes-hut.html` is an offline browser laboratory. It renders the resident bodies and can overlay quadtree cells while the system evolves. Controls expose:

- resident body count;
- opening threshold theta;
- softening epsilon;
- leaf bucket size;
- tree overlay depth;
- leapfrog timestep and steps per rendered frame;
- deterministic presets and seed;
- a 12-body exact-force probe reporting RMS and maximum relative acceleration error.

The readout also reports the number of direct leaf interactions plus accepted aggregate cells in the latest force solve, alongside the corresponding reduction relative to N(N-1) directed direct terms. That is an algorithmic work diagnostic, not a wall-time speedup claim.

## Research provenance

This implementation is GALAXY-owned code written against public descriptions and used as a clean reference rather than copying another project's implementation.

Useful background references:

- Barnes & Hut overview: <https://en.wikipedia.org/wiki/Barnes%E2%80%93Hut_simulation>
- Princeton COS 126 Barnes–Hut assignment: <https://www.cs.princeton.edu/courses/archive/fall03/cs126/assignments/barnes-hut.html>
- Jeffrey Heer interactive explanation: <https://jheer.github.io/barnes-hut/>
- Arbor.js Barnes–Hut notes: <https://arborjs.org/docs/barnes-hut>
- Microsoft Research optimization notes: <https://www.microsoft.com/en-us/research/blog/optimizing-barnes-hut-t-sne/>
- rakau heterogeneous CPU/GPU Barnes–Hut library: <https://github.com/bluescarni/rakau>
- Stochastic Barnes–Hut GPU research: <https://arxiv.org/abs/2506.02219>
- CUDA/Vulkan Barnes–Hut implementation surveyed for later GPU architecture work: <https://github.com/Patistar/Nbody-Barnes-Hut-CUDA>

Those references inform the algorithmic research surface; they are not runtime dependencies.

## Next rung

Do **not** start by porting the recursive CPU tree literally to a GPU.

The next performance phase should first freeze this resident-force contract, then investigate a data-parallel representation:

```text
bounding box
  -> Morton/Z-order keys
  -> stable spatial ordering
  -> flat tree / cell aggregates
  -> GPU traversal
  -> leapfrog update
```

That phase should retain direct-force small-N fixtures and CPU Barnes–Hut comparisons, and should record tree construction separately from traversal timing. GPU performance is not inferred from third-party benchmark numbers.
