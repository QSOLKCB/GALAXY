# GALAXY Roadmap — Crush the Memory Wall Without Weakening Correctness

GALAXY is now far enough along that the main optimization problem is no longer simply “make the loop faster.” The next phases target **data movement, resident state, topology, and heterogeneous execution** while keeping the existing deterministic reference/evidence boundaries intact.

The working rule for this roadmap is:

> **Do not store what can be regenerated exactly. Do not materialize what can be reduced exactly. Do not transfer what can be summarized exactly.**

A faster implementation does not get to redefine correctness.

---

## 1. Frozen lessons from the completed CPU work

The CPU sequence established the current optimization ladder:

```text
PR #10  SIMD/autovectorization feasibility
PR #11  worker-local bounded SoA execution
PR #12  guarded production integration
PR #13  persistent workers + topology-aware scheduling
PR #14  calibrated host-aware promotion with fail-closed parity
```

The important architectural lessons are broader than any one benchmark number:

- deterministic logical IDs allow work to be regenerated from indices instead of stored globally;
- worker-local SoA tiles turn memory from `O(resident)` into bounded working sets;
- exact wrapping-`u64` reductions permit deterministic partitioning and reassociation;
- thread count is a measured policy, not a hardware-model lookup;
- topology can matter more than the nominal core count;
- calibration must include costs that scale with the full requested execution shape, including persistent-pool startup and allocation;
- optimized paths remain subordinate to independent reference/oracle paths.

### qBraid topology warning

The EPYC 7763 experiment is the permanent warning against cargo-cult affinity rules:

- 48 unpinned workers were the best measured point for both Float and BAM-LUT in the recorded 8M-resident sweep;
- 48 logical CPUs constrained to one NUMA node remained near that baseline;
- one logical CPU per exposed physical core spread across both NUMA nodes was roughly 50% slower for both backends;
- deterministic checksums were unchanged.

The result is consistent with NUMA locality / first-touch / remote-memory effects, but the exact mechanism was not established by hardware counters. Future topology policy must therefore be measured rather than inferred.

---

# PE #15 — Heterogeneous Resident Execution

## Goal

Run **CPU and GPU simultaneously on disjoint logical shards** of one GALAXY workload.

Do not use the GPU as fake slow system RAM. Instead, use each memory domain as local working memory for the processor attached to it:

```text
                    deterministic logical domain
                              │
                 ┌────────────┴────────────┐
                 │                         │
          CPU-owned shards          GPU-owned shards
                 │                         │
       CPU RAM / SoA tiles        VRAM resident tiles
                 │                         │
      persistent CPU workers       GPU compute kernels
                 │                         │
        compact reduction           compact reduction
                 └────────────┬────────────┘
                              │
                     deterministic merge
                              │
                          final receipt
```

### Required experiment

Compare the same workload under:

```text
CPU only
GPU only
CPU + GPU concurrently
```

Then sweep static CPU/GPU fractions before introducing an automatic policy.

### Rate-proportional split

If measured steady-state service rates are:

```text
r_cpu
r_gpu
```

use the first-order split:

```text
cpu_fraction = r_cpu / (r_cpu + r_gpu)
gpu_fraction = r_gpu / (r_cpu + r_gpu)
```

This is a scheduling heuristic, not a theorem. The measured full execution remains the authority.

### Deterministic ownership

Every logical range has exactly one owner during an execution. Avoid shared mutable particle state and CPU/GPU page ping-pong.

Prefer:

```text
range A -> CPU
range B -> GPU
```

over CPU and GPU repeatedly touching the same allocation.

### Exact BAM merge

For integer/BAM contribution paths, preserve the existing wrapping-`u64` reduction contract:

```text
C_total = C_cpu + C_gpu mod 2^64
```

This permits independent CPU/GPU completion order while retaining the same final reduction value.

Float paths must retain their existing floating-point claim boundary; do not manufacture bitwise CPU/GPU equivalence where the arithmetic does not support it.

### PE #15 PASS boundary

- CPU-only, GPU-only, and CPU+GPU process exactly the declared logical domain;
- no overlap or gaps in shard ownership;
- exact BAM checksum parity with the canonical CPU oracle;
- bounded-error Float/GPU comparison only where already justified;
- CPU and GPU execution actually overlaps in wall time;
- combined execution beats the faster device alone by a useful measured margin before promotion;
- all split decisions and rates are recorded in the receipt;
- no universal split ratio is claimed.

---

# PE #16 — GPU Stream → Reduce → Discard

## Donor pattern

GLUBALL CUDA Runtime V2/V3/V3.1 and NEXUS VE-24 demonstrate the useful execution pattern:

```text
evaluate transient state
        ↓
fold into compact receivers
        ↓
discard transient state
```

The useful engineering idea is imported, not the donor project's geometry or ontology.

