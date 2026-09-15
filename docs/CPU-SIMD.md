# GALAXY CPU SIMD probe

## Purpose

The current CPU evidence says GALAXY's first-order bottleneck is particle-memory traffic and locality, not lack of arithmetic throughput. Particle-major traversal produced the largest measured LUT gain, compact particle storage reduced RSS, and a host-native build substantially improved the Float path while barely moving the LUT path.

This probe answers the next narrower question before the production runtime is changed:

> Does a structure-of-arrays (SoA) form of GALAXY's exact integer contribution hash gain enough from CPU autovectorization to justify integrating SIMD-aware tiles into the canonical runtime?

The probe deliberately does **not** claim an end-to-end runtime speedup. It isolates a hot, exact, integer-heavy stage that is friendly to SIMD and can be compared under generic and host-native code generation.

## Why this starts with AVX2 locally

The Ryzen 9 5950X is a Zen 3 CPU. The local machine can exercise AVX2/FMA, but not AVX-512. A later Zen 4/Zen 5 or recent EPYC cloud machine can expose AVX-512 to the same source by rebuilding with `-C target-cpu=native`.

The CPU runtime currently declares Rust 1.85 as its minimum supported Rust version. Stable Rust did not stabilize the x86 AVX-512 intrinsic family until Rust 1.89, so this phase intentionally avoids hard-coded AVX-512 intrinsics. LLVM autovectorization can still select the ISA enabled by `target-cpu=native`, preserving the existing MSRV while we collect evidence.

References:

- Rust `core::arch` CPU feature detection and target-specific optimization: <https://doc.rust-lang.org/stable/core/arch/>
- Rust AVX-512 intrinsic stabilization example (`_mm512_add_epi32`, stable since 1.89): <https://doc.rust-lang.org/stable/core/arch/x86_64/fn._mm512_add_epi32.html>
- AMD dynamic dispatch guidance: Zen 1-3 use AVX2 paths; Zen 4/5 use AVX-512 paths: <https://docs.amd.com/r/en-US/57404-AOCL-user-guide/12.4.1.-Dynamic-Dispatch>

## What `simd_probe` measures

`cpu-runtime/src/bin/simd_probe.rs` builds four homogeneous arrays (`id_lo`, `id_hi`, `x`, `y`) and runs GALAXY's exact `contribution()` hash in a SoA batch loop.

Properties:

- no approximation;
- exact `hash32` constants and wrapping semantics;
- full element-by-element parity check against the reference `galaxy_retro_math::hash32` path before timing;
- repeat checksum stability gate;
- generic and native builds use the same source;
- the exported `galaxy_hash_batch` symbol allows decoded instruction inspection with `objdump`;
- receipts explicitly limit the claim to the isolated hash stage.

The Rust receipt reports runtime AVX2/FMA/AVX-512 availability and stable `cfg(target_feature)` state for AVX2/FMA. It deliberately does **not** infer AVX-512 code generation from `cfg!(target_feature = "avx512f")` on the Rust 1.85 MSRV. AVX-512 code-generation evidence comes from the decoded disassembly emitted by the benchmark runner.

This is intentionally separate from the Float `sin_cos` path and BAM-LUT gather/interpolation path. Those have different vectorization constraints and should not be conflated with a successful hash result.

## Local Ryzen 9 5950X run

From a fresh checkout of the PR branch:

```sh
./scripts/bench-cpu-simd.sh
```

For a larger run matching the previous 8M-resident work scale:

```sh
GALAXY_SIMD_ITEMS=8388608 \
GALAXY_SIMD_REPEATS=7 \
./scripts/bench-cpu-simd.sh
```

`GALAXY_SIMD_REPEATS` is the number of **paired samples**. Both binaries are built before any timing begins. Pair 1 executes generic then native; pair 2 executes native then generic; the order alternates for subsequent pairs. Each timed invocation performs one kernel timing after its exact parity gate. This prevents a fixed generic-first/native-second schedule from being mistaken for a code-generation speedup when temperature, turbo state, or background load drifts.

The generic build is explicitly pinned with `-C target-cpu=x86-64` on x86-64 hosts (or `generic` on other architectures), and any inherited `CARGO_ENCODED_RUSTFLAGS` is removed. The native build is explicitly pinned with `-C target-cpu=native`. An inherited `RUSTFLAGS=-C target-cpu=native` therefore cannot silently turn the generic control into a second native build.

The script creates a timestamped `runs/cpu-simd-*` directory containing:

- `host.txt` with kernel, CPU model, CPU flags, Cargo/rustc versions and effective build flags;
- `generic.json` and `native.json` paired-series summaries;
- `generic-samples.tsv` and `native-samples.tsv` with every paired timing and checksum;
- `sampling-order.tsv` showing the alternating execution order;
- `comparison.txt` with the paired-series medians and native/generic ratio;
- one per-invocation receipt for every generic/native sample;
- disassembly of `galaxy_hash_batch` for both builds when `objdump` is installed;
- `*-isa-evidence.txt` files containing EVEX/VEX prefix counts and decoded packed-integer vector mnemonic counts.

On the 5950X, a useful result is **not** merely seeing an XMM/YMM register name. The native series should show a repeatable median improvement over the pinned generic series while preserving the exact checksum and full parity gate. ISA claims must be based on the decoded instruction evidence: for example, VEX-encoded packed-integer operations provide much stronger AVX2 evidence than register names alone.

## Cloud AVX-512 run

Use a cloud CPU whose guest actually exposes `avx512f`:

```sh
grep -m1 '^model name' /proc/cpuinfo
grep -m1 '^flags' /proc/cpuinfo | tr ' ' '\n' | grep '^avx512' || true
```

Then run the same script unchanged:

```sh
GALAXY_SIMD_ITEMS=8388608 \
GALAXY_SIMD_REPEATS=7 \
./scripts/bench-cpu-simd.sh
```

The runtime receipt confirms whether the guest exposes AVX-512, but that is **not** code-generation evidence. Inspect `native-isa-evidence.txt` and the corresponding disassembly. EVEX-encoded instructions inside `galaxy_hash_batch` are direct AVX-512-family code-generation evidence, including AVX-512VL cases that may use XMM/YMM operands without any ZMM register names. If the native symbol contains no relevant EVEX/vector instructions, do not describe the run as an AVX-512 optimization merely because the host advertises AVX-512.

## Acceptance gate for production integration

Do not integrate SIMD into `galaxy-cpu` solely because the isolated probe is faster. Require all of the following:

1. full exact parity and stable checksum in every paired probe sample;
2. a repeatable native-vs-generic speedup on the 5950X under alternating paired execution;
3. a repeatable result on an AVX-512 cloud CPU, with decoded instruction evidence showing what ISA LLVM selected;
4. an end-to-end prototype combining the already successful particle-major/compact layout with worker-local SoA tiles;
5. unchanged canonical Float and BAM-LUT checksums;
6. wall time and peak RSS measured for the full runtime, not only the probe;
7. a worker sweep so SIMD gains are not confused with memory-bandwidth or NUMA effects.

If those gates pass, the production shape should be **particle-major, compact, worker-local SoA tiles with runtime-safe dispatch/fallback**, not a return to a frame-major full-resident array.
