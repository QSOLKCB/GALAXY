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
