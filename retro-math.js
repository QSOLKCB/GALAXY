// SPDX-License-Identifier: Apache-2.0
(function (global, factory) {
  const api = factory();
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  global.GalaxyRetroMath = api;
})(typeof globalThis !== "undefined" ? globalThis : this, function () {
  "use strict";

  const ELITE_GALAXY1 = Object.freeze([0x5a4a, 0x0248, 0xb753]);
  const BAM_QUARTER = 0x40000000;
  const BAM_HALF = 0x80000000;
  const U32_MASK = 0xffffffffn;
  const U16_MASK = 0xffffn;
  const U64_MAX = 0xffffffffffffffffn;
  const CORDIC_K_Q30 = 652032874n;
  const CORDIC_ATAN_BAM = Object.freeze([
    536870912n, 316933406n, 167458907n, 85004756n, 42667331n, 21354465n,
    10679838n, 5340245n, 2670163n, 1335087n, 667544n, 333772n,
    166886n, 83443n, 41722n, 20861n, 10430n, 5215n, 2608n, 1304n,
    652n, 326n, 163n, 81n
  ]);

  function u32(value) { return Number(BigInt(value) & U32_MASK); }
  function rol32(value, shift) {
    const x = BigInt(u32(value)), r = BigInt(shift & 31);
    if (r === 0n) return Number(x);
    return Number(((x << r) | (x >> (32n - r))) & U32_MASK);
  }
  function hash32(value) {
    let x = u32(value);
    x = (x ^ (x >>> 16)) >>> 0;
    x = Math.imul(x, 0x7feb352d) >>> 0;
    x = (x ^ (x >>> 15)) >>> 0;
    x = Math.imul(x, 0x846ca68b) >>> 0;
    return (x ^ (x >>> 16)) >>> 0;
  }
  function eliteTwist(seed) {
    if (!Array.isArray(seed) || seed.length !== 3) throw new TypeError("Elite seed must contain three words");
    const [a, b, c] = seed.map(value => Number(BigInt(value) & U16_MASK));
    return [b, c, (a + b + c) & 0xffff];
  }
  function matrixMultiply(a, b) {
    const out = [[0n,0n,0n],[0n,0n,0n],[0n,0n,0n]];
    for (let i = 0; i < 3; i++) for (let j = 0; j < 3; j++) {
      let sum = 0n;
      for (let k = 0; k < 3; k++) sum += a[i][k] * b[k][j];
      out[i][j] = sum & U16_MASK;
    }
    return out;
  }
  function matrixPower(steps) {
    let n = BigInt(steps);
    if (n < 0n) throw new RangeError("Elite jump must be non-negative");
    let result = [[1n,0n,0n],[0n,1n,0n],[0n,0n,1n]];
    let base = [[0n,1n,0n],[0n,0n,1n],[1n,1n,1n]];
    while (n) {
      if (n & 1n) result = matrixMultiply(result, base);
      base = matrixMultiply(base, base);
      n >>= 1n;
    }
    return result;
  }
  function eliteJump(seed, steps) {
    const words = seed.map(value => BigInt(value) & U16_MASK);
    const matrix = matrixPower(steps);
    return matrix.map(row => Number(row.reduce((sum, value, i) => sum + value * words[i], 0n) & U16_MASK));
  }
  function sectorIndex(sector) {
    let index;
    if (typeof sector === "bigint") {
      index = sector;
    } else if (typeof sector === "number") {
      if (!Number.isSafeInteger(sector)) {
        throw new RangeError("Numeric sector index must be a safe integer; use bigint for full-u64 sectors");
      }
      index = BigInt(sector);
    } else {
      throw new TypeError("Sector index must be a bigint or safe integer number");
    }
    if (index < 0n || index > U64_MAX) throw new RangeError("Sector index must fit u64");
    return index;
  }
  function sectorSeed(globalSeed, sector) {
    const index = sectorIndex(sector);
    const gs = u32(globalSeed);
    const base = [
      ELITE_GALAXY1[0] ^ (gs & 0xffff),
      (ELITE_GALAXY1[1] + (gs >>> 16)) & 0xffff,
      ELITE_GALAXY1[2] ^ (rol32(gs, 7) & 0xffff)
    ];
    return eliteJump(base, index & U32_MASK);
  }
  function sectorSalt(globalSeed, sector) {
    const index = sectorIndex(sector);
    const state = sectorSeed(globalSeed, index);
    const folded = ((((state[0] << 16) | state[1]) >>> 0) ^ rol32(state[2], 11)) >>> 0;
    const lo = Number(index & U32_MASK);
    const hi = Number((index >> 32n) & U32_MASK);
    return hash32(folded ^ hash32(lo ^ 0x9e3779b9) ^ hash32(hi ^ 0x85ebca6b));
  }
  function bamAdd(angle, delta) { return u32(BigInt(u32(angle)) + BigInt(u32(delta))); }
  function bamSub(angle, delta) { return u32(BigInt(u32(angle)) - BigInt(u32(delta))); }
  function bamAdvance(angle, delta, ticks) {
    const n = BigInt(ticks);
    if (n < 0n) throw new RangeError("Tick count must be non-negative");
    return Number((BigInt(u32(angle)) + BigInt(u32(delta)) * n) & U32_MASK);
  }
  function bamRetreat(angle, delta, ticks) {
    const n = BigInt(ticks);
    if (n < 0n) throw new RangeError("Tick count must be non-negative");
    return Number((BigInt(u32(angle)) - BigInt(u32(delta)) * n) & U32_MASK);
  }
  function sinCosQ30(angle) {
    const a = BigInt(u32(angle));
    let z = a < 0x80000000n ? a : a - 0x100000000n;
    let x = CORDIC_K_Q30, y = 0n;
    if (z > 0x40000000n) { z -= 0x80000000n; x = -x; }
    else if (z < -0x40000000n) { z += 0x80000000n; x = -x; }
    for (let i = 0; i < CORDIC_ATAN_BAM.length; i++) {
      const oldX = x;
      if (z >= 0n) {
        x -= y >> BigInt(i);
        y += oldX >> BigInt(i);
        z -= CORDIC_ATAN_BAM[i];
      } else {
        x += y >> BigInt(i);
        y -= oldX >> BigInt(i);
        z += CORDIC_ATAN_BAM[i];
      }
    }
    return { cos: Number(x), sin: Number(y) };
  }
  function projectQ16(radiusQ16, angle) {
    if (!Number.isInteger(radiusQ16) || radiusQ16 < -0x80000000 || radiusQ16 > 0x7fffffff) {
      throw new RangeError("Q16.16 radius must fit signed 32-bit");
    }
    const trig = sinCosQ30(angle), radius = BigInt(radiusQ16);
    return {
      x: Number((radius * BigInt(trig.cos)) >> 30n),
      y: Number((radius * BigInt(trig.sin)) >> 30n)
    };
  }
  return Object.freeze({
    ELITE_GALAXY1, BAM_QUARTER, BAM_HALF, hash32, eliteTwist, eliteJump,
    sectorSeed, sectorSalt, bamAdd, bamSub, bamAdvance, bamRetreat, sinCosQ30, projectQ16
  });
});
