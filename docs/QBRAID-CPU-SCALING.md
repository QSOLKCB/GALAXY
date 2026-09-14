# qBraid native CPU scaling replication

## Purpose

This document serves two roles:

1. a reproducible protocol for future qBraid native-CPU scaling runs; and
2. the scientific record of the completed qBraid EPYC 7763 experiment performed manually after GALAXY PR #8 merged.

The experiment follows the merged GALAXY native CPU runtime and the local Ryzen 9 5950X validation. It is a cloud replication and scaling-shape experiment, not a claim that cloud vCPUs are equivalent to physical Ryzen cores.

The central question is whether the large-resident BAM32 LUT plateau observed locally persists, moves, or disappears on a wider cloud CPU topology while the Float backend continues to scale.

### Preferred qBraid targets

Use CPU instances only for this experiment.

1. **Preferred topology-discrimination target:** Nanoacademic - Medium — **96 vCPU / 384 GB RAM**.
   - Listed price observed by the user: **9.60 credits/minute**.
   - The image exists primarily for Nanoacademic nanodcal / RESCU workloads, but GALAXY uses it only as a CPU host.
   - Its scientific value is the ability to test `32 -> 48 -> 64 -> 96` workers.
2. **Cheaper fallback:** generic CPU — **64 vCPU / 256 GB RAM**.
   - Listed price observed by the user: **6.40 credits/minute**.
   - It can test through 64 workers.
3. **Replication-only fallback:** generic CPU — **32 vCPU / 128 GB RAM**.
   - It can reproduce the local worker ceiling but cannot discriminate behaviour above 32 workers.

Do **not** use a GPU profile for this CPU-scaling experiment.

---

## Local Ryzen 9 5950X reference

Reference commit:

```text
b9e61d20d0fe0fa99f302a2ed13aa1215a60c5f3
```

Host:

```text
AMD Ryzen 9 5950X 16-Core Processor
32 available logical CPUs
Ubuntu 26.04.1 LTS / Linux 7.0.0-31-generic x86_64
```

The local validation reached a resident population of **16,777,216** with deterministic checksum parity.

Primary 8,388,608-resident / 32-worker / 7-repeat result:

```text
Float scalar median:      1,623,055,331 ns
Float parallel median:       97,367,124 ns
Float measured speedup:      16.669438968 x

BAM-LUT scalar median:      504,572,701 ns
BAM-LUT parallel median:     84,556,211 ns
BAM-LUT measured speedup:     5.967305004 x

Float/BAM-LUT scalar ratio:   3.216692714 x
Float/BAM-LUT parallel ratio: 1.151507652 x
```

The local matrix showed a repeatable qualitative split:

- Float generally continued improving through 32 workers.
- BAM-LUT scaled strongly at low worker counts but plateaued much earlier for the largest resident sets.
- 32 workers versus 16 workers was workload-dependent for BAM-LUT.
- BAM-LUT remained faster than Float at the largest tested working set.
- No hardware-counter evidence was collected, so the plateau was only **consistent with** cache, memory-bandwidth, NUMA or related topology pressure; its cause was not established.

---

# Reproducible qBraid protocol

## 1. Environment and topology preflight — run before checkout

The environment preflight comes first. Do not attempt the repository checkout until the shell, Git and usable Rust environment have been established or a user-local Rust bootstrap has been explicitly accepted.

Run:

```sh
pwd
date -u
uname -a
cat /etc/os-release || true
lscpu
lscpu -e || true
lscpu -C || true
nproc
getconf _NPROCESSORS_ONLN || true
free -h
cat /proc/meminfo
cat /proc/self/status
cat /proc/self/cgroup
cat /sys/devices/system/node/online 2>/dev/null || true
numactl --hardware 2>/dev/null || true
command -v git || true
command -v cargo || true
command -v rustc || true
command -v sh || true
git --version || true
cargo --version || true
rustc --version --verbose || true
```

Inspect read-only cgroup/cpuset information when present:

