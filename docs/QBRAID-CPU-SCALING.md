# qBraid native CPU scaling replication

## Purpose

This experiment follows the merged GALAXY native CPU runtime in PR #8 and the local Ryzen 9 5950X validation. It is a cloud replication and scaling-shape experiment, not a claim that qBraid vCPUs are equivalent to physical Ryzen cores.

The main question is whether the large-resident BAM32 LUT plateau observed locally persists when the runtime is given more than 32 cloud vCPUs, while the Float backend continues to scale.

The preferred qBraid target is the on-demand CPU profile exposing **64 vCPU / 256 GB RAM**. A 32-vCPU instance cannot answer the 32-versus-64 scaling question.

Do not use a GPU profile for this experiment.

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

The local validation reached a resident population of 16,777,216 with deterministic checksum parity.

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
- The local run did not collect hardware performance counters, so the plateau is only **consistent with** cache/memory-bandwidth pressure; it is not proven to be caused by it.

The qBraid run is intended to test whether that shape survives on a different CPU topology and beyond 32 workers.

---

## 1. Cost discipline

qBraid credits are finite. The 64-vCPU CPU instance should be used only for this CPU study.

Do not launch duplicate instances or concurrent benchmark processes.

Use this order:

1. hardware/topology preflight;
2. repository and correctness gate;
3. 64-vCPU worker-scaling matrix;
4. one high-confidence confirmation run;
5. optional performance-counter probe only if already available without privilege changes;
6. archive and terminate the instance.

A practical default budget is **no more than about 30 minutes of 64-vCPU instance time** unless the user explicitly authorizes more. Stop early if the scientific question is already answered.

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

The historical PR #8 merge commit is:

```text
b9e61d20d0fe0fa99f302a2ed13aa1215a60c5f3
```

If `main` has advanced, do not reset backwards. Test current `main`, but record the actual tested SHA and compare it with the PR #8 merge commit.

Never modify source merely to make a benchmark green.

---

## 3. qBraid CPU topology preflight

Capture before benchmarking:

```sh
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
rustc --version --verbose
cargo --version
```

Also inspect read-only cgroup/cpuset information when present:

```sh
find /sys/fs/cgroup -maxdepth 2 -type f \( -name 'cpu.max' -o -name 'cpuset.cpus' -o -name 'cpuset.cpus.effective' \) -print -exec cat {} \; 2>/dev/null || true
```

Record:

- CPU model string;
- virtualization/hypervisor if reported;
- sockets, cores, threads and NUMA nodes visible to the guest;
- L1/L2/L3 cache information if exposed;
- OS-visible logical CPU count;
- process CPU affinity/cpuset;
- any CPU quota;
- current system load.

Do not assume `64 vCPU` means 64 physical cores or 32 cores with SMT. Treat vCPU topology as an observed cloud contract only.

Do not change affinity, NUMA policy, power governors or kernel settings.

---

## 4. Correctness gate

Run:

```sh
cargo test --manifest-path cpu-runtime/Cargo.toml --locked --offline
cargo build --manifest-path cpu-runtime/Cargo.toml --release --locked --offline
```

Require the binary:

```text
cpu-runtime/target/release/galaxy-cpu
```

Then:

```sh
cpu-runtime/target/release/galaxy-cpu --help
cpu-runtime/target/release/galaxy-cpu verify --workers 64
```

Preserve the complete verifier output.

Require deterministic u64 boundary evidence and scalar/parallel checksum parity.

The current sampled LUT diagnostic is expected to report:

```text
lut_error_sample_count=8193
lut_sampled_max_abs_q30_error=255
```

The diagnostic is sampled evidence, not a mathematical global bound.

If correctness fails, stop performance interpretation.

---

## 5. Worker x resident scaling matrix

Use the exact positive-u64 logical population:

```text
18446744073709551615
```

Use:

```text
frames  = 8
repeats = 5
seed    = 303
```

Resident populations:

```text
1,048,576
8,388,608
16,777,216
```

Worker counts:

```text
1
2
4
8
16
32
64
```

Run every resident x worker combination sequentially. Never benchmark multiple GALAXY processes concurrently.

Example:

```sh
cpu-runtime/target/release/galaxy-cpu bench \
  --logical 18446744073709551615 \
  --resident 8388608 \
  --frames 8 \
  --workers 64 \
  --repeats 5 \
  --seed 303 \
  --receipt runs/qbraid-cpu-r8388608-w64.json
```

Each receipt path must be unique.

Require:

```text
backend_timing_schedule=interleaved-alternating-v1
```

for every benchmark.

Do not intentionally flush caches, sleep between individual trials to manipulate results, or change CPU affinity between worker counts.

