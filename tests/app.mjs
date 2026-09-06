// SPDX-License-Identifier: MPL-2.0
// Application integration without a browser. Real core and Wasm; DOM/render
// adapters model user events and observe counts, uploads, phases and exports.
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { performance } from "node:perf_hooks";
const read = file => fs.readFileSync(new URL("../" + file, import.meta.url), "utf8");

class Element {
  constructor(tag = "div", id = "") {
    this.tagName = tag.toUpperCase(); this.id = id; this.dataset = {}; this.listeners = {};
    this.value = ""; this.textContent = ""; this.options = []; this.disabled = false;
    this.files = []; this.attributes = {}; this.children = [];
  }
  addEventListener(name, handler) { (this.listeners[name] ||= []).push(handler); }
  async emit(name, extras = {}) {
    const event = { target: this, preventDefault() { this.prevented = true; }, ...extras };
    for (const handler of [...(this.listeners[name] || [])]) await handler(event);
    return event;
  }
  setAttribute(key, value) { this.attributes[key] = value; }
  getAttribute(key) { return this.attributes[key]; }
  appendChild(child) { child.parent = this; this.children.push(child); if (child.tagName === "OPTION") this.options.push(child); }
  remove() { if (this.parent) { this.parent.options = this.parent.options.filter(x => x !== this); this.parent.children = this.parent.children.filter(x => x !== this); } }
  getBoundingClientRect() { return { width: 900, height: 700 }; }
  setPointerCapture() {}
  closest() { return ["INPUT", "SELECT", "TEXTAREA", "BUTTON", "A", "SUMMARY"].includes(this.tagName) ? this : null; }
  click() { this.clicked?.(); }
  toBlob(callback) { callback(new Blob(["png"], { type: "image/png" })); }
}

async function boot({ rust = true, gpu = true, brokenWasm = false, reducedMotion = false } = {}) {
  const html = read("index.html"), elements = new Map();
  for (const match of html.matchAll(/<(\w+)\b[^>]*\bid="([^"]+)"[^>]*>/g)) {
    const el = new Element(match[1], match[2]);
    el.value = match[0].match(/\bvalue="([^"]*)"/)?.[1] || "";
    elements.set(el.id, el);
  }
  for (const [, id, content] of html.matchAll(/<select\b[^>]*id="([^"]+)"[^>]*>([\s\S]*?)<\/select>/g)) {
    for (const match of content.matchAll(/<option\s+value="([^"]+)"([^>]*)>([^<]*)<\/option>/g)) {
      const option = new Element("option"); option.value = match[1]; option.textContent = match[3]; option.disabled = match[2].includes("disabled");
      elements.get(id).appendChild(option);
    }
  }
  const document = new Element("document");
  document.hidden = false; document.body = new Element("body");
  document.getElementById = id => elements.get(id) || null;
  document.createElement = tag => new Element(tag);
  document.querySelectorAll = () => [...elements.values()].filter(el => ["INPUT", "BUTTON", "SELECT"].includes(el.tagName));
  const window = new Element("window"), urls = [];
  let scheduled = null, renderer;
  const makeRenderer = (canvas, forceCanvas) => {
    renderer = { gpu: gpu && !forceCanvas, name: gpu && !forceCanvas ? "WebGL" : "Canvas 2D", count: 0, uploads: 0, draws: [],
      setData(data) { this.data = data; this.count = data.length / 8; this.uploads++; },
      setRates(rates) { this.rates = rates; },
      resize() {}, dispose() {},
      draw(settings, phase) { this.draws.push({ settings: { ...settings }, phase }); }
    };
    return { renderer, canvas, fallbackReason: gpu ? "" : "WebGL unavailable" };
  };
  const context = vm.createContext({ document, window, performance, WebAssembly, Blob, atob,
    Uint8Array, Float32Array, console, setTimeout: () => 0, clearInterval() {},
    requestAnimationFrame(fn) { scheduled = fn; return 1; }, devicePixelRatio: 1,
    matchMedia: () => ({ matches: reducedMotion }),
    URL: { createObjectURL(blob) { urls.push(blob); return "blob:test"; }, revokeObjectURL() {} },
    GalaxyRenderer: { createRenderer: makeRenderer },
    GalaxyRotationCurve: class { draw() {} }
  });
  vm.runInContext(read("data/uff-demo.js"), context);
  vm.runInContext(read("uff-physics.js"), context);
  vm.runInContext(read("galaxy-core.js"), context);
  if (rust) vm.runInContext(read("wasm/galaxy-wasm.js"), context);
  if (brokenWasm) context.GALAXY_WASM_BASE64 = "AAAA";
  await vm.runInContext(read("app.js"), context);
  assert.notEqual(elements.get("engineStatus").textContent, "Startup failed", elements.get("status").textContent);
  return {
    el: id => elements.get(id), context, document, urls,
    renderer: () => renderer,
    async change(id, value, event = "change") { const el = elements.get(id); el.value = String(value); await el.emit(event); },
    async click(id) { await elements.get(id).emit("click"); },
    tick(time) { const fn = scheduled; assert.ok(fn); scheduled = null; fn(time); },
    async exported() { await elements.get("exportState").emit("click"); return JSON.parse(await urls.at(-1).text()); }
  };
}