### Target GALAXY evidence mode

GALAXY should have a GPU execution mode that does **not** allocate or read back a complete particle output field.

Instead, the GPU should maintain a compact evidence record such as:

```text
processed_count
checksum / receipt words
nonfinite_count
max_radius_or_error
optional min/max bounds
optional integer histograms
optional fixed-size moments
```

The exact record must be derived from GALAXY's existing correctness/evidence contract rather than invented merely because a field is convenient.

### Materialized mode remains separate

Keep two explicit modes:

```text
materialize  -> rendering, inspection, CSV/snapshot export
reduce       -> benchmark, verification, receipt, scaling, tuning
```

Do not cripple rendering to optimize evidence mode, and do not force evidence mode to pay the rendering readback cost.

### Hierarchical reduction

Use a hierarchy rather than one global update per particle:

```text
thread-local
   ↓
subgroup/warp/workgroup reduction
   ↓
block/workgroup compact summary
   ↓
second-stage reduction or bounded atomics
   ↓
small final evidence record
```

Candidate receiver operations should be associative integer operations wherever possible:

- wrapping addition;
- XOR;
- integer min/max;
- integer counts;
- exact histogram accumulation.

### Two reduction topologies

Measure both where supported:

```text
atomic final receiver
two-stage compact summaries
```

Never assume the theoretically cleaner reduction topology is faster on every GPU.

### Readback target

The performance target is **constant-size or small bounded readback independent of logical population**.

A million-particle evidence run should not imply a million-particle host transfer.

### PE #16 PASS boundary

- no full-output allocation in `reduce` mode;
- compact readback size explicitly reported;
- full materialized path remains available for correctness spot checks and presentation;
- compact evidence agrees with the declared reference/evidence contract;
- reduction topology is recorded;
- peak VRAM and host readback bytes are included in receipts.

---

# PE #17 — Procedural State Elimination

GALAXY particles are unusually friendly to a **compute-instead-of-store** strategy because identity and initial state are deterministically addressable.

## 17A — Do not store logical IDs

If:

```text
id = logical_id(index, resident, logical)
```

is a pure deterministic function, avoid caching `id` in every resident particle when the index is already available.

Measure the ALU cost against bytes saved; do not assume recomputation is free.

## 17B — Backend-specific resident representations

Do not keep Float and BAM fields simultaneously when an execution uses only one backend.

Candidate compact forms:

```text
BAM resident:
  radius_q16
  initial_bam
  delta_bam

Float resident:
  radius / initial phase / phase increment required by Float only
```

Keep the reference representation if needed for oracle compatibility, but optimized paths should not carry dead backend fields through the cache hierarchy.

## 17C — Generate particle fields inside the tile

Prefer:

```text
logical index
   ↓
procedural field generation
   ↓
small worker/GPU tile
   ↓
consume all needed frames
   ↓
clear/reuse tile
```

rather than building a whole-resident particle array.

## 17D — Exact BAM phase recurrence

Candidate optimization:

```text
angle_0 = initial_bam
angle_{n+1} = angle_n + delta_bam mod 2^32
```

This is exactly equivalent to repeated `initial + delta * frame mod 2^32` for the integer BAM domain, but it may exchange integer multiply work for a dependency chain. Benchmark it; do not assume it wins.

Do **not** transfer this recurrence claim to Float without a separate numerical contract because floating recurrence changes rounding history.

---

# PE #18 — NUMA-Local Persistent Pools

## Goal

Turn the qBraid NUMA observation into a controlled runtime experiment without hardcoding “physical cores good” or “node-local always good.”

### Candidate architecture

```text
NUMA node 0 -> local worker pool -> local tiles -> local reduction
NUMA node 1 -> local worker pool -> local tiles -> local reduction
                                   ↓
                         deterministic host merge
```

### First-touch discipline

Where the OS/runtime allows it safely, allocate/touch worker-local buffers from the worker that will own them after its scheduling/affinity policy is established.

Record:

- requested affinity;
- kernel-visible allowed CPU list;
- NUMA node mapping;
- worker-to-node policy;
- allocation/touch policy;
- whether explicit memory binding is actually in use.

### Never silently pin

The canonical/default scheduler must not be replaced by explicit affinity unless the target host demonstrates a repeatable benefit.

The EPYC evidence already shows that a seemingly sensible cross-node physical-core policy can be much worse than leaving the scheduler alone.

### Huge pages

Transparent or explicit huge pages are a **candidate experiment only** for large persistent buffers. Measure TLB effects and setup cost. Never require privileged kernel changes just to make GALAXY fast.

---

# PE #19 — Multi-GPU Compact Sharding

GLUBALL's accepted 1/2/4/8-GPU evidence provides a useful donor pattern: contiguous quotient/remainder device ranges, explicit per-device identity/provenance, and a deterministic host aggregate.