```sh
find /sys/fs/cgroup -maxdepth 2 -type f \
  \( -name 'cpu.max' -o -name 'cpuset.cpus' -o -name 'cpuset.cpus.effective' \) \
  -print -exec cat {} \; 2>/dev/null || true
```

Record:

- exact qBraid profile name;
- CPU model string;
- virtualization/hypervisor if reported;
- sockets, cores, threads and NUMA nodes visible to the guest;
- L1/L2/L3 cache information if exposed;
- OS-visible logical CPU count;
- process CPU affinity/cpuset;
- CPU quota if any;
- current system load;
- whether Git, Rust and Cargo are available normally.

Do not infer physical topology from the marketed vCPU number when the guest does not expose it.

If Rust is absent but a user-local installation is permitted, use the repository-pinned toolchain rather than replacing system packages. GALAXY currently pins Rust `1.85.1` in `rust-toolchain.toml`; rustup may automatically install that pinned toolchain after checkout even if the user's default toolchain is newer.

Do not use sudo or change power, kernel, cgroup or security settings merely to make the benchmark run.

---

## 2. Fresh checkout and provenance

After preflight succeeds:

```sh
git clone https://github.com/QSOLKCB/GALAXY.git
cd GALAXY
git checkout main
git fetch --prune origin
git status --short --branch
git rev-parse HEAD
git log -1 --decorate --oneline
```

Historical PR #8 merge commit:

```text
b9e61d20d0fe0fa99f302a2ed13aa1215a60c5f3
```

If `main` has advanced, do not reset backwards. Test current `main`, record the actual SHA, and note whether later commits changed the runtime or benchmark protocol.

Never modify source merely to make a benchmark green.

---

## 3. Correctness gate

Run:

```sh
cargo test --manifest-path cpu-runtime/Cargo.toml --locked --offline
cargo build --manifest-path cpu-runtime/Cargo.toml --release --locked --offline
```

Require:

```text
cpu-runtime/target/release/galaxy-cpu
```

Then run verification with no more workers than the selected profile exposes:

```sh
cpu-runtime/target/release/galaxy-cpu --help
cpu-runtime/target/release/galaxy-cpu verify --workers 96
```

For the `verify` command, the expected diagnostic keys are exactly:

```text
verify_lut_error_sample_count=8193
verify_lut_sampled_max_abs_q30_error=255
```

The benchmark receipts use the corresponding unprefixed JSON field names. Do not confuse the verifier output with the benchmark receipt schema.

Require deterministic u64 boundary evidence and scalar/parallel checksum parity. The LUT diagnostic is sampled evidence, not a mathematical global bound.

If correctness fails, stop performance interpretation.

---

## 4. Focused topology-discrimination sweep

Use:

```text
logical = 18446744073709551615
frames  = 8
repeats = 5
seed    = 303
```

Start with scientifically useful resident populations, normally:

```text
8,388,608
16,777,216
```

Every focused sweep includes a **1-worker baseline** so later speedup and efficiency calculations remain defined even if the optional full matrix is skipped.

Profile-specific focused ladders:

```text
96-vCPU preferred:  1, 16, 32, 48, 64, 96
64-vCPU fallback:   1, 16, 32, 48, 64
32-vCPU fallback:   1, 8, 16, 32
```

If a profile exposes fewer effective CPUs than its marketed name, truncate the ladder to the actual capacity and record that fact.

Example:

```sh
cpu-runtime/target/release/galaxy-cpu bench \
  --logical 18446744073709551615 \
  --resident 8388608 \
  --frames 8 \
  --workers 48 \
  --repeats 5 \
  --seed 303 \
  --receipt runs/qbraid-cpu-r8388608-w48.json
```

Run sequentially. Never benchmark multiple GALAXY processes concurrently.

Every benchmark must report:

```text
backend_timing_schedule=interleaved-alternating-v1
```

Do not intentionally flush caches, add sleeps to manipulate results, or change affinity during the canonical unpinned scaling sweep.

---

## 5. Optional full matrix

If the focused sweep exposes a scientifically useful transition, fill in the broader matrix.

