// SPDX-License-Identifier: Apache-2.0
// Reproducible sample/rate-buffer benchmark; excludes browser/GPU frame rate.
import fs from "node:fs";
import vm from "node:vm";
import { performance } from "node:perf_hooks";
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
const Physics = require("../uff-physics.js"), Core = require("../galaxy-core.js");
const context = vm.createContext({});
vm.runInContext(fs.readFileSync(new URL("../wasm/galaxy-wasm.js", import.meta.url), "utf8"), context);
const { instance } = await WebAssembly.instantiate(Buffer.from(context.GALAXY_WASM_BASE64, "base64"), {});
const wasm = instance.exports;
const results = [];
for (const logical of [2 ** 24, 2 ** 32]) {
  for (const rendered of [1024, 16384, 65536]) {
    const times = [], physicsTimes = [];
    for (let run = 0; run < 12; run++) {
      const start = performance.now();
      if (wasm.generate(logical, rendered, 303) !== rendered) throw new Error("Generation failed");
      if (run >= 2) times.push(performance.now() - start);
      const settings = Core.defaults(), physicsStart = performance.now();
      if (wasm.configure_dynamics(...Physics.parameters(settings), settings.bulge, settings.shear) !== rendered) throw new Error("Orbital configuration failed");
      if (run >= 2) physicsTimes.push(performance.now() - physicsStart);
    }
    times.sort((a, b) => a - b);
    physicsTimes.sort((a, b) => a - b);
    results.push({ logical, rendered, sampleBytes: wasm.buffer_len() * 4, orbitBytes: wasm.orbit_len() * 4,
      medianGenerationMs: Number(times[Math.floor(times.length / 2)].toFixed(3)),
      medianUffConfigurationMs: Number(physicsTimes[Math.floor(physicsTimes.length / 2)].toFixed(3)),
      wasmMemoryBytes: wasm.memory.buffer.byteLength });
  }
}
console.log(JSON.stringify({ node: process.version, scope: "Wasm sample generation and default UFF orbital-rate configuration; no rendering or FPS measurement", results }, null, 2));