GALAXY candidate:

```text
logical domain
   ↓
contiguous deterministic GPU shards
   ↓
per-GPU resident execution
   ↓
per-GPU compact evidence
   ↓
host deterministic merge
```

Requirements:

- no point may be owned by two GPUs;
- coverage must be complete;
- heterogeneous GPUs must not be treated as bit-identical Float engines without evidence;
- per-device service rates may inform workload size only after measured calibration;
- multi-GPU timing remains a hardware observation, not a physics claim.

---

# PE #20 — Precompute Only What Is Sublinear

A useful rule from GLUBALL V3/V3.1 is that pure repeated subcomputations can be precomputed when the cache is much smaller than the complete output.

Allowed candidate shapes:

```text
O(U)
O(V)
O(workers)
O(blocks)
O(1)
```

Danger sign:

```text
O(total particle × frame output)
```

The goal is to accelerate evaluation, not to benchmark replay of cached answers.

Every precomputation requires:

1. a pure key;
2. deterministic construction;
3. an explicit memory cost;
4. equivalence against the uncached path;
5. setup-cost accounting and an observed break-even point.

---

# PE #21 — Trigonometric Memory Hacks

These are candidates for the BAM path and must be benchmarked against the existing LUT implementation.

## 21A — Quarter-wave LUT

BAM32 exposes quadrant symmetry. A quarter-wave Q2.30 table can reduce the current sine+cosine LUT footprint substantially, with quadrant/sign reconstruction in the hot path.

Candidate target from the prior memory analysis:

```text
4096 × i32 = 16 KiB base quarter table
```

This is attractive for cache residency but adds dispatch/sign logic.

## 21B — Store cosine only

Derive sine by a quarter-turn phase offset:

```text
sin(angle) = cos(angle - quarter_turn)
```

This approximately halves trigonometric table storage while preserving the same table representation. Benchmark lookup pressure versus extra indexing.

## 21C — Inline integer CORDIC

The existing integer CORDIC can remove the heap LUT entirely.

This trades memory traffic for arithmetic. It may win on memory-starved hardware and lose badly on throughput-oriented CPUs. Treat it as an auto-tunable backend candidate, not a default.

---

# PE #22 — Output Without Particle Materialization

The current large-u64 GPU path already bounds resident tiles, but presentation snapshots still gather sampled particles into host structures and retain a frame/sample cache before final output.

Future output paths should separate:

```text
scientific/evidence reduction
sampled CSV export
image accumulation
interactive rendering
```

### Direct image accumulation

Candidate GPU path:

```text
particle position
   ↓
project to pixel/bin
   ↓
GPU histogram / luminance accumulation
   ↓
read back image-sized buffer
```

This avoids returning particles merely so the CPU can immediately bin them into pixels.

The image is presentation output, not canonical geometry evidence.

### Streaming CSV

For sampled snapshots, write records incrementally in canonical sample order instead of caching all frames when ordering can be preserved with bounded staging.

---

# PE #23 — Double-Buffered Host/Device Pipeline

For workloads that genuinely require host/device transfer, overlap transfer and compute:

```text
GPU computes tile N
CPU consumes/reduces tile N-1
next tile transfer prepared concurrently
```

Candidate ingredients:

- two bounded device buffers;
- two bounded staging buffers;
- asynchronous copy/compute queues or CUDA streams;
- events/fences for ownership transfer;
- no allocation in the steady-state loop.

The benefit is workload- and bus-dependent. Report PCIe/fabric bytes and overlap efficiency.

### Unified memory warning

Do not use migration-based unified memory as the default CPU+GPU architecture on discrete GPUs. CPU/GPU page ping-pong can turn the memory wall into a page-fault wall.

UMA/integrated-GPU systems may justify a separate zero-copy/shared-memory experiment because their physical memory topology is different.

---

# PE #24 — Symmetry / Orbit Compression, Proof Before Compression

This is the dangerous fun one.

A symmetry may reduce computation only if GALAXY can prove or exhaustively validate all of:

1. an exact domain transform;
2. closure of the transformed domain;
3. an involution or otherwise explicit orbit structure;
4. an exact reconstruction rule for the required output/evidence;
5. preserved canonical checksum/evidence.

**Same symmetry orbit is not object equality.**

No sample may be skipped merely because a picture or equation “looks symmetric.”

Potential targets include exact angle symmetries and integer BAM periodicity, not speculative physical symmetry claims.

---

# PE #25 — Memory-Budgeted Auto Selection

Extend host-auto selection so performance is not the only objective.

Every candidate should report an execution envelope:

```text
steady_state_ns
startup_ns
host_peak_rss_bytes
device_resident_bytes
host_to_device_bytes
device_to_host_bytes
worker_local_capacity_bytes
```

Possible policy modes:

