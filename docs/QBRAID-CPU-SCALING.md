# qBraid native CPU scaling replication

## Purpose

This experiment follows the merged GALAXY native CPU runtime in PR #8 and the local Ryzen 9 5950X validation. It is a cloud replication and scaling-shape experiment, not a claim that qBraid vCPUs are equivalent to physical Ryzen cores.

The main question is whether the large-resident BAM32 LUT plateau observed locally persists when GALAXY is given substantially more than 32 cloud vCPUs, while the Float backend continues to scale.

### Preferred qBraid targets

Use CPU instances only for this experiment.

1. **Preferred topology-discrimination target:** Nanoacademic - Medium — **96 vCPU / 384 GB RAM**.
   - Listed price observed by the user: **9.60 credits/minute**.
   - This profile exists primarily for Nanoacademic nanodcal / RESCU workloads, but it is useful here only if it exposes a normal shell plus Git, Rust and Cargo.
   - Its scientific value is the ability to test the additional worker checkpoints `48`, `64`, and `96` beyond the local 32-thread Ryzen ceiling.
2. **Cheaper fallback:** generic CPU — **64 vCPU / 256 GB RAM**.
   - Listed price observed by the user: **6.40 credits/minute**.
   - This can test 32 -> 64 scaling, but not 64 -> 96.
3. **Replication-only fallback:** generic CPU — **32 vCPU / 128 GB RAM**.
   - This can reproduce the local worker ceiling but cannot discriminate what happens above 32 workers.

Do **not** use a GPU profile for this CPU-scaling experiment.

If the Nanoacademic image does not expose the required normal Linux/Rust environment, stop after preflight and use the generic 64-vCPU CPU profile instead. Do not mutate the lab image or use privilege escalation merely to force GALAXY onto it.

---

## Local reference evidence

The reference local validation tested merge commit:

```text
b9e61d20d0fe0fa99f302a2ed13aa1215a60c5f3
```

Host:

```text
AMD Ryzen 9 5950X 16-Core Processor
32 available logical CPUs
Ubuntu 26.04.1 LTS / Linux 7.0.0-31-generic x86_64
```

The local validation reached a resident population of **16,777,216** with deterministic checksum parity. The evidence archive passed `unzip -t`, and the source repository remained clean.

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

The local worker matrix showed a repeatable qualitative split:

- Float generally continued improving through 32 workers.
- BAM-LUT scaled strongly at low worker counts but plateaued much earlier for the largest resident sets.
- 32 workers versus 16 workers was workload-dependent for BAM-LUT.
- BAM-LUT remained faster than Float at the largest tested working set.
- The local run did not collect hardware performance counters, so the plateau is only **consistent with** cache, memory-bandwidth, NUMA or related topology pressure; its cause is not established.

The qBraid experiment is intended to test whether that scaling shape survives on a different CPU topology and, on the preferred profile, through **48 / 64 / 96 workers**.

---

## 1. Cost discipline

qBraid credits are finite. Scientific evidence per credit is the objective.

For the 96-vCPU Nanoacademic profile at the listed 9.60 credits/minute, avoid spending time on low-value repetition. Use the staged protocol below and stop once the topology question is answered.

Do not launch duplicate instances or concurrent GALAXY benchmark processes.

Use this order:

1. environment and topology preflight;
2. repository and correctness gate;
3. focused large-resident worker sweep;
4. full worker matrix only if scientifically useful;
5. one high-confidence confirmation run;
6. optional read-only counter probe if already permitted;
7. archive evidence and terminate the instance.

Do not leave the paid instance idle while writing prose or reorganizing files.

---

## 2. Fresh checkout and provenance

Clone fresh:

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

If `main` has advanced, do not reset backwards. Test current `main`, record the actual SHA, and note whether later commits changed `cpu-runtime/`, `retro/`, `rust/`, `scripts/test-cpu-runtime.sh`, or this qBraid protocol.

Never modify source merely to make a benchmark green.

---

## 3. Environment and topology preflight

Run this before spending meaningful credits:

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
- whether the Nanoacademic image exposes Git, Rust and Cargo normally.

Do not assume `96 vCPU` means 96 physical cores, 48 cores with SMT, or any other physical topology. Treat it as an observed cloud execution contract unless the guest exposes stronger evidence.