Resident populations:

```text
1,048,576
8,388,608
16,777,216
```

Worker ladders:

```text
96-vCPU: 1, 2, 4, 8, 16, 32, 48, 64, 96
64-vCPU: 1, 2, 4, 8, 16, 32, 48, 64
32-vCPU: 1, 2, 4, 8, 16, 32
```

Use a unique receipt for every run.

---

## 6. High-confidence confirmation

After the sweep, run a 7-repeat confirmation at the best-performing worker count for the important resident size.

If the highest worker count is slower than an earlier point, that is a valid result. Do not silently promote the highest worker count to the 'best' result.

---

## 7. Optional counter and NUMA evidence

If qBraid exposes `perf` to the unprivileged session, small read-only probes may provide context. Do not use sudo or change perf security policy.

If multiple NUMA nodes and SMT siblings are visible, preserve:

```sh
lscpu -e=CPU,NODE,SOCKET,CORE,ONLINE
for f in /sys/devices/system/cpu/cpu*/topology/thread_siblings_list; do
  printf '%s: ' "$f"
  cat "$f"
done
```

Canonical performance comparisons should remain unpinned. Affinity experiments may be added afterward as explicitly separate topology probes.

For every future affinity probe, preserve the requested mask **and** capture the kernel-visible allowed CPU list from inside the constrained process before launching `galaxy-cpu`. A reproducible pattern is:

```sh
MASK='0-47'
AFFINITY_LOG='runs/qbraid-cpu/r8388608-w48-node0-affinity.txt'
RECEIPT='runs/qbraid-cpu/r8388608-w48-node0.json'

taskset -c "$MASK" sh -c '
  printf "requested_mask=%s\n" "$1"
  grep "^Cpus_allowed_list:" /proc/self/status
  taskset -pc $$ 2>/dev/null || true
  exec cpu-runtime/target/release/galaxy-cpu bench \
    --logical 18446744073709551615 \
    --resident 8388608 \
    --frames 8 \
    --workers 48 \
    --repeats 7 \
    --seed 303 \
    --receipt "$2"
' sh "$MASK" "$RECEIPT" 2>&1 | tee "$AFFINITY_LOG"
```

Retain the affinity log beside the JSON receipt. The `Cpus_allowed_list` line is the per-run evidence that binds the benchmark process to the intended Linux affinity mask.

---

## 8. Analysis

For each resident population with a one-worker baseline calculate:

```text
speedup(N) = one-worker parallel median / N-worker parallel median
efficiency(N) = speedup(N) / N
```

Do this separately for Float and BAM-LUT.

If a one-worker baseline was not collected for an historical run, do **not** manufacture one from scalar timings. Report only the runtime's own measured scalar/parallel speedup and direct worker-to-worker ratios that the receipts support.

When guest topology explicitly establishes SMT sibling relationships, worker-count transitions may be discussed relative to that exposed topology. Do not claim actual thread occupancy without affinity or scheduler evidence.

---

# Completed qBraid EPYC 7763 evidence

## 9. Source and host identity

The completed qBraid experiment was performed manually; no qBraid AI agent was required.

Tested GALAXY commit:

```text
b9e61d20d0fe0fa99f302a2ed13aa1215a60c5f3
```

Observed host:

```text
Linux 6.8.0-1059-azure
AMD EPYC 7763 64-Core Processor
Microsoft full virtualization
96 online logical CPUs
1 socket
48 exposed cores
2 threads per exposed core
2 NUMA nodes
NUMA node 0: CPUs 0-47
NUMA node 1: CPUs 48-95
L2: 24 MiB total across 48 instances
L3: 192 MiB total across 6 instances
377 GiB RAM
no swap
```

The guest topology explicitly reported adjacent SMT siblings:

```text
0-1, 2-3, 4-5, ... , 94-95
```

Thus this particular guest does expose 48 cores / 96 logical CPUs with two threads per core. That still does not prove which CPUs an unpinned worker occupied at any instant.

