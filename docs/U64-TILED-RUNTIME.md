# GALAXY 64-bit tiled runtime

`galaxy-u64` is an experimental native GPU execution path for logical particle populations that are larger than a single resident GPU buffer and, specifically, larger than the 32-bit index space.

It is intentionally separate from the stable `galaxy-runtime` binary while the wider addressing and tiling contract is validated. The browser/Wasm path is unchanged.

## Why this exists

The v0.3 native runtime caps one resident spin field at 8,388,608 particles. That is a useful bounded GPU workload, but it also leaves two unrelated limits entangled:

- how many particles are resident in one GPU buffer;
- how many particles belong to the logical simulation.

`galaxy-u64` separates them.

A job can describe one logical population with a 64-bit particle count while the GPU processes bounded resident tiles. The supplied `beyond-u32.json` job contains:

- **4,303,355,904 logical particles**;
- **8,388,608 resident particles per full tile**;
- **513 tiles**;
- **1,000 leapfrog steps**;
- **4,303,355,904,000 particle-step updates**.

That logical count is one full 8,388,608-particle tile beyond `2^32`.

## Physics boundary

Tiling is valid here because GALAXY's native spin jobs evolve independent test particles in one fixed UFF-derived potential. There are no pairwise stellar forces, self-gravity updates, hydrodynamic interactions, or evolving density field that must be exchanged between tiles.

Therefore the tile boundary is an execution partition, not an additional approximation to the equations already documented in [GPU-RUNTIME.md](GPU-RUNTIME.md).

If a future GALAXY mode introduces particle-particle or tile-coupled physics, this execution argument no longer applies automatically. Such a mode must define and validate its own communication rule.

## 64-bit global addressing

Every particle is identified by its full global `u64` index. The shader receives the resident tile base as split low/high `u32` words and adds the tile-local invocation index with an explicit carry.

Initial-condition random lanes use both halves of the global ID plus the seed. The address mixer is named in receipts as:

```text
split-u64-hash32-avalanche-v1
```

The key invariant is:

```text
particle(i) does not alias particle(i mod 2^32)
```

The verification command exercises the three IDs around the wrap boundary:

```text
4294967295
4294967296
4294967297
```

It requires distinct host fingerprints and, when a GPU is selected, checks GPU initialization against the host reference across that exact boundary.

This is deliberately an addressing construction, not a claim that GALAXY's physical model contains Penrose geometry. The useful Penrose-like idea is only the separation between bounded local pieces and a larger deterministic non-wrapping structure.

## Exact global sampling

A run still exports at most 65,536 particles per snapshot. Sample IDs are selected across the complete logical population with host-side `u128` multiplication before division, then mapped into the tile that owns each global ID.

This avoids both `u32` multiplication overflow and tile-local resampling. CSV files record the actual global 64-bit particle IDs.

Because tiled execution is tile-major, sampled particle states for requested frames are cached until all tiles have completed. The receipt records `sample_cache_bytes` explicitly.

## Commands

Build and list adapters:

```bash
bash scripts/run-u64.sh devices
```

Prove the 32-bit boundary on the host only:

```bash
bash scripts/run-u64.sh verify --cpu
```

Prove it on the first real GPU:

```bash
bash scripts/run-u64.sh verify --adapter 0
```

Validate the supplied beyond-`u32` workload without running it:

```bash
bash scripts/run-u64.sh validate --job runtime/jobs/beyond-u32.json
```

Run it:

```bash
bash scripts/run-u64.sh run \
  --adapter 0 \
  --job runtime/jobs/beyond-u32.json \
  --output runs/beyond-u32-rtx5060ti
```

Output directories must be new, as with the stable runtime.

## Receipts

A successful run records, among other fields:

- logical particle count;
- resident tile particle count;
- tile count;
- resident particle bytes;
- global addressing scheme;
- exact global sample mapping description;
- total integration updates;
- initialization compute wall time;
- integration compute wall time;
- integration particle updates per second;
- frame diagnostics;
- source, job, and artifact hashes.

The receipt distinguishes initialization from leapfrog/circular integration so a many-tile job is not presented as though buffer construction and address generation were free.

## Numerical interpretation

The underlying GPU state remains `float32`, exactly as in the current native runtime. Widening the particle address does **not** widen the orbit arithmetic.

A larger logical population therefore proves a larger deterministic execution domain; it does not by itself improve per-particle numerical precision. Continue to use timestep refinement, trajectory comparison, and the existing sampled angular-momentum diagnostic when assessing integration quality.

## Practical scaling

The default tile size remains the already exercised 8,388,608-particle resident workload. This keeps each particle-state buffer at 256 MiB and stays below the established v0.3 per-field software cap.

The point of the new path is not to allocate billions of particles simultaneously. It is to evolve one globally addressed population through deterministic bounded tiles while retaining exact global identity and reproducible receipts.

For cloud comparisons, use the same job JSON, seed, source commit, and adapter-specific receipt on each device. That makes consumer GPUs and datacenter GPUs comparable without changing the scientific workload.
