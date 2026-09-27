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
- no logical-u64 tiling claim: every resident body participates in the same coupled force field.

## Quick start

```sh
cargo test --manifest-path nbody/Cargo.toml --locked --offline
cargo run --manifest-path nbody/Cargo.toml --release --locked --offline -- verify
cargo run --manifest-path nbody/Cargo.toml --release --locked --offline -- \
  run --preset collision --particles 4096 --steps 64 --theta 0.5 \
  --dt-myr 0.05 --softening-kpc 0.05 --seed 303 \
  --receipt runs/barnes-hut/receipt.json \
  --snapshot runs/barnes-hut/final.csv
```

`theta = 0` disables aggregate acceptance and traverses to leaves. `verify` also compares against a separate direct all-pairs implementation so that tree traversal cannot validate itself.

## Claim boundary

The reference uses `f64`. Stable insertion order makes a run repeatable on a fixed build/host, but this phase does **not** claim bit-identical floating-point state across architectures, compilers, or future SIMD/GPU implementations. Receipts are execution evidence, not universal performance claims.
