# qBraid EPYC 7763 affinity manifest

This file binds the completed 8,388,608-resident / 48-worker topology-probe receipts to the CPU-affinity commands that were executed manually in the qBraid shell.

## Provenance and limitation

This manifest was written **after** the completed runs from the operator's retained terminal commands and uploaded receipts. The current `galaxy-cpu` receipt schema records `available_parallelism` and `effective_workers`, but it does not record the Linux CPU affinity mask. The original compressed evidence archive therefore cannot, by itself, prove the exact `taskset` mask used for each affinity run.

The mappings below preserve the exact commands/masks used by the operator so the completed experiment remains reproducible and auditable, but they are retrospective command provenance rather than a kernel-captured per-run affinity record.

Future affinity experiments must additionally capture `Cpus_allowed_list` from inside the constrained process before executing `galaxy-cpu`; see `docs/QBRAID-CPU-SCALING.md`.

## Common benchmark configuration

```text
logical_population = 18446744073709551615
resident_particles = 8388608
frames             = 8
requested_workers  = 48
repeats            = 7
seed               = 303
```

The retained receipts preserve identical deterministic backend checksums across these runs:

```text
Float   = adf6d6e30d3ad26d
BAM-LUT = 8d6f07bd77e2fc16
```

## Unpinned confirmation

Receipt:

```text
r8388608-w48-unpinned-r7.json
```

Invocation:

```sh
cpu-runtime/target/release/galaxy-cpu bench \
  --logical 18446744073709551615 \
  --resident 8388608 \
  --frames 8 \
  --workers 48 \
  --repeats 7 \
  --seed 303 \
  --receipt runs/qbraid-cpu/r8388608-w48-unpinned-r7.json
```

No `taskset` affinity mask was applied.

## NUMA node 0 constrained run

Receipt:

```text
r8388608-w48-node0.json
```

Requested CPU mask:

```text
0-47
```

Invocation:

```sh
taskset -c 0-47 \
  cpu-runtime/target/release/galaxy-cpu bench \
  --logical 18446744073709551615 \
  --resident 8388608 \
  --frames 8 \
  --workers 48 \
  --repeats 7 \
  --seed 303 \
  --receipt runs/qbraid-cpu/r8388608-w48-node0.json
```

The guest topology snapshot maps CPUs `0-47` to NUMA node 0 and cores `0-23`, with two SMT threads per exposed core.

## NUMA node 1 constrained run

Receipt:

```text
r8388608-w48-node1.json
```

Requested CPU mask:

```text
48-95
```

Invocation:

```sh
taskset -c 48-95 \
  cpu-runtime/target/release/galaxy-cpu bench \
  --logical 18446744073709551615 \
  --resident 8388608 \
  --frames 8 \
  --workers 48 \
  --repeats 7 \
  --seed 303 \
  --receipt runs/qbraid-cpu/r8388608-w48-node1.json
```

The guest topology snapshot maps CPUs `48-95` to NUMA node 1 and cores `24-47`, with two SMT threads per exposed core.

## One SMT thread per exposed core across both NUMA nodes

Receipt:

```text
r8388608-w48-physical-pinned.json
```

The mask was constructed in the shell as:

```sh
PHYSICAL=$(seq 0 2 94 | paste -sd, -)
```

Expanded requested CPU list:

```text
0,2,4,6,8,10,12,14,16,18,20,22,24,26,28,30,32,34,36,38,40,42,44,46,48,50,52,54,56,58,60,62,64,66,68,70,72,74,76,78,80,82,84,86,88,90,92,94
```

Invocation:

```sh
taskset -c "$PHYSICAL" \
  cpu-runtime/target/release/galaxy-cpu bench \
  --logical 18446744073709551615 \
  --resident 8388608 \
  --frames 8 \
  --workers 48 \
  --repeats 7 \
  --seed 303 \
  --receipt runs/qbraid-cpu/r8388608-w48-physical-pinned.json
```

The guest topology snapshot and `thread_siblings_list` capture show adjacent SMT sibling pairs `0-1, 2-3, ... , 94-95`. Therefore the even-numbered mask selects one logical CPU from each of the 48 exposed cores and spans both reported NUMA nodes.

## Required capture for future affinity runs

Future runs should bind the receipt to an in-process affinity record rather than relying only on shell history. One portable pattern is:

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

Retain the affinity log beside the receipt. The critical evidence is the `Cpus_allowed_list` emitted from inside the `taskset`-constrained shell before `exec` replaces it with `galaxy-cpu`.
