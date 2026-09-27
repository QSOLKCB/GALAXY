// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const context = { console };
context.globalThis = context;
vm.runInNewContext(fs.readFileSync(new URL("../barnes-hut.js", import.meta.url), "utf8"), context, { filename: "barnes-hut.js" });
const BH = context.GalaxyBarnesHut;

const bodies = BH.makeDisc(256, 303);
const tree = BH.buildTree(bodies, { bucket: 4 });
assert.equal(tree.bodyCount, 256);
assert.ok(tree.nodeCount > tree.leafCount);
assert.ok(tree.maxDepth > 0 && tree.maxDepth < 40);
assert.equal(tree.root.count, 256);
assert.ok(Math.abs(tree.root.mass - 1) < 1e-12);

const thetaZero = BH.accelerations(bodies, { theta: 0, softening: 0.03 });
const direct = BH.directAccelerations(bodies, { softening: 0.03 });
for (let i = 0; i < bodies.length; i++) {
  assert.ok(Math.abs(thetaZero.values[i].ax - direct[i].ax) < 1e-10);
  assert.ok(Math.abs(thetaZero.values[i].ay - direct[i].ay) < 1e-10);
}

const error = BH.errorAgainstDirect(bodies, { theta: 0.5, softening: 0.03 });
assert.ok(error.rmsRelative < 0.03, `RMS relative acceleration error ${error.rmsRelative}`);
assert.ok(error.maxRelative < 0.25, `max relative acceleration error ${error.maxRelative}`);
assert.ok(error.stats.approximated > 0);
assert.ok(error.stats.direct > 0);

const duplicate = [
  { x: 0, y: 0, vx: 0, vy: 0, mass: 1 },
  { x: 0, y: 0, vx: 0, vy: 0, mass: 1 },
  { x: 0, y: 0, vx: 0, vy: 0, mass: 1 }
];
const dupTree = BH.buildTree(duplicate, { bucket: 1, maxDepth: 12 });
assert.ok(dupTree.maxDepth <= 12);
const dupAcc = BH.accelerations(duplicate, { theta: 0.5, softening: 0.1, bucket: 1, maxDepth: 12 });
for (const a of dupAcc.values) assert.ok(Number.isFinite(a.ax) && Number.isFinite(a.ay));

const before = bodies.map(b => ({ ...b }));
BH.stepLeapfrog(bodies, 0.001, { theta: 0.5, softening: 0.03 });
assert.ok(bodies.some((b, i) => b.x !== before[i].x || b.y !== before[i].y));
assert.ok(bodies.every(b => [b.x, b.y, b.vx, b.vy].every(Number.isFinite)));

const collisionA = BH.makeCollision(128, 99);
const collisionB = BH.makeCollision(128, 99);
assert.deepEqual(JSON.parse(JSON.stringify(collisionA)), JSON.parse(JSON.stringify(collisionB)));
assert.equal(BH.flattenTree(BH.buildTree(collisionA), 2)[0].count, 128);

console.log("Barnes-Hut browser reference checks passed", {
  nodes: tree.nodeCount,
  leaves: tree.leafCount,
  depth: tree.maxDepth,
  rmsRelative: error.rmsRelative,
  maxRelative: error.maxRelative,
  approximated: error.stats.approximated
});
