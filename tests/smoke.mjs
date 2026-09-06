// SPDX-License-Identifier: MPL-2.0
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(import.meta.url);
const Core = require(path.join(root, "galaxy-core.js"));
const read = file => fs.readFileSync(path.join(root, file), "utf8");
const fullLimits = { logical: Core.MAX_LOGICAL_RUST, rendered: Core.MAX_GPU_PARTICLES };
const near = (a, b, epsilon = 1e-10) => assert.ok(Math.abs(a - b) < epsilon, `${a} ≠ ${b}`);

for (const file of ["galaxy-core.js", "renderer.js", "app.js", "wasm/galaxy-wasm.js"]) new vm.Script(read(file), { filename: file });
assert.equal(Core.formatPowerOfTwo(2 ** 24), "2²⁴");
assert.equal(Core.formatPowerOfTwo(2 ** 32), "2³²");
assert.equal(Core.logicalIndexForSample(511, 512, 2 ** 24), 16744448);
assert.equal(Core.logicalIndexForSample(65535, 65536, 2 ** 32), 4294901760);
for (const logical of [256, 65536, 2 ** 24, 2 ** 32]) {
  for (const count of [256, 512, 1024, 999, 65536].filter(n => n <= logical)) {
    const ids = Array.from({ length: count }, (_, i) => Core.logicalIndexForSample(i, count, logical));
    assert.equal(new Set(ids).size, count);
    assert.equal(ids[0], 0);
    assert.ok(ids.at(-1) < logical);
    assert.ok(logical - ids.at(-1) <= Math.ceil(logical / count));
  }
}
for (const [logical, count] of [[Infinity, 512], [NaN, 512], [2 ** 32 + 1, 512], [65536, 0], [1, 512], [2 ** 24, 65537], [123.5, 12]]) {
  assert.throws(() => Core.logicalIndexForSample(0, count, logical), RangeError);
}
assert.throws(() => Core.buildSample(2 ** 25, 512), RangeError);
assert.throws(() => Core.buildSample(2 ** 24, 1025), RangeError);
const data = Core.buildSample(2 ** 24, 1024, 303);
assert.equal(data.byteLength, 32768);
assert.deepEqual(data, Core.buildSample(2 ** 24, 1024, 303));
assert.notDeepEqual(data, Core.buildSample(2 ** 24, 1024, 304));
assert.ok(data.every(n => n >= 0 && n < 1));

// Finite projections, permanent orbital radius, motion and differential shear.
for (const preset of Object.values(Core.PRESETS)) {
  const settings = Core.normalizeSettings({ ...Core.defaults(), ...preset }, fullLimits);
  for (const tilt of [0, 38, 84, 90]) {
    settings.inclination = tilt;
    for (let i = 0; i < 1024; i++) {
      const a = Core.starAt(data, i, settings, 0), b = Core.starAt(data, i, settings, 20);
      for (const n of [a.x, a.y, b.x, b.y, b.radius]) assert.ok(Number.isFinite(n));
      near(a.radius, b.radius); assert.ok(a.radius > 0);
      assert.ok(Math.abs(a.x) < 1.3 && Math.abs(a.y) < 1.3);
      assert.ok(b.angle > a.angle);
    }
  }
}
const rigid = { ...Core.defaults(), shear: 0, inclination: 0, rotation: 0 };
for (let i = 0; i < 1024; i++) {
  const a = Core.starAt(data, i, rigid, 0), b = Core.starAt(data, i, rigid, 3);
  near(b.angle - a.angle, 0.96); near(Math.hypot(a.x, a.y), Math.hypot(b.x, b.y));
}
const differential = { ...rigid, shear: 1 };
const inner = new Float32Array([0, 0.1, 0.5, 0.5, 0.6, 0.5, 0.5, 0.5]);
const outer = new Float32Array([0.99, 0.1, 0.5, 0.5, 0.6, 0.5, 0.5, 0.5]);
const movement = sample => Core.starAt(sample, 0, differential, 1).angle - Core.starAt(sample, 0, differential, 0).angle;
assert.ok(movement(inner) > movement(outer));
let clock = Core.advanceClock(9, 0.02, Core.defaults());
assert.ok(clock > 9);
assert.equal(Core.advanceClock(clock, 10, { ...Core.defaults(), running: false }), clock);
assert.equal(Core.effectivePhase(clock, { ...Core.defaults(), running: false }), clock);
assert.equal(Core.advanceClock(clock, 1, { ...Core.defaults(), evolution: "phase" }), clock);
near(Core.advanceClock(clock, 0.02, { ...Core.defaults(), direction: -1 }), 9);
assert.equal(Core.advanceClock(clock, NaN, Core.defaults()), clock);
assert.equal(Core.effectivePhase(clock, { ...Core.defaults(), evolution: "phase", phase: 180 }), Math.PI);

