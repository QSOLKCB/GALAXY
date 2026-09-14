# qBraid EPYC 7763 evidence — 2026-09-14

This directory preserves the raw compressed evidence bundle from the completed manual qBraid CPU experiment described in `docs/QBRAID-CPU-SCALING.md`.

## Source identity

```text
GALAXY commit: b9e61d20d0fe0fa99f302a2ed13aa1215a60c5f3
```

## Observed host

```text
AMD EPYC 7763 64-Core Processor
96 online logical CPUs
48 exposed cores
2 SMT threads per exposed core
2 NUMA nodes
node 0: CPUs 0-47
node 1: CPUs 48-95
192 MiB aggregate L3 reported by lscpu
377 GiB RAM
Microsoft full virtualization
Linux 6.8.0-1059-azure
```

## Archive

```text
GALAXY-qbraid-EPYC7763-evidence.tar.gz
SHA-256: 5c0474b9537a0ee34493a58c22b368f4846ad12f41633430d4444a678561e87a
```

The archive contains the host topology capture, SMT sibling mapping and the eight retained 8,388,608-resident benchmark receipts:

```text
r8388608-w32.json
r8388608-w48.json
r8388608-w64.json
r8388608-w96.json
r8388608-w48-unpinned-r7.json
r8388608-w48-node0.json
r8388608-w48-node1.json
r8388608-w48-physical-pinned.json
```

## Affinity command provenance

`AFFINITY-MANIFEST.md` binds the three affinity receipts and the repeat-matched unpinned receipt to the exact shell commands and requested CPU masks used by the operator:

```text
r8388608-w48-unpinned-r7.json        -> no taskset mask
r8388608-w48-node0.json              -> taskset -c 0-47
r8388608-w48-node1.json              -> taskset -c 48-95
r8388608-w48-physical-pinned.json    -> taskset -c 0,2,4,...,94
```

The original `galaxy.cpu-runtime-receipt.v1` schema does **not** record the Linux affinity mask. Consequently, the original compressed archive proves the timings, worker counts and deterministic checksums but does not independently prove the exact affinity mask from receipt contents alone. The manifest preserves the operator's retained terminal command provenance retrospectively and states that limitation explicitly.

Future affinity runs must also capture `Cpus_allowed_list` from inside the constrained process before launching `galaxy-cpu`; the reproducible pattern is documented in `AFFINITY-MANIFEST.md` and `docs/QBRAID-CPU-SCALING.md`.

## Result boundary

The unpinned 32/48/64/96 sweep measured its best result at 48 workers for both Float and BAM-LUT. A repeat-matched 48-worker affinity study then found that the runs invoked with node-local masks remained close to the unpinned baseline while the run invoked with a one-SMT-thread-per-exposed-core mask spanning both NUMA nodes was roughly 50% slower for both backends.

The evidence is consistent with a strong topology / memory-locality effect. It does not, without hardware memory-traffic counters, prove a specific first-touch, remote-memory, cache or bandwidth mechanism.

All retained receipts preserve scalar/parallel checksum parity for their backend and workload.
