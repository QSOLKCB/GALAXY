// SPDX-License-Identifier: Apache-2.0
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const output = path.join(root, "_site");
fs.rmSync(output, { recursive: true, force: true });
fs.mkdirSync(output, { recursive: true });
for (const file of ["index.html", "style.css", "galaxy-core.js", "uff-physics.js", "rotation-curve.js", "renderer.js", "app.js", "data/uff-demo.js", "data/uff/DEMO_GALAXY.csv", "data/uff/provenance.json", "data/uff/NOTICE", "wasm/galaxy-wasm.js", "wasm/galaxy_sampler.wasm", "LICENSE", "LICENSES/MPL-2.0.txt", "NOTICE.md"]) {
  fs.mkdirSync(path.dirname(path.join(output, file)), { recursive: true });
  fs.copyFileSync(path.join(root, file), path.join(output, file));
}
const html = fs.readFileSync(path.join(output, "index.html"), "utf8");
for (const [, asset] of html.matchAll(/(?:src|href)="([^"#]+)"/g)) {
  if (!asset.startsWith("http") && !fs.existsSync(path.join(output, asset))) throw new Error("Missing distributed asset: " + asset);
}
fs.writeFileSync(path.join(output, ".nojekyll"), "");
console.log("Static GALAXY bundle prepared in _site.");