const max = Core.normalizeSettings({ ...Core.defaults(), logicalCount: 2 ** 32, renderedCount: 65536 }, fullLimits);
const document = Core.exportState(max, -93.25);
assert.deepEqual(Core.importState(JSON.parse(JSON.stringify(document)), fullLimits), { settings: max, clock: -93.25 });
const fallback = Core.importState(document, {}).settings;
assert.equal(fallback.logicalCount, 2 ** 24); assert.equal(fallback.renderedCount, 1024);
for (const input of [null, {}, { ...document, application: "VORTEX" }, { ...document, version: "999" }, { ...document, clock: Infinity }, { ...document, settings: [] }]) {
  assert.throws(() => Core.importState(input, fullLimits));
}
const safe = Core.normalizeSettings({ logicalCount: Infinity, renderedCount: -9, arms: NaN, pitch: "99", bulge: 9, inclination: 999, seed: -20 }, fullLimits);
assert.equal(safe.renderedCount, 256); assert.equal(safe.bulge, 0.5); assert.equal(safe.inclination, 90); assert.equal(safe.seed, 0);

// Execute the checked-in module, including maximum capacity and memory growth.
const context = vm.createContext({});
vm.runInContext(read("wasm/galaxy-wasm.js"), context);
const bytes = Buffer.from(context.GALAXY_WASM_BASE64, "base64");
assert.deepEqual(bytes, fs.readFileSync(path.join(root, "wasm/galaxy_sampler.wasm")), "Standalone Wasm must match the offline browser payload");
const { instance } = await WebAssembly.instantiate(bytes, {});
const wasm = instance.exports;
assert.equal(wasm.abi_version(), 1); assert.equal(wasm.max_rendered(), 65536);
for (const logical of [65536, 2 ** 24, 2 ** 28, 2 ** 32]) {
  for (const count of [256, 512, 1024, 65536]) {
    assert.equal(wasm.generate(logical, count, 303), count);
    const sample = new Float32Array(wasm.memory.buffer, wasm.buffer_ptr(), wasm.buffer_len());
    assert.equal(sample.length, count * 8);
    assert.ok(sample.every(n => n >= 0 && n < 1));
    for (const i of [0, 1, Math.floor(count / 2), count - 1]) {
      const id = Core.logicalIndexForSample(i, count, logical);
      for (let lane = 0; lane < 8; lane++) assert.equal(sample[i * 8 + lane], Core.sampleValue(id, 303, lane));
    }
    if (logical <= 2 ** 24 && count <= 1024) assert.deepEqual(sample, Core.buildSample(logical, count, 303));
  }
}
for (const value of [NaN, Infinity, -1, 0.5, 2 ** 32 + 1]) {
  assert.equal(wasm.generate(value, 512, 303), 0); assert.equal(wasm.buffer_len(), 0);
}
assert.ok(wasm.memory.buffer.byteLength < 16 * 1024 * 1024, "Wasm memory must stay bounded across rebuilds");

// Static/offline entrypoints, unique controls, attribution and no missing assets.
const html = read("index.html");
const ids = [...html.matchAll(/\bid="([^"]+)"/g)].map(match => match[1]);
assert.equal(ids.length, new Set(ids).size);
for (const [, id] of read("app.js").matchAll(/byId\("([^"]+)"\)/g)) assert.ok(ids.includes(id), `Missing control ${id}`);
for (const [, asset] of html.matchAll(/(?:src|href)="([^"#]+)"/g)) {
  if (!asset.startsWith("http")) assert.ok(fs.existsSync(path.join(root, asset)), `Missing asset ${asset}`);
}
assert.match(html, /wasm-unsafe-eval/);
assert.doesNotMatch(html, /<(?:script|link)[^>]+(?:src|href)="https?:/);
assert.match(read("NOTICE.md"), /VORTEX/);
console.log("PASS: logical sampling, orbital invariants, pause/reverse, settings, Rust/Wasm parity, maximum capacity and offline assets.");