The correctness gate passed with:

```text
available_parallelism=96
effective_workers=96
verify_u64_boundary=88a9cb0a,6551a647,1ec272fb
verify_lut_error_sample_count=8193
verify_lut_sampled_max_abs_q30_error=255
GALAXY native CPU runtime verification passed
```

---

## 10. Unpinned 8,388,608-resident scaling

Common configuration:

```text
logical population = 18446744073709551615
resident particles = 8388608
frames             = 8
repeats            = 5
seed               = 303
schedule           = interleaved-alternating-v1
```

| Workers | Float parallel median | Float measured speedup | BAM-LUT parallel median | BAM-LUT measured speedup | Float/LUT parallel ratio |
|---:|---:|---:|---:|---:|---:|
| 32 | 121,016,057 ns | 20.297370257x | 45,064,074 ns | 16.647936536x | 2.6854x |
| **48** | **77,071,937 ns** | **31.710744042x** | **31,460,778 ns** | **24.102310343x** | **2.4498x** |
| 64 | 92,227,565 ns | 26.651559173x | 38,580,695 ns | 19.533954559x | 2.3905x |
| 96 | 81,488,044 ns | 29.923119311x | 35,517,853 ns | 21.221956350x | 2.2943x |

All four worker counts preserved identical deterministic checksums for the same workload:

```text
Float   = adf6d6e30d3ad26d
BAM-LUT = 8d6f07bd77e2fc16
```

Observed shape:

- 32 -> 48 improved Float parallel time by about 36.3% and BAM-LUT by about 30.2%.
- 48 -> 64 regressed by about 19.7% for Float and 22.6% for BAM-LUT.
- 48 -> 96 also remained slower by about 5.7% for Float and 12.9% for BAM-LUT.
- 48 workers was the best measured point for both backends in this 8M unpinned sweep.
- The LUT advantage over Float narrowed as worker count increased.

These are environment-specific measurements, not universal EPYC scaling claims.

---

## 11. Repeat-matched 48-worker confirmation and affinity probes

A 7-repeat unpinned confirmation produced:

```text
Float parallel median   = 77,223,700 ns
Float measured speedup  = 31.825052659x
BAM-LUT parallel median = 31,375,612 ns
BAM-LUT measured speedup= 23.712758081x
```

This closely reproduced the 5-repeat 48-worker result.

Three 7-repeat affinity experiments then isolated topology effects:

| 48-worker configuration | Available CPUs seen by runtime | Float parallel | BAM-LUT parallel | Float vs unpinned | LUT vs unpinned |
|---|---:|---:|---:|---:|---:|
| Unpinned | 96 | **77,223,700 ns** | **31,375,612 ns** | baseline | baseline |
| Node 0 only, CPUs `0-47` | 48 | 78,122,207 ns | 33,014,629 ns | +1.2% | +5.2% |
| Node 1 only, CPUs `48-95` | 48 | 77,098,962 ns | 34,328,507 ns | -0.2% | +9.4% |
| One SMT thread per exposed core across both nodes, `0,2,...,94` | 48 | 115,543,237 ns | 47,844,460 ns | **+49.6%** | **+52.5%** |

All affinity runs preserved the same deterministic backend checksums.

### Affinity-mask provenance for the completed runs

The exact shell commands and requested CPU masks used for these completed probes are preserved in:

```text
evidence/qbraid/EPYC7763-20260914/AFFINITY-MANIFEST.md
```

That manifest binds each affinity receipt filename to its `taskset` invocation and expanded mask:

```text
r8388608-w48-unpinned-r7.json        -> no taskset mask
r8388608-w48-node0.json              -> taskset -c 0-47
r8388608-w48-node1.json              -> taskset -c 48-95
r8388608-w48-physical-pinned.json    -> taskset -c 0,2,4,...,94
```

