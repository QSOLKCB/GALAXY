# Barnes–Hut self-gravity phase

GALAXY now has two deliberately distinct dynamics families:

1. **Prescribed-field / test-particle modes** — the rotation-law browser instrument (`rotation-lab.html`), CPU and GPU paths. These retain huge logical address spaces because individual resident particles do not affect one another.
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

`index.html` is the default lightweight N-body observatory. It reuses the verified browser force solver with 768 resident bodies by default (128–2,048 selectable), fixed-rate leapfrog scheduling, seeded presets, optional trails and luminous sprites. Rendering contributes no additional gravitational mass. Orbit/zoom controls project the planar dynamics without modifying them. At 4× speed on an ordinary 60 Hz display, each rendered frame advances four physics steps. After a render gap the scheduler may batch up to 24 steps, derived from the 100 ms elapsed-time cap, then discards any remaining overload debt; hidden tabs suspend physics and under sustained load simulated time advances more slowly. Force probes pause the simulation so their result remains attached to the displayed state.


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

The evidence binary `galaxy-bh-gpu` reports CPU tree-build time, f32 packing time, GPU transfer, GPU dispatch, GPU readback, flat-CPU reference traversal, and direct-force reference time separately. Error fields compare every GPU acceleration against the flat f64 CPU oracle and a bounded deterministic subset against the independent direct-force oracle. This keeps high-count GPU verification from requiring a complete O(N²) CPU solve.

BH #2B1 is frozen by merged PR #20. It does **not** perform GPU tree construction.

## BH #2B2 — evolving GPU state with a host rebuild boundary

BH #2B2 adds persistent resident state without changing the frozen BH #2B1 force-transfer records.

Additional GPU records are:

| Record | Bytes | Contents |
| --- | ---: | --- |
| evolution settings | 32 | body count + timestep |
| evolving state | 32 | f32 x/y/mass + vx/vy |

The execution ladder is:

```text
initial CPU flat tree
  -> GPU force into persistent acceleration buffer
  -> GPU half-kick + drift
  -> drifted state readback
  -> CPU flat-tree rebuild
  -> f32 tree packing
  -> GPU boundary force
  -> GPU final half-kick
  -> next step reuses that boundary acceleration
```

For N steps, the evolution path performs N+1 force solves and N host tree rebuilds. Force output is not read back between traversal and the corresponding GPU integration kernel. The only per-step state readback occurs after drift, where host tree reconstruction requires the new positions.

`galaxy-bh-evolve` emits `galaxy.barnes-hut-gpu-evolution-receipt.v1` with:

- force-solve and host-rebuild counts;
- initial/final state checksums;
- initial/final flat-topology checksums plus rebuilds that changed topology;
- cumulative CPU tree-build/rebuild and f32 packing time;
- cumulative GPU force upload/dispatch, kick/drift, final-kick and readback time;
- final direct-force probe error;
- total mass, centre-of-mass, linear-momentum and angular-momentum drift diagnostics;
- a complete f64 flat-tree trajectory comparison for workloads below explicit particle and particle×step limits.

Large GPU jobs do not silently run a full CPU trajectory first. The CPU trajectory oracle is bounded to avoid turning validation into the dominant workload; bounded direct-force probes remain available at the final state.

The matched CUDA source includes the same 32-byte evolution settings/state ABI and kick/drift/final-kick kernels. Host-side CI checks source/layout parity but does not claim CUDA execution without an NVIDIA run.

## BH #2C — GPU-built flat tree

BH #2B2 is frozen by merged PR #21. BH #2C removes the per-step host rebuild boundary for workloads up to 4,096 resident bodies.

The device build is:

```text
persistent f32 state
  -> one-invocation f32 bounds reduction
  -> parallel Morton generation
  -> deterministic GPU bitonic sort on (Morton code, body index)
  -> parallel target-position assignment
  -> one-invocation level-order flat topology build
  -> one-invocation reverse-order aggregate build
  -> frozen BH #2B1 traversal
```

The sort's body-index tie break gives equal Morton keys the same resident-order intent as BH #2A's stable host sort. The flat GPU cell representation retains contiguous sorted ranges, explicit child indices, the 16-level Morton cap, target range membership, and bottom-up mass/centre-of-mass aggregates.

Cell numbering is level-order rather than BH #2A's recursive preorder, and bounds/aggregates are f32. BH #2C therefore defines a **new GPU topology representation** instead of claiming bit-identical topology checksums with the f64 host tree. Its scientific gates are:

- full final-force comparison against a freshly built BH #2A f64 flat tree;
- bounded exact direct-force probes;
- full multi-step trajectory comparison against BH #2A f64 leapfrog;
- deterministic same-state rebuild checksum over sorted entries and discrete GPU cell ranges/links;
- explicit capacity-overflow rejection.

`galaxy-bh-gpu-tree` requires zero host particle readbacks and zero host tree rebuilds during the evolution loop. Final state/tree readbacks exist only for evidence.

The current builder intentionally serializes the control-heavy bounds, bitonic sort, topology and aggregate stages inside GPU kernels. This establishes device ownership and correctness; it is **not** a performance architecture claim.

## BH #2D — parallel GPU tree construction

BH #2C is frozen by merged PR #22 and immutable v0.7.0. BH #2D preserves the same device-owned evolution boundary while replacing the serialized construction stages with bounded parallel GPU work.

