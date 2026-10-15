// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import { BH, MUTUAL, ROOT, OPTIONS, BUDGET, clone, errorMetrics, forceDiagnostics,
  fixture, trajectory, stateDifference } from '../scripts/lib/mutual-gravity-evidence.mjs';

const summary = { cases: [], pairedPotential: null, trajectory: null };
function near(actual, expected, relative = 2e-12, absolute = 2e-13, label = '') {
  assert.ok(Math.abs(actual - expected) <= absolute + relative * Math.abs(expected),
    `${label}: ${actual} != ${expected}`);
}
function finite(values) {
  assert.ok(values.every(a => Number.isFinite(a.ax) && Number.isFinite(a.ay)));
}
function checkState(name, bodies, { accepted = false, ...options } = {}) {
  const o = { ...OPTIONS, ...options }, saved = JSON.stringify(bodies);
  const exact = BH.directAccelerations(bodies, o), zero = MUTUAL.accelerations(bodies, { ...o, theta: 0 });
  assert.equal(zero.stats.approximated, 0);
  assert.equal(zero.stats.direct, bodies.length * (bodies.length - 1) / 2, `${name}: unordered pair coverage`);
  for (let i = 0; i < bodies.length; i++) {
    near(zero.values[i].ax, exact[i].ax, 3e-12, 2e-11, `${name} direct x ${i}`);
    near(zero.values[i].ay, exact[i].ay, 3e-12, 2e-11, `${name} direct y ${i}`);
  }
  const approximate = MUTUAL.accelerations(bodies, o), error = errorMetrics(approximate.values, exact);
  const forces = forceDiagnostics(bodies, approximate.values);
  finite(approximate.values);
  assert.ok(error.rmsRelative < BUDGET.rmsRelative, `${name} RMS ${error.rmsRelative}`);
  assert.ok(error.maxRelative < BUDGET.maxRelative, `${name} max ${error.maxRelative}`);
  assert.ok(forces.normalizedNetForce < BUDGET.normalizedNetForce,
    `${name} normalized net force ${forces.normalizedNetForce}`);
  if (accepted) assert.ok(approximate.stats.approximated > 0, `${name}: no accepted cell pairs`);
  assert.ok(approximate.stats.direct > 0, `${name}: no direct near pairs`);
  assert.ok(approximate.tree.maxDepth <= o.maxDepth);
  assert.equal(JSON.stringify(bodies), saved, `${name}: force solve mutated inputs`);
  const again = MUTUAL.accelerations(bodies, o);
  assert.notEqual(again.tree, approximate.tree);
  assert.notEqual(again.tree.root, approximate.tree.root);
  assert.deepEqual(again.values, approximate.values, `${name}: non-deterministic same-state solve`);
  summary.cases.push({ name, n: bodies.length, error, forces, stats: approximate.stats });
  return approximate;
}

const browserContext = vm.createContext({});
vm.runInContext(fs.readFileSync(new URL('mutual-gravity.js', ROOT), 'utf8'), browserContext);
assert.ok(Object.isFrozen(MUTUAL));
assert.equal(browserContext.GalaxyMutualGravity.DEFAULTS.theta, OPTIONS.theta);
assert.deepEqual(JSON.parse(JSON.stringify(browserContext.GalaxyMutualGravity.accelerations(fixture('disc', 24, 17)).values)),
  MUTUAL.accelerations(fixture('disc', 24, 17)).values);

for (const n of [128, 512]) for (const seed of [17, 303, 991]) for (const preset of ['disc', 'collision'])
  checkState(`${preset}-${n}-${seed}`, fixture(preset, n, seed), { accepted: true });
for (const preset of ['disc', 'collision']) checkState(`${preset}-2048-303`, fixture(preset, 2048, 303), { accepted: true });

const unequal = fixture('collision', 160, 109);
for (let i = 0; i < unequal.length; i++) unequal[i].mass *= 0.15 + (i % 11) ** 2;
const mass = unequal.reduce((a, b) => a + b.mass, 0);
for (const b of unequal) b.mass /= mass;
checkState('unequal-masses', unequal, { accepted: true });
const clustered = unequal.map((b, i) => ({ ...b, x: (i % 2 ? 2 : -2) + b.x * .08,
  y: (i % 3 ? .15 : -.15) + b.y * .05 }));
