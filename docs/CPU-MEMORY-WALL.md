# PE #15 — CPU Memory Wall: Stream → Reduce → Discard

GALAXY v0.5.0 froze the host-aware CPU architecture. The next active phase is deliberately narrower: reduce the amount of hot memory required while preserving the exact BAM-LUT checksum contract.

All previously planned heterogeneous CPU/GPU, NUMA, multi-GPU, output-pipeline, and symmetry work is deferred until this phase reaches a clear PASS/HOLD decision.

## Baseline

The v0.5.0 worker-local SoA tile uses five persistent `u32` fields per tiled particle:

```text
id_lo       4 B
id_hi       4 B
radius_q16  4 B
initial_bam 4 B
delta_bam   4 B
             ----
             20 B
```

Projection and contribution scratch adds:

```text
x_word        4 B
y_word        4 B
contribution  8 B
              ----
              16 B
```

The materialized baseline therefore requires 36 bytes per tiled particle before the LUT. With 32 workers and a 1,024-particle tile this is 1,179,648 bytes of tile capacity. The existing sine+cosine 16K Q2.30 LUT adds 131,072 bytes, for an algorithmic working-set accounting target of 1,310,720 bytes before ordinary process/runtime overhead.

## Experiment A — fused hash/reduce

`memory_wall_probe` implements two exact contribution paths:

```text
materialized:
  x/y arrays -> u64 contribution array -> wrapping-u64 sum

fused:
  x/y arrays -> hash -> wrapping-u64 partial sum
```

The fused path removes the complete `u64` contribution array and reduces scratch from 16 to 8 bytes per tiled particle. Compact particle fields remain unchanged, so total tiled storage falls from 36 to 28 bytes per particle.

The exported symbols `galaxy_memory_hash_batch` and `galaxy_memory_hash_reduce` remain available for disassembly inspection. Fused reduction must produce exactly the same wrapping-u64 result as materialized reduction.

## Experiment B — cache-sized microtiles

The required sweep is:

```text
1024
512
256
128
64
```

For 32 workers, fused tile capacity is predicted from the declared layout as:

| Microtile | Materialized tile bytes | Fused tile bytes |
|---:|---:|---:|
| 1024 | 1,179,648 | 917,504 |
| 512 | 589,824 | 458,752 |
| 256 | 294,912 | 229,376 |
| 128 | 147,456 | 114,688 |
| 64 | 73,728 | 57,344 |

Particles are generated once per microtile, consumed across all requested frames, reduced, then discarded. The experiment does **not** regenerate particle state once per frame.

## Experiment C — exact cosine-only LUT

A naive `sin(angle) = cos(angle - quarter_turn)` substitution is not accepted merely because the mathematical identity is exact: the canonical 24-stage integer CORDIC can contain small direction/rounding asymmetries at table samples.

The implemented candidate therefore stores:

```text
16,384 × i32 cosine samples
16,384 × i16 exact sine corrections
```

for 98,304 bytes. Construction fails closed if any required correction does not fit the declared representation. Every one of the 16,384 reconstructed sine/cosine sample pairs is compared against the canonical CORDIC-derived table before execution.

## Experiment D — exact quarter-wave LUT with correction palette

The quarter-wave candidate stores 4,097 canonical cosine samples, then reconstructs the other quadrants by symmetry. Because integer-CORDIC sample symmetry is not assumed to be bit-exact, a small signed correction palette plus one-byte cosine and sine correction codes is retained for every full-table index.

The representation is accepted only if:

1. the palette fits in 256 entries;
2. every correction fits `i16`;
3. every reconstructed 16K cosine/sine sample is exactly equal to the canonical table;
4. representative non-table BAM angles produce exactly identical integer interpolation output.

The probe records actual LUT storage bytes and correction-palette cardinality rather than claiming a fixed size in advance.

## Verification contract

`memory_wall_probe verify` requires:

- exact compressed-LUT sample parity;
- exact representative interpolation parity;
- fused hash/reduce equality with materialized batch reduction;
- exact streaming-reference checksum parity for both reduction modes;
- exact checksum parity for all three LUT modes;
- exact checksum parity at every required microtile size;
- deterministic worker-index reduction.

Any mismatch is a correctness failure. Memory savings never justify changing the numerical result.

## Benchmark methodology

Run:

```sh
./scripts/bench-cpu-memory-wall.sh
```

The runner builds the probe with `-C target-cpu=native`, verifies the complete parity matrix, then launches every benchmark cell as a separate process so Linux `VmHWM` is not contaminated by a previous matrix candidate.

Default workload:

```text
logical = u64::MAX
resident = 1,048,576
frames = 8
repeats = 5
workers = online logical CPUs (bounded by the probe)
seed = 303
```

The matrix crosses:

```text
reduction: materialized / fused
LUT:       full / cosine-corrected / quarter-corrected
microtile: 1024 / 512 / 256 / 128 / 64
```

Receipts record timing, checksum, isolated-process Linux `VmHWM` where available, exact LUT storage, bytes per tiled particle, aggregate worker microtile capacity, and algorithmic working-set bytes.

## PASS / HOLD boundary

PASS requires all exactness gates plus a material reduction in declared algorithmic working set. Promotion into `galaxy-cpu` requires the memory winner to avoid a material wall-time regression on representative hardware and to retain useful native code generation.

A memory win with a substantial runtime loss is HOLD. A fast path with checksum drift is FAIL. No result is generalized beyond measured hosts/configurations.

The canonical v0.5.0 paths remain untouched during this experiment.
