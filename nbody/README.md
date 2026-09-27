# GALAXY Barnes–Hut self-gravity reference

This crate is the native CPU correctness surface for GALAXY's first mutually-coupled N-body path.

It is intentionally separate from `cpu-runtime/` and `runtime/`: those established paths evolve bounded samples in prescribed gravitational fields and, where applicable, depend on the fact that particles do not exert forces on one another. Barnes–Hut changes that contract.

## Contract

- planar quadtree spatial decomposition;
- three-dimensional Newtonian point-mass force evaluated in the plane;
- Plummer-style softening `r² -> r² + ε²`;
- classical Barnes–Hut acceptance `s / d < theta`;
- a node containing the target body is never accepted as an aggregate;
- deterministic stable quadrant construction from resident body order;
- bounded leaf buckets and depth limit for duplicate/coincident positions;
- kick–drift–kick leapfrog integration;
- exact O(N²) direct reference for verification and sampled error probes;
- no logical-u64 tiling claim: every resident body participates in the same coupled force field;
- a BH #2A pointer-free Morton/Z-order representation with stable resident ordering, flat child indices, contiguous body ranges, and bottom-up cell aggregates.

## Quick start

```sh
cargo test --manifest-path nbody/Cargo.toml --locked --offline
cargo run --manifest-path nbody/Cargo.toml --release --locked --offline -- verify
cargo run --manifest-path nbody/Cargo.toml --release --locked --offline -- verify-flat
cargo run --manifest-path nbody/Cargo.toml --release --locked --offline -- \
  flat-probe --preset collision --particles 16384 --theta 0.5 --seed 303 \
  --receipt runs/barnes-hut-flat/receipt.json
cargo run --manifest-path nbody/Cargo.toml --release --locked --offline -- \
  run --preset collision --particles 4096 --steps 64 --theta 0.5 \
  --dt-myr 0.05 --softening-kpc 0.05 --seed 303 \
  --receipt runs/barnes-hut/receipt.json \
  --snapshot runs/barnes-hut/final.csv
```

`theta = 0` disables aggregate acceptance and traverses to leaves. `verify` also compares against a separate direct all-pairs implementation so that tree traversal cannot validate itself.

## Claim boundary

The reference uses `f64`. Stable insertion order makes a run repeatable on a fixed build/host, but this phase does **not** claim bit-identical floating-point state across architectures, compilers, or future SIMD/GPU implementations. Receipts are execution evidence, not universal performance claims.


## BH #2A flat Morton substrate

The recursive BH #1 tree remains the scientific correctness oracle. BH #2A adds a separate pointer-free representation intended to make later GPU transfer and traversal explicit:

```text
resident bodies
  -> 16-bit-per-axis quantization inside the BH root square
  -> 32-bit Morton/Z-order keys
  -> stable key sort
  -> flat quadtree cells with u32 child indices
  -> bottom-up mass / centre-of-mass aggregation
  -> iterative flat traversal
```

Equal Morton keys preserve original resident order. The topology receipt checksum covers discrete ordering/range/child structure rather than floating aggregate values. The current reference stores f64 aggregates so this phase does not silently introduce a precision change; a future shader phase must define and measure any f32 transfer representation separately.

`flat-probe` reports construction and traversal time independently. Those are CPU measurements of the flat substrate, **not GPU performance evidence**.
