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

## BH #2A — Morton / flat-tree substrate

BH #1 is frozen by merged PR #18. The next representation is deliberately pointer-free before any shader promotion.

The native reference now supports:

- 16 bits per planar axis encoded into one 32-bit Morton/Z-order key;
- stable sorting so equal keys preserve resident body order;
- a flat cell array with explicit `u32` child indices and contiguous Morton body ranges;
- a fixed 16-level Morton topology cap;
- bottom-up mass and centre-of-mass aggregation after topology construction;
- iterative flat traversal using the same Barnes–Hut opening rule and target-cell exclusion;
- a topology checksum over discrete Morton ordering and tree links;
- separate construction and traversal timing in `galaxy.barnes-hut-flat-receipt.v1`.

The 16-level cap is a representation property, not an astrophysical claim. If more than the configured bucket size maps to one final quantized cell, that leaf retains all of those bodies and evaluates them directly. The receipt records the maximum leaf occupancy so this condition is observable.

The flat reference remains f64. This avoids combining a topology migration with an arithmetic-precision migration. Shader-oriented f32 packing belongs to the next phase and must be validated independently against BH #1/direct-force evidence.

### Verification

`verify-flat` requires:

1. theta-zero flat traversal parity with the direct O(N²) oracle to floating reduction-order tolerance;
2. theta-0.5 error inside the existing BH #1 RMS/worst gates;
3. close agreement between flat traversal and the recursive BH #1 oracle on the deterministic fixture;
4. repeatable Morton topology checksums;
5. stable ordering for equal Morton keys;
6. exact root range and total mass preservation.

## BH #2B1 — explicit GPU transfer + traversal

BH #2A is frozen by merged PR #19. The recursive BH #1 tree and the flat f64 BH #2A tree remain the scientific oracles.

The GPU-facing ABI is intentionally explicit:

| Record | Bytes | Contents |
| --- | ---: | --- |
| settings | 32 | body/cell/root counts + theta, softening, G |
| body | 32 | f32 x/y/mass + Morton-entry position |
| Morton entry | 16 | Morton code + resident body index |
| flat cell | 64 | f32 bounds/mass/COM + u32 range/depth/children |
| acceleration | 16 | f32 ax/ay + reserved lanes |

Rust, WGSL and CUDA share that record contract. A canonical 144-byte fixture freezes the Rust/CUDA byte representation. The CUDA verifier is host-side structural evidence only; it does not claim that a CUDA kernel executed when no NVIDIA device is present.

The Vulkan path executes the actual flat traversal in WGSL. It preserves:

- Morton-range target membership instead of geometric containment;
- direct evaluation inside leaf ranges;
- the same `s / d < theta` aggregate rule;
- deterministic child visitation order;
- a bounded local DFS stack justified by the 16-level Morton cap.

The evidence binary `galaxy-bh-gpu` reports CPU tree-build time, f32 packing time, GPU transfer, GPU dispatch, GPU readback, flat-CPU reference traversal, and direct-force reference time separately. Error fields compare GPU accelerations against both the flat f64 CPU oracle and the independent direct O(N²) oracle.

This phase does **not** perform GPU tree construction or evolve multiple self-gravity steps.

## Next rung

BH #2B2 adds the evolving-state boundary:

```text
verified GPU traversal
  -> first acceleration
  -> GPU kick/drift
  -> rebuild flat tree for drifted positions
  -> second GPU acceleration
  -> final kick
  -> multi-step receipts and drift diagnostics
```

GPU-side Morton sorting/tree construction remains a later optimization candidate. It must not be silently conflated with traversal correctness or inferred from third-party benchmark numbers.