checkState('clustered-unequal', clustered, { accepted: true });
const coincident = Array.from({ length: 19 }, (_, i) => ({ x: 1.25, y: -2.5, mass: (i + 1) / 190 }));
const collisionAtOnePoint = checkState('all-coincident', coincident, { bucket: 1, maxDepth: 12 });
assert.ok(collisionAtOnePoint.values.every(a => a.ax === 0 && a.ay === 0));
const duplicateGroups = clustered.map((b, i) => i < 8 ? ({ ...b, x: 0, y: 0 }) : b);
checkState('partial-coincidence', duplicateGroups, { accepted: true, bucket: 1, maxDepth: 12 });
checkState('finite-depth-direct', unequal, { bucket: 1, maxDepth: 1 });
checkState('large-bucket-direct', unequal, { bucket: 1024 });
checkState('nondefault-kernel', unequal, { G: 1.7, softening: .21, accepted: true });
checkState('maximum-depth-option', unequal, { maxDepth: 64, bucket: 1, accepted: true });
const permissive = MUTUAL.accelerations(unequal, { ...OPTIONS, theta: 1 });
finite(permissive.values);
assert.ok(permissive.stats.approximated > 0);
assert.ok(forceDiagnostics(unequal, permissive.values).normalizedNetForce < BUDGET.normalizedNetForce);
const scaledG = MUTUAL.accelerations(unequal, { ...OPTIONS, G: 2 });
const unitG = MUTUAL.accelerations(unequal, OPTIONS);
for (let i = 0; i < unequal.length; i++) {
  near(scaledG.values[i].ax, 2 * unitG.values[i].ax);
  near(scaledG.values[i].ay, 2 * unitG.values[i].ay);
}

const reference = checkState('translation-reference', unequal, { accepted: true });
for (const shift of [[8, -13], [1e4, -2e4]]) {
  const translated = unequal.map(b => ({ ...b, x: b.x + shift[0], y: b.y + shift[1] }));
  const r = checkState(`translation-${shift.join(',')}`, translated, { accepted: true });
  const delta = errorMetrics(r.values, reference.values);
  assert.ok(delta.globalRelativeL2 < 2e-10, `translation covariance ${delta.globalRelativeL2}`);
}
for (const angle of [Math.PI / 2, .431]) {
  const c = Math.cos(angle), s = Math.sin(angle);
  const rotated = unequal.map(b => ({ ...b, x: c * b.x - s * b.y, y: s * b.x + c * b.y }));
  checkState(`rotation-${angle}`, rotated, { accepted: true });
  // Axis-aligned tree partitions can change under rotation. Exact theta-zero
  // force must transform covariantly; the approximate solver is accuracy-gated.
  const original = MUTUAL.accelerations(unequal, { ...OPTIONS, theta: 0 }).values;
  const r = MUTUAL.accelerations(rotated, { ...OPTIONS, theta: 0 }).values;
  for (let i = 0; i < r.length; i++) {
    near(r[i].ax, c * original[i].ax - s * original[i].ay);
    near(r[i].ay, s * original[i].ax + c * original[i].ay);
  }
}

