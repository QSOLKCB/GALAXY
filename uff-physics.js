// SPDX-License-Identifier: Apache-2.0
// Adapted from QSOLKCB/UFF's models.py, data.py, compact.py and constants.py.
// Copyright 2025-2026 Trent Slade / QSOL-IMC. See data/uff/NOTICE and provenance.json.
(function (global, factory) {
  const data = global.UffDemo || (typeof require === "function" ? require("./data/uff-demo.js") : null);
  const api = factory(data);
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  global.UffPhysics = api;
})(globalThis, function (Data) {
  "use strict";
  if (!Data) throw new Error("The bundled UFF demo data must load first.");
  const KPC_TO_M = 3.085677581491367e19;
  const G = 6.67430e-11 * 1.98847e30 / (KPC_TO_M * 1e6);
  const MYR_TO_S = 31557600 * 1e6;
  const DISK_KPC = 12;
  const MYR_PER_CLOCK = 20;
  const RATE_CONVERSION = MYR_TO_S * 1000 / KPC_TO_M * MYR_PER_CLOCK;
  const MODELS = Object.freeze({ legacy: 0, baryons: 1, nfw: 2, burkert: 3, "mond-rar": 4, "uff-empirical": 5 });
  const LABELS = Object.freeze({ legacy: "Original visual rotation", baryons: "Newtonian baryons", nfw: "NFW halo + baryons", burkert: "Burkert halo + baryons", "mond-rar": "MOND / RAR", "uff-empirical": "UFF empirical v4" });
  const RANGES = Object.freeze({ diskML: [0.05, 1.5], bulgeML: [0.05, 2], blackHoleMillion: [0, 1000],
    uffVInf: [0, 500], uffCore: [0.02, 100], uffBeta: [-1, 1], haloLogMass: [8, 14.5],
    haloConcentration: [1, 40], burkertLogDensity: [4, 11], burkertCore: [0.05, 100], mondA0: [0.03, 6.3] });
  function defaults() {
    return { dynamics: "uff-empirical", diskML: 0.5, bulgeML: 0.7, blackHoleMillion: 0,
      uffVInf: 120, uffCore: 3, uffBeta: 0, haloLogMass: 11.5, haloConcentration: 10,
      burkertLogDensity: 7.5, burkertCore: 5, mondA0: 1.2 };
  }
  function normalize(input = {}) {
    const result = defaults();
    for (const [key, [min, max]] of Object.entries(RANGES)) {
      if (typeof input[key] === "number" && Number.isFinite(input[key])) result[key] = Math.max(min, Math.min(max, input[key]));
    }
    if (Object.hasOwn(MODELS, input.dynamics)) result.dynamics = input.dynamics;
    return result;
  }
  function positiveRadius(radius) {
    if (!Number.isFinite(radius) || radius <= 0) throw new RangeError("Radius must be finite and positive");
  }
  function componentsAt(radius) {
    positiveRadius(radius);
    const rows = Data.rows;
    if (radius <= rows[0][0]) return rows[0].slice(3);
    if (radius >= rows.at(-1)[0]) return rows.at(-1).slice(3);
    let i = 1;
    while (rows[i][0] < radius) i++;
    const a = rows[i - 1], b = rows[i], t = (radius - a[0]) / (b[0] - a[0]);
    return [3, 4, 5].map(column => a[column] + t * (b[column] - a[column]));
  }
  function baryonicV2(gas, disk, bulge, diskML, bulgeML) {
    return Math.max(0, gas * Math.abs(gas) + diskML * disk * disk + bulgeML * bulge * bulge);
  }
  function nfwShape(x) {
    return x < 1e-4 ? 0.5 * x * x - 2 / 3 * x ** 3 + 0.75 * x ** 4 : Math.log1p(x) - x / (1 + x);
  }
  function nfwR200(mass) {
    const rhoCritical = 3 * 0.07 ** 2 / (8 * Math.PI * G);
    return Math.cbrt(3 * mass / (4 * Math.PI * 200 * rhoCritical));
  }
  function haloV2(radius, settings) {
    if (settings.dynamics === "nfw") {
      const mass = 10 ** settings.haloLogMass, c = settings.haloConcentration;
      return G * mass * nfwShape(c * radius / nfwR200(mass)) / nfwShape(c) / radius;
    }
    if (settings.dynamics === "burkert") {
      const x = radius / settings.burkertCore;
      const shape = x < 1e-3 ? 4 / 3 * x ** 3 : Math.log((1 + x) ** 2 * (1 + x * x)) - 2 * Math.atan(x);
      return G * Math.PI * 10 ** settings.burkertLogDensity * settings.burkertCore ** 3 * shape / radius;
    }
    if (settings.dynamics === "uff-empirical") {
      const x = radius / settings.uffCore;
      // Stable continuation of 1-atan(x)/x, avoiding cancellation at small x.
      const base = x < 1e-3 ? x * x * (1 / 3 - x * x / 5 + x ** 4 / 7) : Math.max(0, 1 - Math.atan(x) / x);
      return settings.uffVInf ** 2 * base * Math.exp(2 * settings.uffBeta * x / (1 + x));
    }
    return 0;
  }
  function velocityComponents(radius, settings) {
    const [gas, disk, bulge] = componentsAt(radius);
    const baryons = baryonicV2(gas, disk, bulge, settings.diskML, settings.bulgeML);
    const central = G * settings.blackHoleMillion * 1e6 / radius;
    const extra = haloV2(radius, settings);
    let total = baryons + central + extra;
    if (settings.dynamics === "mond-rar" && total > 0) {
      const y = total * 1e6 / (radius * KPC_TO_M) / (settings.mondA0 * 1e-10);
      total /= -Math.expm1(-Math.sqrt(y));
    }
    return { total: Math.sqrt(total), baryons: Math.sqrt(baryons), central: Math.sqrt(central), extra: Math.sqrt(extra) };
  }
  function velocityKms(radius, settings) { return velocityComponents(radius, settings).total; }
  function angularRate(radius, settings) {
    if (settings.dynamics === "legacy") return 0.32 * ((1 - settings.shear) + settings.shear / Math.sqrt(radius * radius + 0.0144));
    const physicalRadius = radius * DISK_KPC;
    return velocityKms(physicalRadius, settings) / physicalRadius * RATE_CONVERSION;
  }
  function parameters(settings) {
    return [MODELS[settings.dynamics], settings.diskML, settings.bulgeML, settings.blackHoleMillion,
      settings.uffVInf, settings.uffCore, settings.uffBeta, settings.haloLogMass, settings.haloConcentration,
      settings.burkertLogDensity, settings.burkertCore, settings.mondA0];
  }
  return Object.freeze({ SOURCE_ID: Data.sourceId, Data, G, KPC_TO_M, MYR_TO_S, DISK_KPC, MYR_PER_CLOCK,
    RATE_CONVERSION, MODELS, LABELS, RANGES, defaults, normalize, componentsAt, baryonicV2, nfwShape,
    nfwR200, velocityComponents, velocityKms, angularRate, parameters });
});
