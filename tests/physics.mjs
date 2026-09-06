// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import crypto from "node:crypto";
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
const Physics = require("../uff-physics.js"), Core = require("../galaxy-core.js");
const read = file => fs.readFileSync(new URL("../" + file, import.meta.url), "utf8");
const expected = JSON.parse(read("tests/uff-reference.json"));
assert.equal(expected.source_commit, Physics.Data.commit);
assert.equal(expected.data_sha256, Physics.Data.sha256);
assert.equal(crypto.createHash("sha256").update(read("data/uff/DEMO_GALAXY.csv")).digest("hex"), Physics.Data.sha256);
const context = vm.createContext({});
vm.runInContext(read("wasm/galaxy-wasm.js"), context);
const { instance } = await WebAssembly.instantiate(Buffer.from(context.GALAXY_WASM_BASE64, "base64"), {});
const wasm = instance.exports;
assert.equal(wasm.abi_version(), 2);
const near = (a, b, tolerance = 2e-8) => assert.ok(Math.abs(a - b) <= tolerance * Math.max(1, Math.abs(b)), `${a} differs from ${b}`);

// Independent predictions come from UFF Python, not this JS/Rust implementation.
for (const entry of expected.cases) {
  for (const [model, values] of Object.entries(entry.predictions)) {
    const settings = { ...Core.defaults(), ...entry.settings, dynamics: model };
    values.forEach((velocity, i) => {
      near(Physics.velocityKms(expected.radii_kpc[i], settings), velocity);
      near(wasm.circular_velocity(expected.radii_kpc[i], ...Physics.parameters(settings)), velocity);
    });
  }
}
assert.equal(Physics.baryonicV2(-10, 20, 10, 0.5, 0.7), 170);
assert.equal(Physics.baryonicV2(-100, 20, 10, 0.5, 0.7), 0);
const base = { ...Core.defaults(), dynamics: "baryons" };
const vbar = Physics.velocityKms(8, base);
near(Physics.velocityKms(8, { ...base, dynamics: "uff-empirical", uffVInf: 0 }), vbar);
assert.ok(Physics.velocityKms(8, { ...base, dynamics: "mond-rar" }) > vbar);
near(Physics.velocityKms(8, { ...base, blackHoleMillion: 100 }) ** 2 - vbar ** 2, Physics.G * 1e8 / 8);
assert.deepEqual(Physics.componentsAt(0.1), [5, 20, 10]);
assert.deepEqual(Physics.componentsAt(30), [15, 95, 5]);
assert.deepEqual(Physics.componentsAt(0.75), [7.5, 30, 11]);

// Physics reaches the full rendered sample, with correct orbital units and no
// stale/detached views after Rust allocates its orbital-rate buffer.
assert.equal(wasm.generate(2 ** 32, 65536, 303), 65536);
for (const dynamics of Object.keys(Physics.MODELS)) {
  const settings = { ...Core.defaults(), dynamics };
  assert.equal(wasm.configure_dynamics(...Physics.parameters(settings), settings.bulge, settings.shear), 65536);
  const sample = new Float32Array(wasm.memory.buffer, wasm.buffer_ptr(), wasm.buffer_len());
  const rates = new Float32Array(wasm.memory.buffer, wasm.orbit_ptr(), wasm.orbit_len());
  const jsRates = Core.buildOrbitRates(sample, settings);
  assert.equal(sample.byteLength + rates.byteLength, 2359296);
  for (let i = 0; i < rates.length; i++) {
    near(rates[i], jsRates[i], 2e-6);
    assert.ok(rates[i] > 0 && Number.isFinite(rates[i]));
  }
  for (const i of [0, 37, 65535]) {
    const start = Core.starAt(sample, i, settings, 0, {}, rates);
    const end = Core.starAt(sample, i, settings, 3, {}, rates);
    near(end.angle - start.angle, 3 * rates[i]);
    near(end.radius, start.radius);
    if (dynamics !== "legacy") near(rates[i], Physics.velocityKms(start.radius * 12, settings) / (start.radius * 12) * Physics.RATE_CONVERSION, 2e-6);
  }
}
const args = Physics.parameters(Core.defaults());
assert.equal(wasm.configure_dynamics(...args, NaN, 0.35), 0);
assert.equal(wasm.orbit_len(), 0);
for (const radius of [0, -1, Infinity, NaN]) {
  assert.throws(() => Physics.velocityKms(radius, base), RangeError);
  assert.ok(Number.isNaN(wasm.circular_velocity(radius, ...args)));
}
assert.equal(wasm.generate(2 ** 24, 512, 303), 512);
assert.equal(wasm.orbit_len(), 0, "Reseeding invalidates old orbital rates");

// Existing v0.1 recipes explicitly retain their original motion law.
const old = { application: "GALAXY", version: "0.1.0", clock: 2.5, settings: { ...Core.defaults(), dynamics: undefined } };
const imported = Core.importState(old);
assert.equal(imported.settings.dynamics, "legacy"); assert.equal(imported.clock, 2.5);
const current = Core.exportState(Core.defaults(), 2.5);
assert.throws(() => Core.importState({ ...current, physicsSource: "wrong data" }));
assert.throws(() => Core.importState({ ...current, settings: { ...current.settings, dynamics: "unknown" } }));
assert.deepEqual(Core.importState(current, { logical: 2 ** 32, rendered: 65536 }).settings, Core.defaults());
console.log("PASS: 195 pinned UFF Python predictions agree with JS and Wasm; five physical laws, orbital units, full-capacity rates, data provenance and v0.1 state migration.");
