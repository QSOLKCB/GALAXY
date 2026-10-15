// SPDX-License-Identifier: Apache-2.0
// Separate CPU/Number (IEEE-754 binary64) research lane. See docs/MUTUAL-GRAVITY.md.
(function (global) {
  "use strict";
  const DEFAULTS = Object.freeze({ theta: 0.3, softening: 0.03, G: 1, bucket: 4, maxDepth: 40 });

  function finite(x, name) {
    if (!Number.isFinite(x)) throw new RangeError(name + " must be finite (including intermediate arithmetic)");
    return x;
  }
  function optionsFor(options) {
    const o = { ...DEFAULTS, ...options };
    for (const key of Object.keys(DEFAULTS)) finite(o[key], key);
    if (!(o.theta >= 0 && o.theta <= 1)) throw new RangeError("theta must be in [0, 1]");
    if (!(o.G > 0 && o.softening > 0 && o.softening * o.softening > 0)) throw new RangeError("G and softening must be positive; squared softening must not underflow");
    finite(o.softening * o.softening, "squared softening");
    if (!Number.isSafeInteger(o.bucket) || o.bucket < 1) throw new RangeError("bucket must be a positive safe integer");
    if (!Number.isInteger(o.maxDepth) || o.maxDepth < 1 || o.maxDepth > 64) throw new RangeError("maxDepth must be an integer in [1, 64]");
    return o;
  }
  function validateBodies(bodies) {
    if (!Array.isArray(bodies) || bodies.length < 2) throw new RangeError("at least two bodies required");
    for (const b of bodies) {
      if (!b) throw new TypeError("body required");
      finite(b.x, "body.x"); finite(b.y, "body.y");
      if (!(finite(b.mass, "body.mass") > 0)) throw new RangeError("body.mass must be positive");
    }
  }
  function zero() { return new Float64Array(9); }
  function positiveScale(value, name) {
    if (!(value > 0) || !Number.isFinite(value))
      throw new RangeError(name + " is outside the supported f64 range; nondimensionalize the state");
    return value;
  }

  function product(a, b) {
    const value = a * b;
    if (!Number.isFinite(value) || (value === 0 && a !== 0 && b !== 0))
      throw new RangeError("kernel product is outside the supported f64 range; nondimensionalize the state");
    return value;
  }
  function direction(value, radius) {
    const result = value / radius;
    if (result === 0 && value !== 0)
      throw new RangeError("normalized displacement is outside the supported f64 range; nondimensionalize the state");
    return result;
  }

  // Coefficient order: ax, ay, Jxx, Jxy, Jyy, Kxxx, Kxxy, Kxyy, Kyyy.
  // a(u) = a0 + J u + (1/2) K:uu. Both cells share the SAME derivatives.
  function expansion(a, b, G, epsilon) {
    const x = a.comX - b.comX, y = a.comY - b.comY;
    const s2 = finite(x * x + y * y + epsilon * epsilon, "pair distance squared");
    const radius = Math.sqrt(s2), ux = direction(x, radius), uy = direction(y, radius);
    // Normalize displacement before applying inverse powers: forming 1/r^3
    // first can underflow even when the acceleration 1/r^2 is representable.
    const forceScale = positiveScale(G / s2, "force kernel");
    const hessianScale = positiveScale(forceScale / radius, "second derivative");
    const thirdScale = positiveScale(hessianScale / radius, "third derivative");
    // Fail closed on lost directional/moment products as well as radial scales:
    // a later large source mass could otherwise recover an already lost term.
    const ux2 = product(ux, ux), uy2 = product(uy, uy), uxuy = product(ux, uy);
    const hx = product(-forceScale, ux), hy = product(-forceScale, uy);
    const xx = product(hessianScale, 3 * ux2 - 1), xy = product(hessianScale, 3 * uxuy);
    const yy = product(hessianScale, 3 * uy2 - 1);
    const xxx = product(thirdScale, 9 * ux - 15 * product(ux2, ux));
    const xxy = product(thirdScale, 3 * uy - 15 * product(ux2, uy));
    const xyy = product(thirdScale, 3 * ux - 15 * product(ux, uy2));
    const yyy = product(thirdScale, 9 * uy - 15 * product(uy2, uy));
    function coefficients(source, sign) {
      const m = source.mass;
      return new Float64Array([
        product(sign * m, hx + 0.5 * (product(xxx, source.qxx) + 2 * product(xxy, source.qxy) + product(xyy, source.qyy))),
        product(sign * m, hy + 0.5 * (product(xxy, source.qxx) + 2 * product(xyy, source.qxy) + product(yyy, source.qyy))),
        product(m, xx), product(m, xy), product(m, yy),
        product(sign * m, xxx), product(sign * m, xxy), product(sign * m, xyy), product(sign * m, yyy)
      ]);
    }
    return { a: coefficients(b, 1), b: coefficients(a, -1) };
  }

  // Exposed for independent fixed-centre Taylor/finite-difference conformance tests.
  function pairedExpansion(a, b, G = DEFAULTS.G, softening = DEFAULTS.softening) {
    optionsFor({ G, softening });
    for (const cell of [a, b]) {
      for (const key of ["mass", "comX", "comY", "qxx", "qxy", "qyy"]) finite(cell[key], "cell." + key);
      if (!(cell.mass > 0 && cell.qxx >= 0 && cell.qyy >= 0)) throw new RangeError("cell mass/moments invalid");
    }
    const result = expansion(a, b, G, softening);
    for (const c of [result.a, result.b]) for (const x of c) finite(x, "pair coefficient");
    return result;
  }

  function build(bodies, o) {
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    for (const b of bodies) {
      minX = Math.min(minX, b.x); maxX = Math.max(maxX, b.x);
      minY = Math.min(minY, b.y); maxY = Math.max(maxY, b.y);
    }
    const span = finite(Math.max(maxX - minX, maxY - minY), "root span");
    let nodeCount = 0, leafCount = 0, deepest = 0;
    function node(indices, cx, cy, half, depth) {
      nodeCount++; deepest = Math.max(deepest, depth);
      const anchor = bodies[indices[0]];
      let mass = 0, wx = 0, wy = 0;
      for (const i of indices) {
        const b = bodies[i]; mass += b.mass;
        wx += product(b.mass, b.x - anchor.x); wy += product(b.mass, b.y - anchor.y);
      }
      finite(mass, "cell mass");
      const comX = finite(anchor.x + direction(wx, mass), "cell centre x");
      const comY = finite(anchor.y + direction(wy, mass), "cell centre y");
      let qxx = 0, qxy = 0, qyy = 0, radius = 0;
      for (const i of indices) {
        const b = bodies[i], x = b.x - comX, y = b.y - comY;
        const w = positiveScale(b.mass / mass, "normalized cell mass");
        qxx += product(w, product(x, x)); qxy += product(w, product(x, y)); qyy += product(w, product(y, y));
        radius = Math.max(radius, Math.hypot(x, y));
      }
      for (const x of [qxx, qxy, qyy, radius]) finite(x, "cell moment/radius");
      // Enlarge the measured support radius slightly against rounding at the MAC.
      radius *= 1 + 8 * Number.EPSILON;
      const n = { mass, comX, comY, qxx, qxy, qyy, radius, count: indices.length, depth, local: zero(), children: null, indices: null };
      if (indices.length <= o.bucket || depth >= o.maxDepth || half === 0 || radius === 0) {
        n.indices = indices; leafCount++; return n;
      }
      const groups = [[], [], [], []];
      for (const i of indices) groups[(bodies[i].x >= cx ? 1 : 0) | (bodies[i].y >= cy ? 2 : 0)].push(i);
      const h = half / 2;
      // If subdivision is no longer representable, direct evaluation is safe.
      if (cx - h === cx && cx + h === cx && cy - h === cy && cy + h === cy) {
        n.indices = indices; leafCount++; return n;
      }
      n.children = [];
      for (let q = 0; q < 4; q++) if (groups[q].length) {
        n.children.push(node(groups[q], cx + (q & 1 ? h : -h), cy + (q & 2 ? h : -h), h, depth + 1));
      }
      return n;
    }
    const root = node(bodies.map((_, i) => i), minX + (maxX - minX) / 2, minY + (maxY - minY) / 2, span / 2, 0);
    return { root, nodeCount, leafCount, maxDepth: deepest, bodyCount: bodies.length };
  }

  function addShifted(target, p, x, y) {
    target[0] += p[0] + p[2] * x + p[3] * y + 0.5 * (p[5] * x * x + 2 * p[6] * x * y + p[7] * y * y);
    target[1] += p[1] + p[3] * x + p[4] * y + 0.5 * (p[6] * x * x + 2 * p[7] * x * y + p[8] * y * y);
    target[2] += p[2] + p[5] * x + p[6] * y;
    target[3] += p[3] + p[6] * x + p[7] * y;
    target[4] += p[4] + p[7] * x + p[8] * y;
    for (let k = 5; k < 9; k++) target[k] += p[k];
  }

  function accelerations(bodies, options = {}) {
    const o = optionsFor(options); validateBodies(bodies);
    const tree = build(bodies, o);
    const values = bodies.map(() => ({ ax: 0, ay: 0 }));
    const stats = { visited: 0, approximated: 0, direct: 0, propagated: 0 };
    const eps2 = o.softening * o.softening;
    function direct(i, j) {
      const a = bodies[i], b = bodies[j], x = b.x - a.x, y = b.y - a.y;
      const s2 = finite(x * x + y * y + eps2, "direct distance squared");
      const radius = Math.sqrt(s2), scale = positiveScale(o.G / s2, "direct force kernel");
      const fx = product(direction(x, radius), scale), fy = product(direction(y, radius), scale);
      values[i].ax += product(b.mass, fx); values[i].ay += product(b.mass, fy);
      values[j].ax -= product(a.mass, fx); values[j].ay -= product(a.mass, fy);
      stats.direct++;
    }
    function interact(a, b) {
      stats.visited++;
      if (a === b) {
        if (a.indices) {
          for (let i = 0; i < a.indices.length; i++) for (let j = i + 1; j < a.indices.length; j++) direct(a.indices[i], a.indices[j]);
        } else {
          for (let i = 0; i < a.children.length; i++) for (let j = i; j < a.children.length; j++) interact(a.children[i], a.children[j]);
        }
        return;
      }
      if (a.count === 1 && b.count === 1) { direct(a.indices[0], b.indices[0]); return; }
      const distance = Math.hypot(a.comX - b.comX, a.comY - b.comY);
      // Unordered, disjoint node pairs only. Identical criterion in both directions.
      if (o.theta > 0 && a.radius + b.radius < o.theta * distance) {
        const p = expansion(a, b, o.G, o.softening);
        for (let k = 0; k < 9; k++) { a.local[k] += p.a[k]; b.local[k] += p.b[k]; }
        stats.approximated++; return;
      }
      if (a.indices && b.indices) {
        for (const i of a.indices) for (const j of b.indices) direct(i, j);
      } else if (!a.indices && (b.indices || a.radius >= b.radius)) {
        for (const c of a.children) interact(c, b);
      } else {
        for (const c of b.children) interact(a, c);
      }
    }
    interact(tree.root, tree.root);
    function propagate(n) {
      if (n.children) {
        for (const c of n.children) {
          addShifted(c.local, n.local, c.comX - n.comX, c.comY - n.comY);
          stats.propagated++; propagate(c);
        }
      } else {
        for (const i of n.indices) {
          const c = zero(); addShifted(c, n.local, bodies[i].x - n.comX, bodies[i].y - n.comY);
          values[i].ax += c[0]; values[i].ay += c[1];
        }
      }
    }
    propagate(tree.root);
    for (const v of values) { finite(v.ax, "acceleration x"); finite(v.ay, "acceleration y"); }
    return { values, tree, stats };
  }

  function stepLeapfrog(bodies, dt, options = {}) {
    if (!(finite(dt, "dt") > 0)) throw new RangeError("dt must be positive");
    if (!Array.isArray(bodies) || bodies.length < 2) throw new RangeError("at least two bodies required");
    // Resolve caller-controlled option accessors before preflighting destinations.
    const normalizedOptions = optionsFor(options);
    // Snapshot own array elements without invoking accessors or custom iterators.
    const destinations = [];
    for (let i = 0; i < bodies.length; i++) {
      const entry = Object.getOwnPropertyDescriptor(bodies, String(i));
      if (!entry || !("value" in entry)) throw new TypeError("bodies must have own data elements");
      destinations.push(entry.value);
    }
    bodies = destinations;
    // Preflight every destination before any computation or write. Only own,
    // writable data fields are supported; absent velocities require extensibility.
    // Proxy objects are outside this API's ordinary-data-object contract.
    for (const b of bodies) {
      if (!b || typeof b !== "object") throw new TypeError("body must be a data object");
      for (const key of ["x", "y", "vx", "vy"]) {
        const descriptor = Object.getOwnPropertyDescriptor(b, key);
        if (descriptor) {
          if (!("value" in descriptor) || !descriptor.writable)
            throw new TypeError("body." + key + " must be a writable own data property");
        } else if (key === "x" || key === "y" || !Object.isExtensible(b)) {
          throw new TypeError("body requires writable x/y and writable or extensible velocity fields");
        }
      }
      const mass = Object.getOwnPropertyDescriptor(b, "mass");
      if (!mass || !("value" in mass)) throw new TypeError("body.mass must be an own data property");
      finite(Object.getOwnPropertyDescriptor(b, "vx")?.value ?? 0, "body.vx");
      finite(Object.getOwnPropertyDescriptor(b, "vy")?.value ?? 0, "body.vy");
    }
    // Work on a copy so invalid/overflowing steps leave the caller's state intact.
    const next = bodies.map(b => ({ x: b.x, y: b.y,
      vx: Object.getOwnPropertyDescriptor(b, "vx")?.value ?? 0,
      vy: Object.getOwnPropertyDescriptor(b, "vy")?.value ?? 0, mass: b.mass }));
    const first = accelerations(next, normalizedOptions);
    for (let i = 0; i < next.length; i++) {
      const b = next[i], a = first.values[i];
      b.vx = (b.vx ?? 0) + 0.5 * dt * a.ax; b.vy = (b.vy ?? 0) + 0.5 * dt * a.ay;
      b.x += dt * b.vx; b.y += dt * b.vy;
    }
    const second = accelerations(next, normalizedOptions);
    for (let i = 0; i < next.length; i++) {
      const b = next[i], a = second.values[i];
      b.vx = finite(b.vx + 0.5 * dt * a.ax, "updated velocity x");
      b.vy = finite(b.vy + 0.5 * dt * a.ay, "updated velocity y");
    }
    for (let i = 0; i < bodies.length; i++) {
      for (const key of ["x", "y", "vx", "vy"]) {
        // defineProperty avoids inherited setters for previously absent velocities;
        // keep attributes of existing data properties and all unrelated fields.
        const descriptor = Object.getOwnPropertyDescriptor(bodies[i], key);
        Object.defineProperty(bodies[i], key, descriptor
          ? { ...descriptor, value: next[i][key] }
          : { value: next[i][key], writable: true, enumerable: true, configurable: true });
      }
    }
    return second;
  }
  const api = Object.freeze({ DEFAULTS, accelerations, pairedExpansion, stepLeapfrog });
  global.GalaxyMutualGravity = api;
  if (typeof module !== "undefined" && module.exports) module.exports = api;
})(globalThis);
