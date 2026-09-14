// SPDX-License-Identifier: Apache-2.0
"use strict";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const __dirname = path.dirname(fileURLToPath(import.meta.url));
const Retro = require("../retro-math.js");

const lines = fs.readFileSync(path.join(__dirname, "retro-vectors.txt"), "utf8")
  .split(/\r?\n/).map(line => line.trim()).filter(line => line && !line.startsWith("#"));

function words(text) { return text.split(",").map(value => parseInt(value, 16)); }
function hex32(value) { return (value >>> 0).toString(16).padStart(8, "0"); }
function hex16s(values) { return values.map(value => value.toString(16).padStart(4, "0")).join(","); }

for (const line of lines) {
  const fields = line.split("|");
  switch (fields[0]) {
    case "elite_jump": {
      const actual = Retro.eliteJump(words(fields[1]), BigInt(fields[2]));
      assert.equal(hex16s(actual), fields[3], line);
      break;
    }
    case "sector": {
      const seed = parseInt(fields[1], 16), sector = BigInt(fields[2]);
      assert.equal(hex16s(Retro.sectorSeed(seed, sector)), fields[3], line);
      assert.equal(hex32(Retro.sectorSalt(seed, sector)), fields[4], line);
      break;
    }
    case "cordic": {
      const actual = Retro.sinCosQ30(parseInt(fields[1], 16));
      assert.equal(actual.cos, Number(fields[2]), line);
      assert.equal(actual.sin, Number(fields[3]), line);
      break;
    }
    case "project": {
      const actual = Retro.projectQ16(Number(fields[1]), parseInt(fields[2], 16));
      assert.equal(actual.x, Number(fields[3]), line);
      assert.equal(actual.y, Number(fields[4]), line);
      break;
    }
    case "roundtrip": {
      const start = parseInt(fields[1], 16), delta = parseInt(fields[2], 16), ticks = BigInt(fields[3]);
      const advanced = Retro.bamAdvance(start, delta, ticks);
      assert.equal(hex32(advanced), fields[4], line);
      assert.equal(hex32(Retro.bamRetreat(advanced, delta, ticks)), fields[5], line);
      break;
    }
    default: throw new Error(`Unknown retro vector kind: ${fields[0]}`);
  }
}

assert.deepEqual(Retro.eliteTwist(Retro.ELITE_GALAXY1), [0x0248, 0xb753, 0x13e5]);
assert.notEqual(Retro.sectorSalt(303, 0n), Retro.sectorSalt(303, 1n << 32n));
console.log(`retro-math: ${lines.length} golden vectors passed`);
