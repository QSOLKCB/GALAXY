# GALAXY retro CPU portability and benchmark

PR #7 extends the isolated retro-deterministic reference layer with a CPU
microbenchmark and multi-architecture validation. It does **not** replace the
production GALAXY GPU runtime or make a performance claim by itself.

## What is being compared

`retro/examples/cpu_bench.rs` measures the same bounded batch of deterministic
angles and Q16.16 radii through three projection paths:

1. **`float_libm`** — convert BAM32 input angles to radians and use native
   floating-point `sin_cos()`.
2. **`bam_cordic_q30`** — use the existing 24-stage integer CORDIC reference and
   Q2.30 projection arithmetic.
3. **`bam_lut_q30`** — use a 16,384-entry BAM lookup table generated from the
   CORDIC reference, with integer linear interpolation between entries.

The input vectors and lookup table are prepared before timing. Each path is
warmed before measurement. The benchmark reports best and median elapsed
nanoseconds, best nanoseconds per sample, best samples per second and an opaque
checksum used to keep the work observable to the optimizer.

The LUT path additionally reports its maximum absolute Q2.30 error against the
CORDIC oracle over a deterministic diagnostic set.

This is intentionally a **projection microbenchmark**, not an end-to-end GALAXY
simulation benchmark. A ratio greater than 1 for `float_over_*_best_ratio` means
the named retro path was faster than the float path on that host for this exact
microbenchmark. It must not be generalized to GPU execution or a different CPU.

## POSIX sh runner

Run the complete CPU validation and benchmark with:

```sh
sh scripts/test-retro-cpu.sh
```

The script is written for POSIX `sh`: it uses `#!/bin/sh`, `set -eu`, portable
`test`/`case` syntax and `$0`-based path resolution. It does not use Bash arrays,
`[[ ... ]]`, `BASH_SOURCE` or `pipefail`.

Defaults:

```text
samples = 1,048,576
repeats = 5
```

Override them without changing the repository:

```sh
GALAXY_CPU_BENCH_SAMPLES=8388608 \
GALAXY_CPU_BENCH_REPEATS=7 \
sh scripts/test-retro-cpu.sh
```

The benchmark implementation bounds samples to 16,777,216 and repeats to 25 so
a malformed environment cannot accidentally turn CI into an unbounded workload.

## GPU wrapper shell boundary

`scripts/test-retro-gpu-local.sh` is also POSIX-sh syntax in this phase and may
be invoked with:

```sh
sh scripts/test-retro-gpu-local.sh
```

That wrapper still calls existing production helpers such as `run-gpu.sh`,
`run-u64.sh` and `build-wasm.sh` with **Bash explicitly**. Those established
helpers are outside this PR's shell-portability boundary. In other words: the
validation wrapper is POSIX `sh`; the complete native GPU toolchain still has a
stated Bash dependency.

## Architecture matrix

`.github/workflows/retro-portability.yml` runs the same deterministic vectors on
native GitHub-hosted machines covering:

- Linux x86-64 (`ubuntu-24.04`)
- Linux ARM64 (`ubuntu-24.04-arm`)
- macOS ARM64 (`macos-15`)
- Windows x86-64 (`windows-2025`)

The Unix jobs execute `scripts/test-retro-cpu.sh` through `/bin/sh` and verify
the actual `uname -m` architecture. Linux additionally checks both POSIX
wrappers with `dash -n` when Dash is available. Windows runs the Rust and
JavaScript deterministic suites and the bounded benchmark directly rather than
pretending to provide a POSIX shell.

GitHub's current hosted-runner reference documents native ARM64 labels including
`ubuntu-24.04-arm` and ARM64 macOS runners:

https://docs.github.com/en/actions/reference/runners/github-hosted-runners

## Interpretation boundary

The goals are deliberately separate:

- **Determinism:** the existing golden vectors must agree across languages and
  architectures.
- **Portability:** the isolated Rust/JavaScript reference implementation must
  build and execute on x86-64 and ARM64 without architecture-specific source.
- **Performance exploration:** collect timing evidence for float, CORDIC and LUT
  CPU paths without asserting beforehand which one wins.

A faster LUT or CORDIC result on a CPU does not imply a faster GPU implementation.
Conversely, a slower CORDIC result does not reduce its value as the exact integer
reference oracle introduced by PR #6.