Do not change affinity, NUMA policy, power governors, kernel settings, or security policy.

If the Nanoacademic profile lacks a usable normal Rust environment, preserve the preflight result, stop that instance, and use the 64-vCPU generic CPU profile.

---

## 4. Correctness gate

Run:

```sh
cargo test --manifest-path cpu-runtime/Cargo.toml --locked --offline
cargo build --manifest-path cpu-runtime/Cargo.toml --release --locked --offline
```

Require:

```text
cpu-runtime/target/release/galaxy-cpu
```

Then run:

```sh
cpu-runtime/target/release/galaxy-cpu --help
cpu-runtime/target/release/galaxy-cpu verify --workers 96
```

If the selected instance exposes fewer effective CPUs, the runtime must report that honestly. Do not fake 96 effective workers.

Require deterministic u64 boundary evidence and scalar/parallel checksum parity.

Expected sampled LUT diagnostic on the current implementation:

```text
lut_error_sample_count=8193
lut_sampled_max_abs_q30_error=255
```

This is sampled evidence, not a mathematical global bound.

If correctness fails, stop performance interpretation.

---

## 5. Focused topology-discrimination sweep

On the **96-vCPU preferred profile**, start with the scientifically important large resident sets rather than immediately spending credits on the full matrix.

Use:

```text
logical = 18446744073709551615
frames  = 8
repeats = 5
seed    = 303
```

Resident populations:

```text
8,388,608
16,777,216
```

Worker counts:

```text
16
32
48
64
96
```

These checkpoints directly ask whether the local large-set BAM-LUT plateau moves, disappears, or persists beyond 32 workers.

Example:

```sh
cpu-runtime/target/release/galaxy-cpu bench \
  --logical 18446744073709551615 \
  --resident 16777216 \
  --frames 8 \
  --workers 96 \
  --repeats 5 \
  --seed 303 \
  --receipt runs/qbraid-cpu-r16777216-w96.json
```

Run sequentially. Never benchmark multiple GALAXY processes concurrently.

Every benchmark must report:

```text
backend_timing_schedule=interleaved-alternating-v1
```

Do not intentionally flush caches, add sleeps to manipulate results, or change affinity between worker counts.

---

## 6. Full matrix if the focused sweep is useful

If the focused sweep reveals a meaningful change around 32-96 workers, fill in the broader matrix.

Resident populations:

```text
1,048,576
8,388,608
16,777,216
```

Worker counts on 96-vCPU:

```text
1
2
4
8
16
32
48
64
96
```

On the 64-vCPU fallback, stop at 64 workers. On the 32-vCPU fallback, stop at 32 workers.

Use a unique receipt for every run.

---

## 7. High-confidence confirmation

After the matrix, run one confirmation at the largest resident population and largest **useful** worker count established by the sweep.

For a successful 96-vCPU experiment, the default confirmation candidate is:

```text
logical   = 18446744073709551615
resident  = 16777216
frames    = 8
workers   = 96
repeats   = 7
seed      = 303
```

If 96 workers are slower than 64 or 32, that is a valid result. Also run a 7-repeat confirmation at the best-performing worker count if different, provided the credit budget allows it.

Do not silently relabel a lower effective worker count as 96.

---

## 8. Optional counter and NUMA evidence

The local Ryzen result did not contain hardware-counter evidence. If qBraid already exposes `perf` to the unprivileged session, collect a small read-only probe without changing host configuration.

Example, if permitted:

```sh
perf stat -e cycles,instructions,cache-references,cache-misses \
  cpu-runtime/target/release/galaxy-cpu bench \
  --logical 18446744073709551615 \
  --resident 16777216 \
  --frames 8 \
  --workers 32 \
  --repeats 1 \
  --seed 303 \
  --receipt runs/qbraid-perf-w32.json
```

If the 32-worker probe succeeds, optionally repeat at 64 and 96 workers.

Treat whole-process counters as supporting context only because the process contains Float and BAM-LUT paths plus warm-ups. Do not attribute all process-level misses solely to BAM-LUT.

If `perf` is unavailable or permission is denied, record that and continue. Do not use sudo or change perf security settings.

If multiple NUMA nodes are visible, record the topology but do not manually pin or rebalance the process for the canonical comparison.

---

## 9. Analysis questions

Answer from receipts and topology evidence:

