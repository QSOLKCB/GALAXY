# Attribution and licences

GALAXY adapts the VORTEX 2.1.0 browser simulation supplied as
`VORTEX-main(6).zip` from <https://github.com/QSOLKCB/VORTEX>.

The adaptation reuses VORTEX's bounded logical-index sampling, count formatting,
offline instrument layout, Canvas fallback approach, animation control patterns,
and local PNG/WebM/JSON capture. Galaxy orbital equations and the accelerated
renderer replace the original gate–centre–mouth transfer geometry.

The adapted browser sources (`galaxy-core.js`, `app.js`, `renderer.js`,
`index.html`, `style.css`, and the JavaScript smoke/application tests) retain
**Mozilla Public License 2.0** file notices. The full licence is in
[`LICENSES/MPL-2.0.txt`](LICENSES/MPL-2.0.txt). Their corresponding source is
provided directly in this repository and static distribution.

The new Rust sampler and native example, build scripts, generated Wasm module
and browser payload, and benchmark are
**Apache-2.0**, under the repository's existing [`LICENSE`](LICENSE).
The existing repository licence has not been used to relicense the MPL files.

VORTEX's reference photographs, artwork and historical stress-test reports are
not included. Historical VORTEX measurements are not GALAXY benchmarks.

## QSOL UFF physics and demonstration data

`uff-physics.js`, `rust/src/physics.rs`, and the bundled demonstration table
adapt the Apache-2.0 implementation in QSOL UFF v5.3.0, pinned to commit
`596cd732df61587aa1a9801cad1ec13483b1347f`.

Copyright 2025–2026 Trent Slade / QSOL-IMC. The upstream notice is reproduced
verbatim in [`data/uff/NOTICE`](data/uff/NOTICE). The source identities and
hashes are in [`data/uff/provenance.json`](data/uff/provenance.json).

`DEMO_GALAXY.csv` is copied without changes. `data/uff-demo.js` and
`rust/src/uff_data.rs` are generated from it. Rotation-curve plotting,
reference-generation tools and their new physics tests also use Apache-2.0.
Upstream paper citations identify the model literature; those papers are not
redistributed or relicensed here.

## Native GPU runtime

The `runtime/` crate, compute kernels, headless renderer, job examples and local/
SSH runners use Apache-2.0. `runtime/src/kernels.wgsl` and
`runtime/src/reference.rs` adapt the pinned UFF rotation and compact-object
equations above, copyright 2025–2026 Trent Slade / QSOL-IMC. GPU data tables are
generated from the unchanged CSV. Native compact reference fixtures are generated
by executing that UFF source, with its original source identity recorded.

Third-party Rust dependencies retain their own licences and are pinned by
`runtime/Cargo.lock`. The container build copies their distributed licence/notice
files under `/usr/share/doc/galaxy-runtime/dependencies/`, together with this
project's and UFF's notices. The runtime is separate from the MPL browser files.
