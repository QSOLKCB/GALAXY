// SPDX-License-Identifier: Apache-2.0
// Independent diagnostics shared by tests and the bounded evidence runner.
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import { compileFunction } from 'node:vm';
import { TextDecoder } from 'node:util';
import { fileURLToPath } from 'node:url';

export const ROOT = typeof __EVIDENCE_ROOT === 'undefined' ? new URL('../../', import.meta.url) : new URL(__EVIDENCE_ROOT);
// Read and hash both solver byte strings before evaluating either. Compile
// those exact UTF-8 bytes in the current native realm, never through require's
// mutable cache or an asymmetric vm context. These retained identities bind
// the executable functions to the later capture source-hash checks.
const loadedBytes = typeof __EVIDENCE_SOURCE_BASE64 === 'undefined'
  ? Object.fromEntries(['barnes-hut.js', 'mutual-gravity.js'].map(name => [name, fs.readFileSync(new URL(name, ROOT))]))
  : Object.fromEntries(Object.entries(__EVIDENCE_SOURCE_BASE64).map(([name, value]) => [name, Buffer.from(value, 'base64')]));
export const LOADED_SOURCE_SHA256 = Object.freeze(Object.fromEntries(
  Object.entries(loadedBytes).map(([name, bytes]) => [name, sha256(bytes)])));
