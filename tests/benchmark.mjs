// SPDX-License-Identifier: Apache-2.0
// Reproducible buffer benchmark. This does not measure browser/GPU frame rate.
import fs from "node:fs";
import vm from "node:vm";
import { performance } from "node:perf_hooks";
const context = vm.createContext({});
vm.runInContext(fs.readFileSync(new URL("../wasm/galaxy-wasm.js", import.meta.url), "utf8"), context);
const { instance } = await WebAssembly.instantiate(Buffer.from(context.GALAXY_WASM_BASE64, "base64"), {});
const wasm = instance.exports;
const results = [];
for (const logical of [2 ** 24, 2 ** 32]) {
  for (const rendered of [1024, 16384, 65536]) {
    const times = [];
    for (let run = 0; run < 12; run++) {
      const start = performance.now();
      if (wasm.generate(logical, rendered, 303) !== rendered) throw new Error("Generation failed");
      if (run >= 2) times.push(performance.now() - start);
    }
    times.sort((a, b) => a - b);
    results.push({ logical, rendered, particleBytes: wasm.buffer_len() * 4,
      medianGenerationMs: Number(times[Math.floor(times.length / 2)].toFixed(3)),
      wasmMemoryBytes: wasm.memory.buffer.byteLength });
  }
}
console.log(JSON.stringify({ node: process.version, scope: "Wasm sample generation only; no rendering or FPS measurement", results }, null, 2));