```text
fastest
memory-cap <= X
balanced
no-GPU-readback
```

Promotion remains fail-closed on correctness.

A fast candidate that violates an explicit memory budget is not eligible.

---

# Unholy Math / Systems Hacks Worth Testing

These are **candidate experiments**, not promises.

## A. Exploit the reduction algebra

Wrapping addition modulo `2^64` and XOR form associative reduction structures. This permits:

- arbitrary tree reduction;
- CPU/GPU split execution;
- per-NUMA partials;
- per-GPU partials;
- completion-order independence;
- hierarchical compact evidence.

Use the algebra deliberately instead of serializing work for aesthetic reasons.

## B. Recompute versus load: use a roofline-style decision

For every stored field ask:

```text
bytes avoided / extra integer-or-float operations
```

If memory bandwidth is the bottleneck and a field is cheap/pure to regenerate, recomputation may be faster **and** smaller.

Measure this per architecture; do not assume arithmetic is universally cheaper than memory.

## C. Counter-based procedural randomness only

Keep random-looking particle parameters as pure functions of:

```text
(logical_id, seed, lane)
```

Avoid mutable PRNG state, per-worker RNG buffers, locks, and skip-ahead bookkeeping.

## D. Reduce at the producer

If the consumer only needs:

```text
sum / xor / min / max / count / histogram
```

do that reduction where the data is produced.

Moving raw values to another memory domain just to reduce them is usually paying twice.

## E. Adaptive chunk size from cache/memory evidence

Tile size should remain a measured finite search variable. Useful candidate inputs include:

- worker count;
- physical/SMT policy;
- NUMA node;
- observed cache sizes when reliable;
- per-tile byte footprint;
- GPU storage-buffer limits;
- measured service rate.

Do not encode CPU product-name folklore.

## F. Sparse evidence sampling

When complete materialization is not part of the correctness contract, keep exact deterministic samples using logical-index mapping rather than retaining the whole field.

Sampling never substitutes for a checksum/oracle requirement that is supposed to cover the complete domain.

## G. Fixed-size moments/histograms

For diagnostic distributions, online integer/fixed accumulators can replace giant retained arrays when the requested statistic permits it.

Be careful with floating online moments: reassociation changes floating results. Use an explicit numerical contract or fixed/integer accumulator when exactness is required.

## H. Bit packing

Any packed field must have a proven range. Candidates include small enums, bounded flags, or lane/state metadata. Do not pack physical quantities merely to save bytes unless quantization is explicitly noncanonical.

## I. Memory-map archival output, not hot state

Large output that truly must exist can be streamed to files or memory-mapped archival storage instead of held in heap RAM. This is an I/O strategy, not a hot-loop optimization.

## J. Make allocation itself observable

Receipts for performance-sensitive paths should increasingly expose:

```text
requested capacity
actual bounded capacity
bytes per worker/device
startup allocation time
peak RSS / VRAM when observable
readback bytes
```

The PR #14 full-pool-startup correction is the model: if a cost scales with the full execution shape, do not estimate it from a smaller calibration object and pretend the score is complete.

---

# Permanent boundaries

## Canonical versus accelerated

- canonical/reference paths remain available;
- accelerated CPU/GPU paths do not define their own correctness;
- Float GPU execution does not become bitwise canonical merely because it is fast;
- integer/BAM exactness is promoted only where exact parity is actually demonstrated.

## Presentation versus evidence

Rendering, screenshots, galaxy images, visual modes, and sampled exports are presentation/data products. They must not silently become scientific proof surfaces.

## Donor provenance

Useful engineering donor patterns may be reimplemented from QSOL projects such as GLUBALL and NEXUS, but GALAXY must keep its own contracts and tests. Import the optimization pattern, not the donor project's ontology.

## No universal benchmark claims

A measured winner is tied to:

- source identity;
- compiler/toolchain;
- workload;
- host topology;
- memory policy;
- accelerator/driver where applicable.

Re-measure before transferring a tuning constant to another host.

---

# Suggested next order

```text
PE #15  CPU + GPU heterogeneous resident execution
PE #16  GPU compact evidence: stream -> reduce -> discard
PE #17  procedural state elimination / backend-specific state
PE #18  NUMA-local persistent pools and first-touch experiments
PE #19  multi-GPU compact sharding
PE #20  bounded pure precomputation + break-even accounting
PE #21  quarter-wave / cosine-only / CORDIC BAM experiments
PE #22  direct GPU image/evidence output without particle materialization
PE #23  double-buffered transfer/compute overlap
PE #24  proof-gated symmetry/orbit compression
PE #25  memory-budget-aware automatic promotion
```

The sequence is deliberately evidence-first. The goal is not to build the most complicated runtime possible. The goal is to make **logical population size increasingly unrelated to physical memory consumption** while keeping every optimization auditable and reversible.