export const VERIFIER_CHECK_COUNT = 166;
export const VERIFIER_COMPLETION = `Mutual-gravity evidence verifier checks passed (${VERIFIER_CHECK_COUNT} checks; deterministic fixtures)\n`;
export function loadedSource(name) {
  if (!Object.hasOwn(loadedBytes, name)) throw new Error(`Source was not captured: ${name}`);
  return new TextDecoder('utf-8', { fatal: true }).decode(loadedBytes[name]);
}
function evaluate(name, globalName) {
  const source = new TextDecoder('utf-8', { fatal: true }).decode(loadedBytes[name]);
  const module = { exports: {} }, namespace = Object.create(null);
  // Export names are private to this evaluation; existing real-global getters
  // or setters cannot substitute an old API. Intrinsics remain native-realm.
  compileFunction(source, ['module', 'exports', 'globalThis'], { filename: fileURLToPath(new URL(name, ROOT)) })(module, module.exports, namespace);
  const api = namespace[globalName];
  if (!api || typeof api.accelerations !== 'function' || (name === 'mutual-gravity.js' && module.exports !== api))
    throw new Error(`Solver did not publish its freshly evaluated API: ${name}`);
  return api;
}
export const BH = evaluate('barnes-hut.js', 'GalaxyBarnesHut');
export const MUTUAL = evaluate('mutual-gravity.js', 'GalaxyMutualGravity');
export const OPTIONS = Object.freeze({ G: 1, softening: 0.03, theta: 0.3, bucket: 4, maxDepth: 40 });
export const BUDGET = Object.freeze({ rmsRelative: 0.03, maxRelative: 0.25, normalizedNetForce: 5e-13 });
export function sha256(value) { return createHash('sha256').update(value).digest('hex'); }
export function clone(bodies) { return Array.from(bodies, body => ({ ...body })); }
export function quantile(sorted, q) {
  const at = (sorted.length - 1) * q, lo = Math.floor(at), hi = Math.ceil(at);
  return sorted[lo] + (sorted[hi] - sorted[lo]) * (at - lo);
}
export function errorMetrics(values, exact) {
  let squared = 0, absoluteSquared = 0, referenceSquared = 0;
  const relative = values.map((value, i) => {
    const delta = Math.hypot(value.ax - exact[i].ax, value.ay - exact[i].ay);
    const magnitude = Math.hypot(exact[i].ax, exact[i].ay);
    absoluteSquared += delta * delta; referenceSquared += magnitude * magnitude;
    const result = delta / Math.max(magnitude, 1e-30);
    squared += result * result;
    return result;
  }).sort((a, b) => a - b);
  return {
    rmsRelative: Math.sqrt(squared / relative.length),
    p50Relative: quantile(relative, 0.5), p90Relative: quantile(relative, 0.9),
    p95Relative: quantile(relative, 0.95), p99Relative: quantile(relative, 0.99),
    maxRelative: relative.at(-1), rmsAbsolute: Math.sqrt(absoluteSquared / relative.length),
    globalRelativeL2: Math.sqrt(absoluteSquared / Math.max(referenceSquared, 1e-60))
  };
}
export function forceDiagnostics(bodies, values) {
  let fx = 0, fy = 0, sumForceMagnitudes = 0, mass = 0, wx = 0, wy = 0;
  for (const b of bodies) { mass += b.mass; wx += b.mass * b.x; wy += b.mass * b.y; }
  const cx = wx / mass, cy = wy / mass;
  let torque = 0, torqueScale = 0;
  for (let i = 0; i < bodies.length; i++) {
    const b = bodies[i], a = values[i], f = b.mass * Math.hypot(a.ax, a.ay);
    fx += b.mass * a.ax; fy += b.mass * a.ay; sumForceMagnitudes += f;
    torque += b.mass * ((b.x - cx) * a.ay - (b.y - cy) * a.ax);
    torqueScale += Math.hypot(b.x - cx, b.y - cy) * f;
  }
  return { netForceX: fx, netForceY: fy, netForce: Math.hypot(fx, fy), sumForceMagnitudes,
    normalizedNetForce: Math.hypot(fx, fy) / Math.max(sumForceMagnitudes, 1e-30),
    torqueAboutCOM: torque, normalizedAbsTorque: Math.abs(torque) / Math.max(torqueScale, 1e-30) };
}
export function stateDiagnostics(bodies, options = OPTIONS) {
  let mass = 0, wx = 0, wy = 0, px = 0, py = 0, kinetic = 0, angularMomentum = 0;
  for (const b of bodies) {
    const vx = b.vx ?? 0, vy = b.vy ?? 0;
    mass += b.mass; wx += b.mass * b.x; wy += b.mass * b.y;
    px += b.mass * vx; py += b.mass * vy;
    kinetic += 0.5 * b.mass * (vx * vx + vy * vy);
    angularMomentum += b.mass * (b.x * vy - b.y * vx);
  }
  let potential = 0;
  for (let i = 0; i < bodies.length; i++) for (let j = i + 1; j < bodies.length; j++) {
    const a = bodies[i], b = bodies[j], dx = b.x - a.x, dy = b.y - a.y;
    potential -= options.G * a.mass * b.mass / Math.sqrt(dx * dx + dy * dy + options.softening ** 2);
  }
  return { mass, comX: wx / mass, comY: wy / mass, momentumX: px, momentumY: py,
    angularMomentum, kinetic, potential, energy: kinetic + potential };
}
export function solve(bodies, method, options = OPTIONS) {
  if (method === 'direct') return { values: BH.directAccelerations(bodies, options), stats: null, tree: null };
  if (method === 'barnes-hut') return BH.accelerations(bodies, options);
  if (method === 'mutual') return MUTUAL.accelerations(bodies, options);
  throw new Error(`Unknown force method ${method}`);
}
export function trajectory(initialBodies, method, options, dt, steps) {
  const bodies = clone(initialBodies), initial = stateDiagnostics(bodies, options);
  let current = solve(bodies, method, options), forceSolves = 1;
  let maxNormalizedNetForce = 0, maxNormalizedAbsTorque = 0;
  let maxRelativeEnergyDrift = 0, maxMomentumDrift = 0, maxCOMBallisticDrift = 0;
  let maxAngularMomentumDrift = 0;
  const checkpoints = [];
  function record(step) {
    const s = stateDiagnostics(bodies, options), f = forceDiagnostics(bodies, current.values);
    const relativeEnergyDrift = (s.energy - initial.energy) / Math.max(Math.abs(initial.energy), 1e-30);
    const momentumDrift = Math.hypot(s.momentumX - initial.momentumX, s.momentumY - initial.momentumY);
    const comBallisticDrift = Math.hypot(
      s.comX - initial.comX - step * dt * initial.momentumX / initial.mass,
      s.comY - initial.comY - step * dt * initial.momentumY / initial.mass);
    const angularMomentumDrift = s.angularMomentum - initial.angularMomentum;
    maxNormalizedNetForce = Math.max(maxNormalizedNetForce, f.normalizedNetForce);
    maxNormalizedAbsTorque = Math.max(maxNormalizedAbsTorque, f.normalizedAbsTorque);
    maxRelativeEnergyDrift = Math.max(maxRelativeEnergyDrift, Math.abs(relativeEnergyDrift));
    maxMomentumDrift = Math.max(maxMomentumDrift, momentumDrift);
    maxCOMBallisticDrift = Math.max(maxCOMBallisticDrift, comBallisticDrift);
    maxAngularMomentumDrift = Math.max(maxAngularMomentumDrift, Math.abs(angularMomentumDrift));
    if (step === 0 || step === steps || step % Math.max(1, Math.floor(steps / 4)) === 0)
      checkpoints.push({ step, time: step * dt, relativeEnergyDrift, momentumDrift,
        comBallisticDrift, angularMomentumDrift, ...f });
  }
  record(0);
  for (let step = 1; step <= steps; step++) {
    for (let i = 0; i < bodies.length; i++) {
      const b = bodies[i], a = current.values[i];
      b.vx = (b.vx ?? 0) + 0.5 * dt * a.ax; b.vy = (b.vy ?? 0) + 0.5 * dt * a.ay;
      b.x += dt * b.vx; b.y += dt * b.vy;
    }
    current = solve(bodies, method, options); forceSolves++;
    for (let i = 0; i < bodies.length; i++) {
      bodies[i].vx += 0.5 * dt * current.values[i].ax;
      bodies[i].vy += 0.5 * dt * current.values[i].ay;
    }
    record(step);
  }
  return { bodies, diagnostic: { method, dt, steps, duration: dt * steps, forceSolves,
    initial, final: stateDiagnostics(bodies, options), maxRelativeEnergyDrift, maxMomentumDrift,
    maxCOMBallisticDrift, maxAngularMomentumDrift, maxNormalizedNetForce, maxNormalizedAbsTorque,
    finalForceError: errorMetrics(current.values, BH.directAccelerations(bodies, options)), checkpoints } };
}
export function stateDifference(bodies, reference) {
  let positionSquared = 0, velocitySquared = 0, maxPosition = 0, maxVelocity = 0;
  for (let i = 0; i < bodies.length; i++) {
    const dp = Math.hypot(bodies[i].x - reference[i].x, bodies[i].y - reference[i].y);
    const dv = Math.hypot(bodies[i].vx - reference[i].vx, bodies[i].vy - reference[i].vy);
    positionSquared += dp * dp; velocitySquared += dv * dv;
    maxPosition = Math.max(maxPosition, dp); maxVelocity = Math.max(maxVelocity, dv);
  }
  return { rmsPosition: Math.sqrt(positionSquared / bodies.length), maxPosition,
    rmsVelocity: Math.sqrt(velocitySquared / bodies.length), maxVelocity };
}
export function fixture(preset, count, seed) {
  return clone(preset === 'disc' ? BH.makeDisc(count, seed) : BH.makeCollision(count, seed));
}
