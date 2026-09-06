// SPDX-License-Identifier: MPL-2.0
// Adapted from VORTEX 2.1.0: bounded logical sampling, formatting and settings.
(function (global, factory) {
  const physics = global.UffPhysics || (typeof require === "function" ? require("./uff-physics.js") : null);
  const api = factory(physics);
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  global.GalaxyCore = api;
})(typeof globalThis !== "undefined" ? globalThis : this, function (Physics) {
  "use strict";

  const VERSION = "0.2.0";
  const TAU = Math.PI * 2;
  const STRIDE = 8;
  const MAX_LOGICAL_JS = 2 ** 24;
  const MAX_LOGICAL_RUST = 2 ** 32;
  const MAX_CANVAS_PARTICLES = 1024;
  const MAX_GPU_PARTICLES = 65536;
  const PRESETS = Object.freeze({
    grand: Object.freeze({ arms: 2, pitch: 22, scatter: 0.45, bulge: 0.18, thickness: 0.025, inclination: 38, rotation: -18, shear: 0.35 }),
    pinwheel: Object.freeze({ arms: 4, pitch: 29, scatter: 0.5, bulge: 0.1, thickness: 0.02, inclination: 12, rotation: 0, shear: 0.25 }),
    flocculent: Object.freeze({ arms: 6, pitch: 32, scatter: 1.7, bulge: 0.13, thickness: 0.04, inclination: 32, rotation: 20, shear: 0.6 }),
    edge: Object.freeze({ arms: 2, pitch: 20, scatter: 0.6, bulge: 0.25, thickness: 0.035, inclination: 84, rotation: -12, shear: 0.35 })
  });

  function clamp(value, minimum, maximum) {
    return Math.max(minimum, Math.min(maximum, value));
  }
  function fract(value) { return value - Math.floor(value); }
  function formatCount(value) { return Math.round(Number(value) || 0).toLocaleString("en-US"); }
  function formatPowerOfTwo(value) {
    const exponent = Math.log2(value);
    if (!Number.isInteger(exponent)) return formatCount(value);
    return "2" + String(exponent).replace(/\d/g, digit => "⁰¹²³⁴⁵⁶⁷⁸⁹"[Number(digit)]);
  }
  function bounded(value, fallback, min, max) {
    return typeof value === "number" && Number.isFinite(value) ? clamp(value, min, max) : fallback;
  }
  function defaults() {
    return {
      ...PRESETS.grand, ...Physics.defaults(), logicalCount: MAX_LOGICAL_JS, renderedCount: 16384,
      speed: 0.35, direction: 1, phase: 0, running: true, evolution: "animated",
      zoom: 1, exposure: 1, starSize: 1, palette: "stellar", seed: 303,
      profile: "grand"
    };
  }
  function normalizeSettings(candidate = {}, limits = {}) {
    const input = candidate && typeof candidate === "object" ? candidate : {};
    const settings = defaults();
    Object.assign(settings, Physics.normalize(input));
    const ranges = {
      logicalCount: [256, limits.logical || MAX_LOGICAL_JS],
      renderedCount: [256, limits.rendered || MAX_CANVAS_PARTICLES],
      arms: [1, 8], pitch: [10, 40], scatter: [0.05, 2.5], bulge: [0, 0.5],
      thickness: [0, 0.12], inclination: [0, 90], rotation: [-180, 180],
      speed: [0, 2], phase: [0, 360], shear: [0, 1], zoom: [0.5, 1.8],
      exposure: [0.2, 2.5], starSize: [0.5, 2], seed: [0, 0xffffffff]
    };
    for (const [key, [min, max]] of Object.entries(ranges)) {
      settings[key] = bounded(input[key], clamp(settings[key], min, max), min, max);
    }
    for (const key of ["logicalCount", "renderedCount", "arms", "seed"]) settings[key] = Math.round(settings[key]);
    settings.renderedCount = Math.min(settings.renderedCount, settings.logicalCount);
    settings.direction = input.direction === -1 ? -1 : 1;
    settings.running = typeof input.running === "boolean" ? input.running : settings.running;
    settings.evolution = input.evolution === "phase" ? "phase" : "animated";
    settings.palette = ["stellar", "ember", "silver"].includes(input.palette) ? input.palette : "stellar";
    settings.profile = Object.hasOwn(PRESETS, input.profile) ? input.profile : "custom";
    return settings;
  }

  // VORTEX's equal-interval logical-index mapping, now mapped to seeded stars.
  // Products stay below 2^48, inside JavaScript's exact integer range.
  function logicalIndexForSample(index, count, logicalCount) {
    if (!Number.isInteger(logicalCount) || logicalCount < 1 || logicalCount > MAX_LOGICAL_RUST ||
        !Number.isInteger(count) || count < 1 || count > Math.min(logicalCount, MAX_GPU_PARTICLES) ||
        !Number.isInteger(index) || index < 0 || index >= count) throw new RangeError("Invalid logical sample");
    return Math.floor(index * logicalCount / count);
  }
  function hash32(value) {
    let x = value >>> 0;
    x ^= x >>> 16;
    x = Math.imul(x, 0x7feb352d);
    x ^= x >>> 15;
    x = Math.imul(x, 0x846ca68b);
    return (x ^ (x >>> 16)) >>> 0;
  }
  function sampleValue(logicalId, seed, lane) {
    return (hash32((logicalId ^ seed ^ Math.imul(lane + 1, 0x9e3779b9)) >>> 0) >>> 8) / 16777216;
  }
  function buildSample(logicalCount, count, seed = 303) {
    if (logicalCount > MAX_LOGICAL_JS || count > MAX_CANVAS_PARTICLES) throw new RangeError("JavaScript fallback capacity exceeded");
    logicalIndexForSample(0, count, logicalCount);
    const data = new Float32Array(count * STRIDE);
    for (let index = 0; index < count; index++) {
      const id = logicalIndexForSample(index, count, logicalCount);
      for (let lane = 0; lane < STRIDE; lane++) data[index * STRIDE + lane] = sampleValue(id, seed, lane);
    }
    return data;
  }

  // Authored kinematic model; the GPU uses the same equations in renderer.js.
  // No pairwise forces, accretion, centre crossing, or claim of N-body dynamics.
  function starRadius(u, kind, bulge) {
    return kind < Math.fround(bulge) ? 0.015 + 0.27 * Math.pow(u, 1.8)
      : kind >= 0.97 ? 0.25 + 0.85 * u : 0.06 + 0.92 * Math.pow(u, 1.4);
  }
  function buildOrbitRates(data, settings) {
    const rates = new Float32Array(data.length / STRIDE);
    for (let i = 0; i < rates.length; i++) {
      rates[i] = Physics.angularRate(starRadius(data[i * STRIDE], data[i * STRIDE + 4], settings.bulge), settings);
    }
    return rates;
  }
  function starAt(data, index, settings, phase, out = {}, rates = null) {
    const offset = index * STRIDE;
    const u = data[offset], v = data[offset + 1], w = data[offset + 2];
    const kind = data[offset + 4];
    const bulge = kind < Math.fround(settings.bulge);
    const halo = kind >= 0.97;
    const radius = starRadius(u, kind, settings.bulge);
    let angle = TAU * v;
    if (!bulge && !halo) {
      angle = Math.floor(v * settings.arms) * TAU / settings.arms +
        Math.log(radius / 0.1) / Math.tan(settings.pitch * Math.PI / 180) +
        (w - 0.5) * settings.scatter;
    }
    const omega = rates ? rates[index] : Physics.angularRate(radius, settings);
    // Phase already carries accumulated speed and direction. Editing either
    // changes future motion only, so pausing and reversing cannot jump the field.
    angle += phase * omega;
    const x = radius * Math.cos(angle), y = radius * Math.sin(angle);
    const z = (data[offset + 3] - 0.5) * 2 *
      (bulge ? 0.19 * (1 - radius / 0.3) : halo ? 0.4 : settings.thickness * (0.4 + radius));
    const tilt = settings.inclination * Math.PI / 180;
    const projectedY = y * Math.cos(tilt) + z * Math.sin(tilt);
    const turn = settings.rotation * Math.PI / 180;
    out.x = x * Math.cos(turn) - projectedY * Math.sin(turn);
    out.y = x * Math.sin(turn) + projectedY * Math.cos(turn);
    out.radius = radius;
    out.angle = angle;
    out.bulge = bulge;
    out.halo = halo;
    out.size = 0.7 + 1.4 * Math.pow(data[offset + 5], 6);
    out.light = 0.35 + data[offset + 6] * 0.65;
    out.tint = data[offset + 7];
    return out;
  }
  function advanceClock(clock, dt, settings) {
    if (!Number.isFinite(dt) || dt < 0 || !settings.running || settings.evolution === "phase") return clock;
    return clock + Math.min(dt, 0.05) * settings.speed * settings.direction;
  }
  function effectivePhase(clock, settings) {
    return (settings.evolution === "phase" ? 0 : clock) + settings.phase / 360 * TAU;
  }
  function exportState(settings, clock) {
    return { application: "GALAXY", version: VERSION, physicsSource: Physics.SOURCE_ID, settings: { ...settings }, clock };
  }
  function importState(document, limits) {
    if (!document || document.application !== "GALAXY" || !["0.1.0", VERSION].includes(document.version) ||
        !document.settings || typeof document.settings !== "object" || Array.isArray(document.settings) ||
        !Number.isFinite(document.clock) || Math.abs(document.clock) > 1e7) {
      throw new Error("Choose a GALAXY " + VERSION + " settings file.");
    }
    if (document.version === VERSION && (document.physicsSource !== Physics.SOURCE_ID || !Object.hasOwn(Physics.MODELS, document.settings.dynamics))) {
      throw new Error("This settings file uses different UFF source data or an unknown dynamics model.");
    }
    const candidate = document.version === "0.1.0" ? { ...document.settings, dynamics: "legacy" } : document.settings;
    return { settings: normalizeSettings(candidate, limits), clock: document.clock };
  }
  return Object.freeze({ VERSION, TAU, STRIDE, MAX_LOGICAL_JS, MAX_LOGICAL_RUST,
    MAX_CANVAS_PARTICLES, MAX_GPU_PARTICLES, PRESETS, clamp, fract, formatCount,
    formatPowerOfTwo, defaults, normalizeSettings, logicalIndexForSample, hash32,
    sampleValue, buildSample, starRadius, buildOrbitRates, starAt, advanceClock, effectivePhase, exportState, importState });
});