Evidence limitation: the original `galaxy.cpu-runtime-receipt.v1` schema does not record a Linux CPU-affinity mask, and the original compressed evidence bundle did not capture `Cpus_allowed_list` from inside each constrained process. Therefore the receipt JSON files alone substantiate the timings, worker counts and checksum parity, while the exact mask-to-receipt binding is retrospective operator command provenance preserved in `AFFINITY-MANIFEST.md`. The table above should be read as **runs invoked with those masks**, not as a claim that the current receipt schema independently encoded or re-observed them.

Important interpretation:

- The run invoked with the node-0 mask retained near-baseline performance even though that mask covered 24 exposed cores / 48 logical CPUs.
- The run invoked with the node-1 mask also retained near-baseline Float performance and modestly slower BAM-LUT performance.
- The run invoked with one logical CPU from each exposed core across both NUMA nodes was roughly 50% slower for both backends.
- The result strongly associates the performance loss with the requested cross-NUMA/topology configuration rather than with correctness or the deterministic work itself.
- It is **consistent with** NUMA memory-locality / first-touch / remote-memory effects, but no hardware memory-traffic counters were collected, so that mechanism is not proven.
- The unpinned 48-worker optimum must not be described simply as '48 physical cores occupied'; unpinned scheduler placement was not observed directly.

This finding is relevant to future persistent-worker-pool work: topology and memory placement should be evaluated before assuming that more distinct physical cores or more threads monotonically improve the CPU runtime.

---

## 12. Evidence archive

Curated raw evidence for this completed experiment is stored under:

```text
evidence/qbraid/EPYC7763-20260914/
```

The original compressed evidence bundle has SHA-256:

```text
5c0474b9537a0ee34493a58c22b368f4846ad12f41633430d4444a678561e87a
```

The bundle contains:

- host topology capture;
- SMT sibling mapping;
- 32/48/64/96 unpinned receipts;
- 7-repeat unpinned 48-worker confirmation;
- node-0 and node-1 48-worker affinity receipts;
- one-thread-per-core cross-NUMA 48-worker affinity receipt.

The repository directory also contains `AFFINITY-MANIFEST.md`, which preserves the exact commands and requested masks used for the affinity probes and documents the retrospective provenance limitation described above.

---

## 13. Claim boundary

Do not claim:

- 96 vCPUs are 96 physical cores;
- unpinned 48 workers means all 48 exposed cores were occupied;
- the original receipt JSON independently proves the exact Linux affinity mask used by an historical affinity run;
- BAM-LUT is proven memory-bandwidth bound;
- the affinity result proves a specific first-touch or remote-memory mechanism without counters;
- `u64::MAX` particles were simultaneously allocated;
- this is an N-body simulation;
- CPU beats GPU end to end;
- qBraid performance generalizes to all Azure or EPYC hosts;
- the sampled LUT diagnostic is a global error proof;
- the Nanoacademic software stack contributed to GALAXY performance merely because it was preinstalled.

Supported wording includes:

> On this qBraid EPYC 7763 guest, GALAXY's 8,388,608-resident workload measured its best unpinned 32/48/64/96-worker result at 48 workers for both Float and BAM-LUT, while preserving deterministic checksums.

> According to the preserved operator command provenance, the NUMA-local 48-thread affinity runs retained near-unpinned performance, whereas the run invoked with a one-thread-per-exposed-core mask spanning both NUMA nodes was approximately 50% slower. This is consistent with a strong topology/memory-locality effect, but the historical receipt schema did not encode the affinity mask and the available evidence does not isolate the underlying memory-traffic mechanism.

---

## Definition of success for future replication

A strong future result establishes:

1. exact tested source identity;
2. observed CPU/vCPU/core/SMT/NUMA/cache topology and quotas;
3. native CPU runtime correctness and deterministic checksum parity;
4. a focused profile-appropriate sweep including a one-worker baseline;
5. actual effective workers preserved in every receipt;
6. optional topology probes kept separate from the canonical unpinned sweep;
7. every affinity probe accompanied by an in-process `Cpus_allowed_list` capture and exact invocation;
8. complete hashed evidence.

Be aggressive with available CPU capacity, conservative with causal claims, and meticulous with receipts.
