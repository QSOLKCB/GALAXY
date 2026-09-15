# Guarded worker-local SoA integration

PR #12 integrates the worker-local compact SoA execution path into the production `galaxy-cpu` binary without changing the canonical default runtime.

## Command boundary

The established commands remain unchanged:

```sh
galaxy-cpu verify
galaxy-cpu bench
```

The worker-local SoA path requires explicit opt-in:

```sh
galaxy-cpu verify-soa
galaxy-cpu bench-soa
```

`bench-soa` cannot be redirected back to the reference path with `--path`; the command itself is the execution-mode selection. The canonical `bench` command therefore remains the oracle and fallback.

## Default tile

The guarded integration defaults to a 1,024-particle tile when `--tile` is omitted.

This is an evidence-backed starting point, not a universal optimum. PR #11 showed strong results across multiple worker counts and hosts, while the wider sweep demonstrated that larger tiles can remain competitive depending on host topology and concurrency. Callers can continue to select a different positive tile size explicitly.

## Example

```sh
cargo run --manifest-path cpu-runtime/Cargo.toml --release --locked --offline -- \
  bench-soa \
  --logical 18446744073709551615 \
  --resident 1048576 \
  --frames 8 \
  --workers 32 \
  --tile 1024 \
  --repeats 5 \
  --seed 303 \
  --receipt runs/guarded-soa/receipt.json
```

For the canonical fallback/oracle using the same logical workload:

```sh
cargo run --manifest-path cpu-runtime/Cargo.toml --release --locked --offline -- \
  bench \
  --logical 18446744073709551615 \
  --resident 1048576 \
  --frames 8 \
  --workers 32 \
  --repeats 5 \
  --seed 303 \
  --receipt runs/canonical/receipt.json
```

## Verification

`verify-soa` runs the PR #11 parity suite from inside the production `galaxy-cpu` binary surface. It checks:

- exact reference-vs-SoA BAM-LUT checksum parity;
- awkward tile boundaries;
- deterministic worker partition/reduction;
- u64 address handling;
- the SIMD-friendly contribution batch against the canonical contribution hash.

CI runs both the standalone PR #11 probe and the integrated `verify-soa` command on Linux x86-64, Linux ARM64, macOS ARM64, and Windows x86-64.

## Receipt identity

Integrated SoA receipts use:

```text
schema = galaxy.cpu-runtime-soa-receipt.v1
runtime = galaxy-cpu
execution_mode = worker-local-soa-guarded
guarded_opt_in = true
canonical_fallback = bench
```

The receipt includes the selected tile size, effective worker count, exact checksum, timing, actual worker-local tile capacity, and Linux `VmHWM` peak RSS where available.

## Safety / promotion boundary

This PR does **not** silently make SoA the default.

The integration remains guarded because the performance evidence is host-specific even though deterministic parity is cross-platform. A later phase may consider changing defaults only if additional production evidence shows that doing so is beneficial across the supported hardware envelope.

Until then:

- `bench` / `verify` = canonical default and oracle;
- `bench-soa` / `verify-soa` = explicit optimized path;
- checksum disagreement is a correctness failure, never an acceptable performance trade.
