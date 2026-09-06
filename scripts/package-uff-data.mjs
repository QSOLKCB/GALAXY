// SPDX-License-Identifier: Apache-2.0
// Generate browser/Rust tables from the unmodified UFF demo CSV.
import fs from "node:fs";
import crypto from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const csv = fs.readFileSync(path.join(root, "data/uff/DEMO_GALAXY.csv"));
const provenance = JSON.parse(fs.readFileSync(path.join(root, "data/uff/provenance.json"), "utf8"));
const hash = crypto.createHash("sha256").update(csv).digest("hex");
if (hash !== provenance.files["DEMO_GALAXY.csv"]) throw new Error("UFF demo CSV differs from its recorded source");
const lines = csv.toString("utf8").trim().split(/\r?\n/);
const columns = lines.shift().split(",");
if (columns.slice(0, 6).join(",") !== "R_kpc,V_obs_kms,e_V_kms,V_gas_kms,V_disk_kms,V_bul_kms") throw new Error("Unexpected UFF data columns");
const rows = lines.map(line => line.split(",").slice(0, 6).map(Number));
if (rows.length !== 6 || rows.some((row, i) => row.length !== 6 || row.some(n => !Number.isFinite(n)) || row[0] <= 0 || row[2] <= 0 || (i > 0 && row[0] <= rows[i - 1][0]))) throw new Error("Invalid demo table");
const data = { sourceId: provenance.commit + ":" + hash, commit: provenance.commit, sha256: hash, rows };
fs.writeFileSync(path.join(root, "data/uff-demo.js"),
  "// SPDX-License-Identifier: Apache-2.0\n// Generated from data/uff/DEMO_GALAXY.csv; see data/uff/provenance.json.\n" +
  "(function(g){ const data = " + JSON.stringify(data) + ";\n" +
  "data.rows.forEach(Object.freeze); Object.freeze(data.rows); Object.freeze(data);\n" +
  "if(typeof module !== 'undefined' && module.exports) module.exports=data; g.UffDemo=data; })(globalThis);\n");
fs.writeFileSync(path.join(root, "rust/src/uff_data.rs"),
  "// SPDX-License-Identifier: Apache-2.0\n// Generated from the pinned UFF demo CSV; do not edit.\n" +
  "pub const DEMO: [[f64; 6]; 6] = [\n" + rows.map(row => "    [" + row.map(n => Number.isInteger(n) ? n + ".0" : String(n)).join(", ") + "],").join("\n") + "\n];\n");
console.log("Packaged pinned UFF demo data for JavaScript and Rust.");