const app = await boot();
assert.equal(app.renderer().count, 16384);
assert.equal(app.el("engineStatus").textContent, "Rust / Wasm + WebGL");
assert.equal(app.el("dynamics").value, "uff-empirical");
assert.equal(app.el("shearControl").hidden, true);
await app.click("pushLimit");
assert.equal(app.renderer().count, 65536);
assert.equal(app.renderer().data.byteLength, 2097152);
assert.equal(app.renderer().rates.byteLength, 262144);
assert.equal(app.el("memoryReadout").textContent, "2.25 MiB");
assert.equal(app.el("logicalCount").value, "4294967296");
const maxExport = await app.exported();
assert.equal(maxExport.settings.logicalCount, 2 ** 32);
assert.equal(maxExport.settings.renderedCount, 65536);
for (const model of ["baryons", "nfw", "burkert", "mond-rar", "uff-empirical", "legacy"]) {
  await app.change("dynamics", model);
  const recipe = await app.exported();
  assert.equal(recipe.settings.dynamics, model);
  assert.equal(recipe.clock, 0);
  assert.equal(app.renderer().rates.length, app.renderer().count);
}
assert.equal(app.el("rotationPanel").hidden, true);
await app.change("dynamics", "uff-empirical");
const previousRates = Array.from(app.renderer().rates);
await app.change("uffVInf", 240, "input");
assert.ok(app.renderer().rates.every((rate, i) => rate > previousRates[i]));
assert.equal(app.el("uffParameters").hidden, false);
assert.equal(app.el("nfwParameters").hidden, true);
await app.change("uffVInf", 120, "input");
await app.change("speed", 1, "input");
app.tick(1000); app.tick(1040);
const beforePause = app.renderer().draws.at(-1).phase;
assert.equal(beforePause, 0.04);
await app.click("playToggle"); app.tick(1080);
assert.equal(app.renderer().draws.at(-1).phase, beforePause, "Pause must preserve the displayed phase");
await app.change("speed", 2, "input"); await app.click("rotateCCW"); app.tick(1120);
assert.equal(app.renderer().draws.at(-1).phase, beforePause, "Speed/direction changes cannot move a paused galaxy");
const uploads = app.renderer().uploads;
await app.click("playToggle"); app.tick(1160); app.tick(1200);
assert.ok(app.renderer().draws.at(-1).phase < beforePause);
assert.equal(app.renderer().uploads, uploads, "Animated frames must not rebuild/upload star buffers");
await app.change("evolution", "phase"); await app.change("phase", 180, "input"); app.tick(1240);
assert.equal(app.renderer().draws.at(-1).phase, Math.PI);
app.tick(1280); assert.equal(app.renderer().draws.at(-1).phase, Math.PI);
assert.equal(app.el("playToggle").disabled, true);
await app.change("profile", "pinwheel"); assert.equal(app.el("arms").value, "4");
await app.change("arms", 7, "input"); assert.equal(app.el("profile").value, "custom");
await app.change("logicalCount", 256); assert.equal(app.renderer().count, 256);
await app.change("renderedCount", 65536); assert.equal(app.renderer().count, 256);
await app.click("snapshot"); assert.equal(app.urls.at(-1).type, "image/png");
await app.click("record"); assert.match(app.el("captureStatus").textContent, /unavailable/);

const saved = { ...maxExport, clock: 12.75, settings: { ...maxExport.settings, running: false } };
app.el("importState").files = [{ size: 2000, text: async () => JSON.stringify(saved) }];
await app.el("importState").emit("change"); app.tick(1320);
assert.equal(app.renderer().draws.at(-1).phase, 12.75);
assert.equal((await app.exported()).settings.logicalCount, 2 ** 32);
const beforeBadImport = await app.exported();
app.el("importState").files = [{ size: 30, text: async () => '{"application":"VORTEX"}' }];
await app.el("importState").emit("change");
assert.match(app.el("captureStatus").textContent, /Could not load/);
assert.deepEqual(await app.exported(), beforeBadImport);

for (const args of [{ rust: false }, { brokenWasm: true }]) {
  const fallback = await boot(args);
  assert.equal(fallback.renderer().count, 1024);
  await fallback.click("pushLimit");
  assert.equal(fallback.el("logicalCount").value, String(2 ** 24));
  assert.equal(fallback.renderer().count, 1024);
  assert.ok(fallback.el("logicalCount").options.find(x => x.value === String(2 ** 32)).disabled);
  fallback.el("importState").files = [{ size: 2000, text: async () => JSON.stringify(saved) }];
  await fallback.el("importState").emit("change");
  assert.equal(fallback.renderer().count, 1024);
  assert.match(fallback.el("captureStatus").textContent, /reduced/);
}
const canvas = await boot({ gpu: false });
await canvas.click("pushLimit");
assert.equal(canvas.el("logicalCount").value, String(2 ** 32));
assert.equal(canvas.renderer().count, 1024);
const reduced = await boot({ reducedMotion: true });
assert.equal(reduced.el("playToggle").textContent, "Play");
const inputEvent = await reduced.document.emit("keydown", { code: "Space", target: reduced.el("seed") });
assert.equal(inputEvent.prevented, undefined); assert.equal(reduced.el("playToggle").textContent, "Play");
const canvasEvent = await reduced.document.emit("keydown", { code: "Space", target: reduced.el("fieldCanvas") });
assert.equal(canvasEvent.prevented, true); assert.equal(reduced.el("playToggle").textContent, "Pause");
await app.el("fieldCanvas").emit("webglcontextlost");
assert.equal(app.renderer().count, 1024);
assert.match(app.el("status").textContent, /context lost/);
console.log("PASS: app startup, real Wasm buffers, max field, pause/reverse, phase slice, controls, capture, imports, reduced motion, missing/corrupt Wasm and context-loss fallback.");
