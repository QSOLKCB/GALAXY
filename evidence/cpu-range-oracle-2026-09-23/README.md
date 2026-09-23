# Live GALAXY CPU range parity, 2026-09-23

`frozen-oracle.json` is the exact JSON payload from the
`galaxy-cpu-frozen-range-oracle` artifact produced by the Linux x86-64 job in
[Native CPU runtime run 35906565752](https://github.com/QSOLKCB/GALAXY/actions/runs/35906565752).

- Source head: `dac77234f37ec38a976d5d358d2a2d4299d9ce0f`
- Artifact ID: `10770784268`
- Artifact ZIP SHA-256 (GitHub digest): `c13d2f09f00a4196ea6bbe0c19d3cda90f9db8f1f6d23ab03139bdf11a35e21d`
- Extracted JSON SHA-256: `ec01961206d006ff37724b330001ef1f19f273f1853eea6077610b92838bb464`
- Geometry: logical `u64::MAX`, 8,388,608 resident samples, 8 frames, seed 303
- Ranges: `[0,4194304)` and `[4194304,8388608)`
- Full and reduced BAM-LUT checksum: `8d6f07bd77e2fc16`, equal to the archived v0.4.0 CPU oracle

The workflow executes GALAXY's versioned CPU range command for the full
geometry and both halves, validates exact addition and the frozen checksum,
then uploads the generated JSON. This is CPU execution evidence. It makes no
claim about CUDA, external binary provenance in MESH, or GPU/heterogeneous
baseline parity.