// Independent scalar Taylor polynomial of 1 / sqrt(|R + delta|^2 + eps^2).
// This is deliberately not built from the solver's acceleration derivatives.
function scalarCubic(rx, ry, dx, dy, eps) {
  const u = rx * rx + ry * ry + eps * eps;
  const z1 = 2 * (rx * dx + ry * dy) / u, z2 = (dx * dx + dy * dy) / u;
  return (1 - .5 * z1 - .5 * z2 + .375 * z1 * z1 + .75 * z1 * z2 - .3125 * z1 ** 3) / Math.sqrt(u);
}
function cell(points) {
  const mass = points.reduce((s, p) => s + p.mass, 0);
  const comX = points.reduce((s, p) => s + p.mass * p.x, 0) / mass;
  const comY = points.reduce((s, p) => s + p.mass * p.y, 0) / mass;
  let qxx = 0, qxy = 0, qyy = 0;
  for (const p of points) {
    const x = p.x - comX, y = p.y - comY;
    qxx += p.mass * x * x / mass; qxy += p.mass * x * y / mass; qyy += p.mass * y * y / mass;
  }
  return { mass, comX, comY, qxx, qxy, qyy };
}
function evaluate(c, x, y) {
  return { ax: c[0] + c[2] * x + c[3] * y + .5 * (c[5] * x * x + 2 * c[6] * x * y + c[7] * y * y),
    ay: c[1] + c[3] * x + c[4] * y + .5 * (c[6] * x * x + 2 * c[7] * x * y + c[8] * y * y) };
}
const left = [{ x: -1.2, y: -.2, mass: .7 }, { x: -.8, y: .1, mass: 1.3 }, { x: -1.04, y: .15, mass: .4 }];
const right = [{ x: 1.5, y: .3, mass: 1.8 }, { x: 1.7, y: .04, mass: .3 }, { x: 1.3, y: .01, mass: 1.1 }];
const ca = cell(left), cb = cell(right), pairG = 1.7, pairEps = .21;
const pair = MUTUAL.pairedExpansion(ca, cb, pairG, pairEps);
const swapped = MUTUAL.pairedExpansion(cb, ca, pairG, pairEps);
for (let k = 0; k < 9; k++) { near(pair.a[k], swapped.b[k]); near(pair.b[k], swapped.a[k]); }
function potential() {
  let energy = 0;
  for (const a of left) for (const b of right) {
    const dx = (b.x - cb.comX) - (a.x - ca.comX), dy = (b.y - cb.comY) - (a.y - ca.comY);
    energy -= pairG * a.mass * b.mass * scalarCubic(cb.comX - ca.comX, cb.comY - ca.comY, dx, dy, pairEps);
  }
  return energy;
}
let maxFiniteDifferenceError = 0;
const pairValues = [];
for (const [points, c, coeff] of [[left, ca, pair.a], [right, cb, pair.b]]) for (const p of points) {
  const predicted = evaluate(coeff, p.x - c.comX, p.y - c.comY); pairValues.push(predicted);
  for (const [coordinate, acceleration] of [['x', 'ax'], ['y', 'ay']]) {
    const saved = p[coordinate], h = 2e-5;
    p[coordinate] = saved + h; const plus = potential();
    p[coordinate] = saved - h; const minus = potential(); p[coordinate] = saved;
    const numeric = -(plus - minus) / (2 * h * p.mass);
    maxFiniteDifferenceError = Math.max(maxFiniteDifferenceError, Math.abs(predicted[acceleration] - numeric));
    near(predicted[acceleration], numeric, 3e-8, 3e-9, 'paired potential finite difference');
  }
}
const pairForces = forceDiagnostics(left.concat(right), pairValues);
assert.ok(pairForces.normalizedNetForce < BUDGET.normalizedNetForce);
summary.pairedPotential = { maxFiniteDifferenceError, ...pairForces };

const valid = fixture('disc', 8, 17);
for (const options of [{ theta: -1 }, { theta: 1.01 }, { theta: NaN }, { softening: 0 },
  { softening: -1 }, { softening: 1e-300 }, { softening: Infinity }, { G: 0 }, { G: NaN },
  { bucket: 0 }, { bucket: 1.5 }, { maxDepth: 0 }, { maxDepth: 65 }, { maxDepth: 1.1 }])
  assert.throws(() => MUTUAL.accelerations(valid, options));
for (const bodies of [[], [valid[0]], null, [{ x: 0, y: 0, mass: 0 }, valid[0]],
  [{ x: NaN, y: 0, mass: 1 }, valid[0]], [{ x: 0, y: Infinity, mass: 1 }, valid[0]]])
  assert.throws(() => MUTUAL.accelerations(bodies));
for (const dt of [0, -1, Infinity, NaN]) assert.throws(() => MUTUAL.stepLeapfrog(valid, dt));
const invalidStep = clone(valid); invalidStep[0].vx = Infinity;
const beforeInvalid = structuredClone(invalidStep);
assert.throws(() => MUTUAL.stepLeapfrog(invalidStep, .001));
assert.deepEqual(invalidStep, beforeInvalid);
const overflowingStep = clone(valid); overflowingStep[0].vx = Number.MAX_VALUE;
const beforeOverflow = structuredClone(overflowingStep);
assert.throws(() => MUTUAL.stepLeapfrog(overflowingStep, 2));
assert.deepEqual(overflowingStep, beforeOverflow);