---

## 6. High-confidence large-set confirmation

After the matrix, run one confirmation at the largest resident population:

```text
logical   = 18446744073709551615
resident  = 16777216
frames    = 8
workers   = 64
repeats   = 7
seed      = 303
```

If the runtime reports fewer than 64 effective workers, preserve that fact. Do not fake the worker count.

If 64 workers are slower than 32, that is a valid result.

---

## 7. Optional counter/topology evidence

The local Ryzen result did not contain hardware-counter evidence. If qBraid already exposes `perf` to the unprivileged session, collect a small read-only probe without changing host configuration.

For example, if permitted:

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

Repeat at 64 workers only if the first probe succeeds cleanly.

Treat whole-process counters as supporting context only because the benchmark process contains both Float and BAM-LUT paths plus warm-up work. Do not attribute whole-process cache misses solely to BAM-LUT.

If `perf` is unavailable or permission is denied, record that and continue. Do not use sudo or change paranoid/perf-event settings.

---

## 8. Analysis questions

Answer these from the receipts rather than intuition:

1. Does Float continue scaling from 32 to 64 workers?
2. Does BAM-LUT improve from 32 to 64 workers at 1M, 8M and 16M resident particles?
3. At what worker count does BAM-LUT achieve its best median for each resident population?
4. Does the BAM-LUT plateau begin earlier as the resident set grows?
5. Does the LUT-vs-Float advantage shrink as worker count rises?
6. Does the qBraid 64-vCPU topology reproduce the qualitative local 5950X pattern?
7. Is any observed 32-to-64 regression correlated with NUMA topology, CPU quota, cache information or system contention?

For each resident population calculate from recorded medians:

```text
speedup(N) = one-worker parallel median / N-worker parallel median
efficiency(N) = speedup(N) / N
```

Do this separately for Float and BAM-LUT.

Do not equate 64 vCPUs with 64 physical cores, and do not label a 32-versus-64 result as SMT evidence unless the guest topology independently establishes SMT relationships.

---

## 9. Cross-host comparison with the Ryzen 9 5950X

Compare **scaling shape**, not raw absolute speed, unless the hardware identity and environment justify more.

The strongest local reference points are:

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

- Float clearly retained substantial scaling from 16 to 32 workers locally.
- BAM-LUT gained little or no additional throughput beyond roughly 8-16 workers at the largest local resident sets.
- That shape is consistent with a cache/memory-bandwidth limitation, but the local run did not prove the cause.

The qBraid experiment should test whether a different CPU/cache/NUMA topology shifts that plateau.

---

## 10. Evidence package

Create a unique external report directory, for example:

```text
~/GALAXY-qbraid-cpu-YYYYMMDDTHHMMSSZ-PID/
```

Preserve:

- exact tested Git SHA;
- host/topology/cgroup information;
- Cargo test/build logs;
- verifier output;
- every JSON receipt;
- complete benchmark logs;
- optional perf output;
- machine-readable summary CSV;
- scaling table;
- checksum-consistency table;
- final Markdown report;
- SHA256SUMS.txt.

Create a ZIP and validate it with `unzip -t`.

Do not commit benchmark artifacts to the GALAXY repository.

---

## 11. Claim boundary

Do not claim:

- 64 vCPUs are 64 physical cores;
- 32-versus-64 scaling proves SMT without topology evidence;
- BAM-LUT is proven memory-bandwidth bound without appropriate hardware evidence;
- `u64::MAX` particles were simultaneously allocated;
- this is an N-body simulation;
- the CPU runtime beats the GPU runtime end to end;
- qBraid performance generalizes to all cloud CPUs;
- the sampled LUT diagnostic is a global error proof.

Allowed wording, when supported:

> On this qBraid CPU instance, under this exact GALAXY commit and workload, backend X achieved Y measured scaling from A to B workers.

> The observed BAM-LUT scaling plateau is consistent with cache/memory-bandwidth or topology limits, but the available evidence does not isolate the cause.

---

## Definition of success

A strong result establishes all of the following:

1. exact tested source identity;
2. observed qBraid CPU/vCPU/NUMA/cache topology;
3. native CPU runtime correctness and deterministic checksum parity;
4. successful 1/2/4/8/16/32/64-worker measurements at the selected resident sets;
5. a direct answer to whether 64 workers improve Float and BAM-LUT beyond 32;
6. comparison with the local Ryzen 9 5950X scaling shape without pretending the systems are equivalent;
7. complete, hashed evidence archive.

Be aggressive with the available CPU capacity, conservative with the causal claims, and meticulous with the receipts.
