// SPDX-License-Identifier: Apache-2.0
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const output = path.join(root, "_site");
fs.rmSync(output, { recursive: true, force: true });
fs.mkdirSync(output, { recursive: true });
for (const file of ["index.html", "style.css", "galaxy-core.js", "renderer.js", "app.js", "wasm/galaxy-wasm.js", "wasm/galaxy_sampler.wasm", "LICENSE", "LICENSES/MPL-2.0.txt", "NOTICE.md"]) {
  fs.mkdirSync(path.dirname(path.join(output, file)), { recursive: true });
  fs.copyFileSync(path.join(root, file), path.join(output, file));
}
fs.writeFileSync(path.join(output, ".nojekyll"), "");
console.log("Static GALAXY bundle prepared in _site.");