1. Does Float continue scaling from 32 -> 48 -> 64 -> 96 workers?
2. Does BAM-LUT improve from 32 -> 48 -> 64 -> 96 at 8M and 16M resident particles?
3. At what worker count does BAM-LUT achieve its best median for each resident population?
4. Does the BAM-LUT plateau begin earlier as the resident set grows?
5. Does the LUT-vs-Float advantage shrink as worker count rises?
6. Does the 96-vCPU topology reproduce the qualitative local 5950X pattern?
7. Does the 96-vCPU profile reveal another scaling regime beyond 64 workers?
8. Is any transition correlated with reported NUMA boundaries, cache topology, CPU quota, or contention?

For each resident population calculate from recorded medians:

```text
speedup(N) = one-worker parallel median / N-worker parallel median
efficiency(N) = speedup(N) / N
```

Do this separately for Float and BAM-LUT.

Do not describe 32 -> 64 or 64 -> 96 behaviour as SMT evidence unless guest topology independently establishes SMT sibling relationships.

---

## 10. Cross-host comparison with the Ryzen 9 5950X

Compare **scaling shape** first, not absolute speed.

Important local reference points:

```text
8,388,608 resident:
  Float:   16 workers 131,569,472 ns; 32 workers 104,221,405 ns
  BAM-LUT: 16 workers  85,462,130 ns; 32 workers  82,951,850 ns

16,777,216 resident:
  Float:   16 workers 250,603,920 ns; 32 workers 200,677,048 ns
  BAM-LUT:  8 workers 177,962,263 ns
            16 workers 181,266,944 ns
            32 workers 178,414,303 ns
```

Interpretation boundary:

- Float retained substantial local scaling from 16 to 32 workers.
- BAM-LUT gained little or no additional throughput beyond roughly 8-16 workers at the largest local resident sets.
- This is consistent with cache, memory-bandwidth, NUMA or topology limits, but the local run did not isolate causation.

The 96-vCPU qBraid experiment should determine whether a different cloud CPU/cache/NUMA topology shifts that plateau and whether any useful scaling remains from 64 to 96 workers.

---

## 11. Evidence package

Create a unique external report directory such as:

```text
~/GALAXY-qbraid-cpu-YYYYMMDDTHHMMSSZ-PID/
```

Preserve:

- exact tested Git SHA;
- qBraid profile identity;
- host/topology/cgroup information;
- Cargo test/build logs;
- verifier output;
- every JSON receipt;
- complete benchmark logs;
- optional perf output;
- machine-readable summary CSV;
- worker/resident scaling table;
- checksum-consistency table;
- final Markdown report;
- `SHA256SUMS.txt`.

Create a ZIP and validate it with `unzip -t`.

Do not commit benchmark artifacts to GALAXY.

---

## 12. Claim boundary

Do not claim:

- 96 vCPUs are 96 physical cores;
- 32/48/64/96 scaling proves SMT without topology evidence;
- BAM-LUT is proven memory-bandwidth bound without appropriate counter evidence;
- `u64::MAX` particles were simultaneously allocated;
- this is an N-body simulation;
- the CPU runtime beats the GPU runtime end to end;
- qBraid performance generalizes to all Azure or cloud CPUs;
- the sampled LUT diagnostic is a global error proof;
- the Nanoacademic profile is scientifically special for GALAXY merely because Nanoacademic software is preinstalled.

Allowed wording, when supported:

> On this qBraid CPU instance, under this exact GALAXY commit and workload, backend X achieved Y measured scaling from A to B workers.

> The observed BAM-LUT scaling plateau is consistent with cache, memory-bandwidth, NUMA or topology limits, but the available evidence does not isolate the cause.

---

## Definition of success

A strong 96-vCPU result establishes:

1. exact tested source identity;
2. observed qBraid profile, CPU/vCPU/NUMA/cache topology and quotas;
3. native CPU runtime correctness and deterministic checksum parity;
4. successful measurements through 32, 48, 64 and 96 requested workers, with actual effective workers preserved;
5. a direct answer to whether Float and BAM-LUT gain anything beyond 32 and beyond 64;
6. comparison with the local Ryzen 9 5950X scaling shape without pretending the systems are equivalent;
7. complete, hashed evidence archive.

Be aggressive with the available CPU capacity, conservative with causal claims, and meticulous with receipts.