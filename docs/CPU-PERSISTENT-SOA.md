# CPU persistent worker-pool execution — PE #13

PE #13 targets execution overhead around the already validated worker-local SoA kernel. It does **not** change GALAXY's deterministic addressing, BAM-LUT projection, contribution hash, or wrapping-u64 checksum contract.

## Command boundary

The canonical and PR #12 paths remain available and unchanged:

```sh
galaxy-cpu bench
galaxy-cpu bench-soa
```

PE #13 adds an explicit opt-in persistent path:

```sh
galaxy-cpu verify-soa-pool --workers 32 --tile 1024 --schedule physical-first

galaxy-cpu bench-soa-pool \
  --logical 18446744073709551615 \
  --resident 1048576 \
  --frames 8 \
  --workers 32 \
  --tile 1024 \
  --schedule physical-first \
  --repeats 5 \
  --seed 303 \
  --receipt runs/production-soa/persistent-physical.json
```

To test SMT/logical scheduling explicitly:

```sh
galaxy-cpu bench-soa-pool \
  --resident 1048576 \
  --frames 8 \
  --workers 32 \
  --tile 1024 \
  --schedule logical \
  --repeats 5
```

## Persistent execution shape

The spawned PR #11/#12 path creates worker threads for each complete execution.

PE #13 instead creates the worker set once, allocates one compact SoA tile per worker once, then reuses both across warm-up and measured repetitions:

```text
pool startup
  -> persistent worker threads
  -> worker-local compact SoA tile allocation

repeat N
  -> dispatch one deterministic contiguous range per worker
  -> resident generation into reused tile
  -> BAM-LUT projection
  -> SIMD-friendly contribution batch
  -> worker-local wrapping-u64 sum
  -> collect results
  -> reduce strictly in worker-index order
```

Worker completion order cannot alter the checksum because final reduction is performed in deterministic worker-index order.

## Topology policy

PE #13 deliberately separates execution architecture from PE #14 tuning.

Two explicit policies are available:

- `physical-first` — cap the persistent pool at the detected physical-core count when reliable topology is available.
- `logical` — permit workers up to logical CPU availability, including SMT siblings.

Physical topology detection is best-effort:

- Linux: process CPU allowance from `/proc/self/status` intersected with sysfs `core_id` / `physical_package_id` topology.
- macOS: `sysctl hw.physicalcpu`.
- other platforms: physical count is reported unavailable and `physical-first` falls back to logical availability.

PE #13 does **not** pin threads to individual cores. The topology policy controls worker-count selection only. Affinity or NUMA placement would require separate evidence and should not be implied by this phase.

## Timing boundary

Steady-state pooled timing intentionally excludes pool startup/thread creation. The receipt records `pool_startup_ns` separately.

Measured trials include:

- pool dispatch and result collection;
- deterministic resident generation;
- BAM-LUT projection;
- contribution hashing;
- deterministic worker reduction.

The receipt explicitly records:

```text
worker_threads_persistent_across_trials = true
worker_thread_creation_in_timed_region = false
worker_local_tile_buffers_reused = true
```

This allows direct comparison with PR #12's spawned `bench-soa` path without hiding first-use setup cost.

## Verification boundary

`verify-soa-pool` first executes the existing PR #11/#12 parity suite. It then requires equality among:

1. canonical reference BAM-LUT checksum;
2. spawned worker-local SoA checksum at the selected worker count;
3. persistent-pool checksum on its first dispatch;
4. persistent-pool checksum on a second dispatch.

Any disagreement is a correctness failure.

CI runs this verification across Linux x86-64, Linux ARM64, macOS ARM64, and Windows x86-64.

## Receipt identity

PE #13 receipts use:

```text
schema = galaxy.cpu-runtime-persistent-soa-receipt.v1
runtime = galaxy-cpu
execution_mode = worker-local-soa-persistent
guarded_opt_in = true
canonical_oracle = bench
spawned_soa_fallback = bench-soa
```

Receipts also preserve requested/effective workers, detected topology, scheduling policy, pool startup/spawn/dispatch counts, tile capacity, timing, checksum, and Linux `VmHWM` evidence when available.

## PASS / HOLD boundary

PE #13 is a **PASS** only if:

1. exact checksum parity survives persistent reuse;
2. repeated dispatches do not create additional worker threads;
3. persistent execution is repeatable over useful worker counts;
4. topology metadata is explicit and fail-soft rather than guessed;
5. physical-first and logical policies remain user-visible evidence choices rather than hidden tuning;
6. any measured performance claim remains host/configuration specific.

PE #14, not this phase, decides whether host-aware policy selection should become automatic or promoted toward a default.
