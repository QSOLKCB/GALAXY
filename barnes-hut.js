// SPDX-License-Identifier: Apache-2.0
(function (global) {
  "use strict";

  const DEFAULTS = Object.freeze({ theta: 0.5, softening: 0.03, G: 1, bucket: 4, maxDepth: 40 });
  const TAU = Math.PI * 2;

  function finite(value, name) {
    if (!Number.isFinite(value)) throw new Error(name + " must be finite");
    return value;
  }

  function validateBodies(bodies) {
    if (!Array.isArray(bodies) || bodies.length < 2) throw new Error("Barnes-Hut requires at least two bodies");
    for (let i = 0; i < bodies.length; i++) {
      const b = bodies[i];
      finite(b.x, "body.x"); finite(b.y, "body.y");
      finite(b.vx ?? 0, "body.vx"); finite(b.vy ?? 0, "body.vy");
      if (!Number.isFinite(b.mass) || b.mass <= 0) throw new Error("body.mass must be positive and finite");
    }
  }

  function rootBounds(bodies) {
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    for (const b of bodies) {
      minX = Math.min(minX, b.x); minY = Math.min(minY, b.y);
      maxX = Math.max(maxX, b.x); maxY = Math.max(maxY, b.y);
    }
    const span = Math.max(maxX - minX, maxY - minY, 1e-12);
    const half = span * 0.500000000001 + 1e-12;
    return { cx: (minX + maxX) * 0.5, cy: (minY + maxY) * 0.5, half };
  }

  function quadrant(body, cx, cy) {
    return (body.x >= cx ? 1 : 0) | (body.y >= cy ? 2 : 0);
  }

  function childBounds(cx, cy, half, q) {
    const h = half * 0.5;
    return {
      cx: cx + (q & 1 ? h : -h),
      cy: cy + (q & 2 ? h : -h),
      half: h
    };
  }

  function aggregate(bodies, indices) {
    let mass = 0, wx = 0, wy = 0;
    for (const index of indices) {
      const b = bodies[index];
      mass += b.mass; wx += b.mass * b.x; wy += b.mass * b.y;
    }
    return { mass, comX: wx / mass, comY: wy / mass };
  }

  function buildTree(bodies, options = {}) {
    validateBodies(bodies);
    const bucket = Math.max(1, Math.trunc(options.bucket ?? DEFAULTS.bucket));
    const maxDepth = Math.max(1, Math.trunc(options.maxDepth ?? DEFAULTS.maxDepth));
    const bounds = rootBounds(bodies);
    let nodeCount = 0, leafCount = 0, deepest = 0;

    function build(indices, cx, cy, half, depth) {
      nodeCount++; deepest = Math.max(deepest, depth);
      const a = aggregate(bodies, indices);
      const node = { cx, cy, half, mass: a.mass, comX: a.comX, comY: a.comY, count: indices.length, depth, children: null, indices: null };
      if (indices.length <= bucket || depth >= maxDepth || half <= Number.EPSILON * 32) {
        node.indices = indices;
        leafCount++;
        return node;
      }
      const groups = [[], [], [], []];
      for (const index of indices) groups[quadrant(bodies[index], cx, cy)].push(index);
      const children = [null, null, null, null];
      for (let q = 0; q < 4; q++) {
        if (!groups[q].length) continue;
        const b = childBounds(cx, cy, half, q);
        children[q] = build(groups[q], b.cx, b.cy, b.half, depth + 1);
      }
      node.children = children;
      return node;
    }

    const root = build(Array.from({ length: bodies.length }, (_, i) => i), bounds.cx, bounds.cy, bounds.half, 0);
    return { root, nodeCount, leafCount, maxDepth: deepest, bodyCount: bodies.length, bucket };
  }

  function contains(node, body) {
    return body.x >= node.cx - node.half && body.x <= node.cx + node.half &&
      body.y >= node.cy - node.half && body.y <= node.cy + node.half;
  }

  function addPointMass(acc, target, x, y, mass, G, eps2) {
    const dx = x - target.x, dy = y - target.y;
    const r2 = dx * dx + dy * dy + eps2;
    const invR = 1 / Math.sqrt(r2);
    const scale = G * mass * invR * invR * invR;
    acc.ax += dx * scale; acc.ay += dy * scale;
  }

  function accelerationFor(index, bodies, tree, options = {}, stats = null) {
    const theta = Number(options.theta ?? DEFAULTS.theta);
    const G = Number(options.G ?? DEFAULTS.G);
    const softening = Number(options.softening ?? DEFAULTS.softening);
    if (!(theta >= 0) || !Number.isFinite(theta)) throw new Error("theta must be finite and >= 0");
    if (!(G > 0) || !Number.isFinite(G)) throw new Error("G must be positive and finite");
    if (!(softening >= 0) || !Number.isFinite(softening)) throw new Error("softening must be finite and >= 0");
    const eps2 = softening * softening;
    const target = bodies[index];
    const acc = { ax: 0, ay: 0 };
    const local = stats || { visited: 0, approximated: 0, direct: 0 };

    function walk(node) {
      local.visited++;
      if (node.indices) {
        for (const otherIndex of node.indices) {
          if (otherIndex === index) continue;
          const other = bodies[otherIndex];
          addPointMass(acc, target, other.x, other.y, other.mass, G, eps2);
          local.direct++;
        }
        return;
      }
      const dx = node.comX - target.x, dy = node.comY - target.y;
      const d = Math.hypot(dx, dy);
      const width = node.half * 2;
      if (!contains(node, target) && d > 0 && width / d < theta) {
        addPointMass(acc, target, node.comX, node.comY, node.mass, G, eps2);
        local.approximated++;
        return;
      }
      for (const child of node.children) if (child) walk(child);
    }
    walk(tree.root);
    return acc;
  }

  function accelerations(bodies, options = {}) {
    const tree = buildTree(bodies, options);
    const values = new Array(bodies.length);
    const stats = { visited: 0, approximated: 0, direct: 0 };
    for (let i = 0; i < bodies.length; i++) values[i] = accelerationFor(i, bodies, tree, options, stats);
    return { values, tree, stats };
  }

  function directAccelerationFor(index, bodies, options = {}) {
    validateBodies(bodies);
    const G = Number(options.G ?? DEFAULTS.G);
    const softening = Number(options.softening ?? DEFAULTS.softening);
    const eps2 = softening * softening;
    const target = bodies[index];
    const acc = { ax: 0, ay: 0 };
    for (let j = 0; j < bodies.length; j++) {
      if (j === index) continue;
      const other = bodies[j];
      addPointMass(acc, target, other.x, other.y, other.mass, G, eps2);
    }
    return acc;
  }

  function directAccelerations(bodies, options = {}) {
    validateBodies(bodies);
    const G = Number(options.G ?? DEFAULTS.G);
    const softening = Number(options.softening ?? DEFAULTS.softening);
    const eps2 = softening * softening;
    const values = Array.from({ length: bodies.length }, () => ({ ax: 0, ay: 0 }));
    for (let i = 0; i < bodies.length; i++) {
      for (let j = i + 1; j < bodies.length; j++) {
        const a = bodies[i], b = bodies[j];
        const dx = b.x - a.x, dy = b.y - a.y;
        const r2 = dx * dx + dy * dy + eps2;
        const invR = 1 / Math.sqrt(r2);
        const common = G * invR * invR * invR;
        values[i].ax += dx * common * b.mass;
        values[i].ay += dy * common * b.mass;
        values[j].ax -= dx * common * a.mass;
        values[j].ay -= dy * common * a.mass;
      }
    }
    return values;
  }

  function errorAgainstDirect(bodies, options = {}) {
    const approx = accelerations(bodies, options);
    const exact = directAccelerations(bodies, options);
    let sumSq = 0, max = 0;
    for (let i = 0; i < bodies.length; i++) {
      const dx = approx.values[i].ax - exact[i].ax;
      const dy = approx.values[i].ay - exact[i].ay;
      const denom = Math.max(Math.hypot(exact[i].ax, exact[i].ay), 1e-30);
      const rel = Math.hypot(dx, dy) / denom;
      sumSq += rel * rel; max = Math.max(max, rel);
    }
    return { rmsRelative: Math.sqrt(sumSq / bodies.length), maxRelative: max, ...approx };
  }

  function stepLeapfrog(bodies, dt, options = {}) {
    finite(dt, "dt");
    if (!(dt > 0)) throw new Error("dt must be > 0");
    const first = accelerations(bodies, options);
    for (let i = 0; i < bodies.length; i++) {
      const b = bodies[i], a = first.values[i];
      b.vx = (b.vx ?? 0) + 0.5 * dt * a.ax;
      b.vy = (b.vy ?? 0) + 0.5 * dt * a.ay;
      b.x += dt * b.vx; b.y += dt * b.vy;
    }
    const second = accelerations(bodies, options);
    for (let i = 0; i < bodies.length; i++) {
      const b = bodies[i], a = second.values[i];
      b.vx += 0.5 * dt * a.ax; b.vy += 0.5 * dt * a.ay;
    }
    return second;
  }

  function rng(seed) {
    let state = (BigInt.asUintN(64, BigInt(seed)) || 1n);
    return () => {
      state ^= state << 13n; state ^= state >> 7n; state ^= state << 17n;
      state = BigInt.asUintN(64, state);
      return Number(state >> 11n) / 9007199254740992;
    };
  }

  function makeDisc(count, seed = 303, options = {}) {
    count = Math.trunc(count);
    if (count < 2 || count > 65536) throw new Error("disc count must be in [2, 65536]");
    const random = rng(seed);
    const radius = options.radius ?? 1;
    const totalMass = options.totalMass ?? 1;
    const spin = options.spin ?? 0.72;
    const cx = options.cx ?? 0, cy = options.cy ?? 0;
    const cvx = options.vx ?? 0, cvy = options.vy ?? 0;
    const direction = options.direction ?? 1;
    const bodies = [];
    for (let i = 0; i < count; i++) {
      const r = radius * Math.sqrt(-Math.log(Math.max(1e-12, 1 - random()))) / 2.2;
      const a = TAU * random();
      const x = cx + r * Math.cos(a), y = cy + r * Math.sin(a);
      const speed = spin * Math.sqrt(Math.max(r, 0.02)) / Math.sqrt(1 + r * r);
      bodies.push({ x, y, vx: cvx - direction * speed * Math.sin(a), vy: cvy + direction * speed * Math.cos(a), mass: totalMass / count });
    }
    return bodies;
  }

  function makeCollision(count = 1536, seed = 303) {
    const left = Math.floor(count / 2), right = count - left;
    return makeDisc(left, seed, { radius: 0.62, totalMass: 0.5, cx: -0.75, cy: -0.15, vx: 0.18, vy: 0.08, direction: 1, spin: 0.62 })
      .concat(makeDisc(right, seed + 1, { radius: 0.55, totalMass: 0.5, cx: 0.75, cy: 0.15, vx: -0.18, vy: -0.08, direction: -1, spin: 0.62 }));
  }

  function flattenTree(tree, maxDepth = Infinity) {
    const out = [];
    (function walk(node) {
      if (node.depth <= maxDepth) out.push({ cx: node.cx, cy: node.cy, half: node.half, depth: node.depth, count: node.count, mass: node.mass });
      if (node.children) for (const child of node.children) if (child) walk(child);
    })(tree.root);
    return out;
  }

  global.GalaxyBarnesHut = Object.freeze({
    DEFAULTS, buildTree, accelerationFor, accelerations, directAccelerationFor, directAccelerations,
    errorAgainstDirect, stepLeapfrog, makeDisc, makeCollision, flattenTree
  });
})(globalThis);