The build is:

```text
persistent f32 state
  -> workgroup-parallel bounds reduction
  -> parallel Morton generation
  -> stable block-parallel 4-bit LSD radix ordering
  -> parallel target-position assignment
  -> parallel sparse level-order cell ranges + links
  -> reverse-depth parallel aggregates
  -> frozen BH #2B1 traversal
```

### Stable Morton ordering

The radix pipeline performs eight least-significant-digit passes over the 32-bit Morton code. Each pass uses:

- per-workgroup 16-bin histograms;
- digit/block prefix offsets;
- stable per-workgroup scatter based on lane-order rank;
- ping-pong entry buffers.

Input entries begin in resident body-index order. Because each radix pass is stable, bodies with equal Morton codes retain resident order without a separate global body-index sort key.

### Sparse deterministic topology

BH #2D avoids concurrent cell-index allocation by assigning each possible occupied range a deterministic sparse slot:

```text
slot = depth * N + group_start
```

for Morton depths 0 through 16.

A cell exists only when its sorted position is the start of an occupied prefix range and its parent was not already terminated by the bucket rule. Per-depth kernels build ranges and links in parallel. Aggregate kernels then run from the deepest level back to the root so every parent reads completed child mass/centre-of-mass values.

This representation intentionally spends additional sparse cell-buffer memory to remove index-allocation races and make the discrete GPU topology reproducible. The initial phase cap is 65,536 resident bodies.

### Verification

`galaxy-bh-gpu-tree-parallel` keeps the entire BH #2D rebuild loop on the device and records:

- zero host tree rebuilds during evolution;
- zero host particle readbacks during evolution steps;
- strict sorted `(Morton code, body index)` evidence;
- same-state repeat tree checksum;
- final force versus BH #2A f64 flat traversal;
- bounded exact direct-force probes;
- full BH #2A f64 trajectory comparison for bounded oracle workloads;
- BH #2C serialized-GPU trajectory/force parity when within its 4,096-body cap;
- BH #2B2 host-built-GPU trajectory/force parity for bounded oracle workloads.

The full CPU/BH #2C/BH #2B2 oracle work is explicitly bounded. Larger BH #2D runs record those references as skipped instead of silently turning a GPU-scale workload into a CPU validation workload.

### Timing and performance boundary

Tree construction reports separate stage-family wall times for:

- parallel bounds reduction;
- Morton generation;
- stable radix ordering;
- target-position assignment;
- topology construction;
- reverse-depth aggregates.

Kernel families are batched into command buffers to avoid measuring one CPU/GPU synchronization after every tiny kernel.

Receipts classify the selected adapter as either `software-validation` or `hardware`. `--require-hardware` rejects software adapters. Mesa/llvmpipe timing is therefore correctness and diagnostic evidence only; a hardware performance claim requires a receipt from a real GPU.

### Real-hardware sweep

`scripts/bench-bh2d-hardware.py` is the evidence runner for the remaining BH #2D hardware item. It executes a strictly increasing resident-body sweep through `galaxy-bh-gpu-tree-parallel --require-hardware`, refuses a dirty tracked source tree or an existing evidence directory, validates every receipt, requires one adapter identity for the complete sweep, and hashes each receipt/log into `manifest.json`.

The default sweep is:

```text
512 -> 1024 -> 2048 -> 4096 -> 8192 -> 16384 -> 32768 -> 65536
```

Counts through 4,096 retain repeat-matched BH #2C timing. Larger points explicitly require the BH #2C and bounded full-oracle skips already encoded by the verifier; they do not silently drop those gates.

Example on a real GPU:

```bash
sh scripts/bench-bh2d-hardware-launch.sh \
  --output runs/bh2d-hardware-sweep \
  --adapter 0
```

A completed scaling manifest is hardware evidence for the exact recorded source, workload and adapter. It does **not** by itself promote BH #2D to production or establish a host-independent speed claim.

## Next rung

After real-hardware BH #2D receipts exist, BH #2E can evaluate scaling, memory/capacity limits and production promotion while retaining BH #2A/B2B2/B2C as frozen oracles.

### Hardware harness launch and toolchain boundary

Launch real evidence capture through the clean launcher: `sh scripts/bench-bh2d-hardware-launch.sh ...` on POSIX or `scripts\\bench-bh2d-hardware-launch.cmd ...` on native Windows. The launcher starts isolated Python under a newly constructed environment so inherited loader-injection variables are not resident in the evidence process. Direct Python execution is rejected. Imported use by the unit-test runner is for host validation only.

The harness now selects and records a platform-specific tool boundary. Linux uses the system compiler/linker/Git path, macOS uses the native system paths plus standard Homebrew locations, and Windows records Git plus any active MSVC/LLVM linker tools before narrowing the build PATH to those resolved tool directories. Cargo and rustc are recorded separately from rustup proxies and invoked through their resolved binaries. The OS, installed system libraries and toolchain are trusted host prerequisites; this is provenance checking, not a sandbox against a hostile host. Git checks clear ambient `GIT_*` selectors and bind the checkout explicitly, including linked worktrees. Relative `CARGO_HOME` is interpreted from Cargo's repository working directory. Logs preserve raw bytes, receipt parser failures enter the failed-manifest path, and workload values are validated before any output is created. Manifests serialize strict JSON.