// Transactional commit preflight: a later unsupported destination must not leave
// earlier ordinary bodies advanced. No getters/setters are invoked by commit.
for (const kind of ['frozen', 'sealed-missing-velocity', 'readonly-x', 'setter-x', 'getter-vx']) {
  const state = [{ x: 0, y: 0, mass: 1 }, { x: 1, y: 0, mass: 2 }];
  if (kind === 'frozen') Object.freeze(state[1]);
  if (kind === 'sealed-missing-velocity') Object.seal(state[1]);
  if (kind === 'readonly-x') Object.defineProperty(state[1], 'x', { writable: false });
  if (kind === 'setter-x') Object.defineProperty(state[1], 'x', {
    get() { return 1; }, set() { throw new Error('setter must not be invoked'); }
  });
  if (kind === 'getter-vx') Object.defineProperty(state[1], 'vx', {
    get() { throw new Error('getter must not be invoked'); }
  });
  const saved = state.map(body => Object.getOwnPropertyDescriptors(body));
  assert.throws(() => MUTUAL.stepLeapfrog(state, .01), TypeError, kind);
  assert.deepEqual(state.map(body => Object.getOwnPropertyDescriptors(body)), saved, kind);
}
const ordinary = [{ x: 0, y: 0, vx: 0, vy: 0, mass: 1 }, { x: 1, y: 0, vx: 0, vy: 0, mass: 1 }];
Object.defineProperty(ordinary[0], 'annotation', { value: 'preserve', writable: false });
Object.seal(ordinary[1]);
MUTUAL.stepLeapfrog(ordinary, .001);
assert.equal(ordinary[0].annotation, 'preserve');
assert.notEqual(ordinary[1].x, 1);
let inheritedSetterCalls = 0;
const inherited = Object.create({ set vx(value) { inheritedSetterCalls++; }, get vy() { throw new Error('inherited getter'); } });
Object.assign(inherited, { x: 0, y: 0, mass: 1 });
MUTUAL.stepLeapfrog([inherited, { x: 1, y: 0, mass: 1 }], .001);
assert.equal(inheritedSetterCalls, 0);
assert.ok(Object.hasOwn(inherited, 'vx') && Object.hasOwn(inherited, 'vy'));

// Large-scale direct acceleration stays representable even if 1/r^3 does not.
const farBodies = [{ x: 0, y: 0, mass: 1 }, { x: 1e108, y: 0, mass: 1 }];
for (const theta of [0, .3]) {
  const far = MUTUAL.accelerations(farBodies, { theta }).values;
  assert.ok(Math.abs(far[0].ax / 1e-216 - 1) < 1e-14);
  assert.equal(far[1].ax, -far[0].ax);
}
const unitCell = { mass: 1, comX: 0, comY: 0, qxx: 0, qxy: 0, qyy: 0 };
const widePair = MUTUAL.pairedExpansion(unitCell, { ...unitCell, comX: 1e70 });
for (const [i, expected] of [[0, 1e-140], [2, 2e-210], [5, 6e-280]])
  assert.ok(Math.abs(widePair.a[i] / expected - 1) < 1e-14, `wide derivative ${i}`);
assert.throws(() => MUTUAL.pairedExpansion(unitCell, { ...unitCell, comX: 1e108 }), /supported f64 range/);
assert.throws(() => MUTUAL.accelerations(farBodies, { G: 1e-200, theta: 0 }), /supported f64 range/);

// Nonzero directional terms must not disappear before a large mass can
// recover a representable acceleration. Unsupported anisotropy fails closed.
const anisotropic = [{ x: 0, y: 0, mass: 1e100 }, { x: 1e-100, y: 1e100, mass: 1e100 }];
assert.throws(() => MUTUAL.accelerations(anisotropic, { theta: 0 }), /supported f64 range/);
assert.throws(() => MUTUAL.pairedExpansion({ ...unitCell, mass: 1e100 },
  { ...unitCell, mass: 1e100, comX: 1e-100, comY: 1e100 }), /supported f64 range/);
assert.throws(() => MUTUAL.accelerations([{ x: 0, y: 0, mass: 1 },
  { x: 1e-300, y: 1e100, mass: 1 }], { theta: 0 }), /supported f64 range/);
assert.throws(() => MUTUAL.accelerations([{ x: 0, y: 0, mass: 1 },
  { x: 1e-200, y: 0, mass: 1 }]), /supported f64 range/);

const initial = fixture('collision', 64, 17), dt = .001, steps = 16;
const fine = trajectory(initial, 'mutual', OPTIONS, dt / 2, steps * 2);
const coarse = trajectory(initial, 'mutual', OPTIONS, dt, steps);
const direct = trajectory(initial, 'direct', OPTIONS, dt, steps);
assert.equal(coarse.diagnostic.forceSolves, steps + 1);
assert.ok(coarse.diagnostic.maxNormalizedNetForce < BUDGET.normalizedNetForce);
assert.ok(coarse.diagnostic.maxMomentumDrift < 2e-13);
assert.ok(coarse.diagnostic.maxCOMBallisticDrift < 2e-13);
const oneStep = clone(initial); MUTUAL.stepLeapfrog(oneStep, dt, OPTIONS);
const shared = trajectory(initial, 'mutual', OPTIONS, dt, 1);
assert.ok(stateDifference(oneStep, shared.bodies).maxVelocity < 2e-15);
summary.trajectory = { coarse: coarse.diagnostic, fine: fine.diagnostic,
  versusDirect: stateDifference(coarse.bodies, direct.bodies), timestepControl: stateDifference(coarse.bodies, fine.bodies) };
console.log('Mutual-gravity CPU/f64 checks passed');
console.log(JSON.stringify(summary, null, 2));
