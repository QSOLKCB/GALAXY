#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Bounded, source-bound CPU/f64 evidence. Raw timings are observations, not speed claims.
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import { performance } from 'node:perf_hooks';
import { spawnSync } from 'node:child_process';
import { ROOT, BH, MUTUAL, OPTIONS, BUDGET, sha256, errorMetrics, forceDiagnostics,
  fixture, trajectory, stateDifference, quantile } from './lib/mutual-gravity-evidence.mjs';

const root = fileURLToPath(ROOT);
const SOURCE_FILES = ['barnes-hut.js', 'mutual-gravity.js', 'tests/mutual-gravity.mjs',
  'scripts/bench-mutual-gravity.mjs', 'scripts/lib/mutual-gravity-evidence.mjs'];
function sourceHashes() {
  return Object.fromEntries(SOURCE_FILES.map(p => [p, sha256(fs.readFileSync(path.join(root, p)))]));
}
function sameHashes(a, b) {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error('Source changed during capture or does not match receipt');
}
function assert(condition, message) { if (!condition) throw new Error(message); }
function strictJSON(data) {
  return JSON.stringify(data, (key, value) => {
    if (typeof value === 'number' && !Number.isFinite(value)) throw new Error(`Non-finite JSON field ${key}`);
    return value;
  }, 2) + '\n';
}
function writeNew(output, name, data) { fs.writeFileSync(path.join(output, name), data, { flag: 'wx' }); }
function qualify(error, force, name) {
  assert(error.rmsRelative < BUDGET.rmsRelative, `${name}: RMS budget failed`);
  assert(error.maxRelative < BUDGET.maxRelative, `${name}: maximum-relative budget failed`);
  assert(force.normalizedNetForce < BUDGET.normalizedNetForce, `${name}: linear-momentum budget failed`);
}
function describe(result, bodies, exact) {
  return { error: errorMetrics(result.values, exact), force: forceDiagnostics(bodies, result.values),
    stats: result.stats, tree: result.tree ? { nodes: result.tree.nodeCount, leaves: result.tree.leafCount,
      maxDepth: result.tree.maxDepth, bodyCount: result.tree.bodyCount,
      localCoefficientFloat64PayloadBytes: result.tree.root.local ? result.tree.nodeCount * 9 * 8 : 0 } : null };
}
function timedSolves(bodies, matchedTheta) {
  const optionsBH = { ...OPTIONS, theta: matchedTheta };
  const solvers = {
    mutual: () => MUTUAL.accelerations(bodies, OPTIONS),
    barnesHutMatched: () => BH.accelerations(bodies, optionsBH),
    direct: () => BH.directAccelerations(bodies, OPTIONS)
  };
  const samples = Object.fromEntries(Object.keys(solvers).map(key => [key, []]));
  const memory = Object.fromEntries(Object.keys(solvers).map(key => [key, []]));
  // Every call allocates and computes the whole result. Tree building, moment
  // generation, interaction traversal and local propagation are all included.
  for (let run = 0; run < 11; run++) {
    const keys = Object.keys(solvers); if (run % 2) keys.reverse();
    for (const key of keys) {
      const before = process.memoryUsage();
      const started = performance.now(); const result = solvers[key](); const ms = performance.now() - started;
      const after = process.memoryUsage();
      assert((Array.isArray(result) ? result : result.values).length === bodies.length, 'Incomplete timed solve');
      if (run >= 2) {
        samples[key].push(ms);
        memory[key].push({ heapUsedBefore: before.heapUsed, heapUsedAfter: after.heapUsed,
          heapUsedDelta: after.heapUsed - before.heapUsed, rssDelta: after.rss - before.rss,
          arrayBuffersDelta: after.arrayBuffers - before.arrayBuffers });
      }
    }
  }
  return Object.fromEntries(Object.entries(samples).map(([key, values]) => [key, {
    memoryObservations: memory[key], samplesMs: values, medianMs: quantile([...values].sort((a, b) => a - b), .5),
    p90Ms: quantile([...values].sort((a, b) => a - b), .9), warmups: 2, repeats: values.length
  }]));
}
function calibrateBH(bodies, exact, target) {
  const candidates = [];
  for (let i = 0; i <= 40; i++) {
    const theta = i / 40, result = BH.accelerations(bodies, { ...OPTIONS, theta });
    const error = errorMetrics(result.values, exact);
    candidates.push({ theta, error });
  }
  // Choose closest achieved RMS among candidates that meet the inherited
  // RMS/worst gates. Matching is declared only within an explicit 25% band.
  const qualified = candidates.filter(c => c.error.rmsRelative < BUDGET.rmsRelative && c.error.maxRelative < BUDGET.maxRelative);
  qualified.sort((a, b) => Math.abs(Math.log(Math.max(a.error.rmsRelative, 1e-30) / target.rmsRelative)) -
    Math.abs(Math.log(Math.max(b.error.rmsRelative, 1e-30) / target.rmsRelative)));
  const selected = qualified[0];
  assert(selected, 'No BH calibration point satisfies baseline budgets');
  const targetDirectEquivalent = target.rmsRelative < 1e-12;
  const rmsRatioBHToMutual = selected.error.rmsRelative / Math.max(target.rmsRelative, 1e-30);
  const p99RatioBHToMutual = selected.error.p99Relative / Math.max(target.p99Relative, 1e-30);
  return { theta: selected.theta, targetDirectEquivalent, rmsRatioBHToMutual, p99RatioBHToMutual,
    matchedRMS: !targetDirectEquivalent && rmsRatioBHToMutual >= .8 && rmsRatioBHToMutual <= 1.25,
    tailMatchedWithin25Percent: p99RatioBHToMutual >= .8 && p99RatioBHToMutual <= 1.25,
    candidates };
}
function verify(output) {
  const manifest = JSON.parse(fs.readFileSync(path.join(output, 'manifest.json'), 'utf8'));
  strictJSON(manifest);
  assert(manifest.schema === 'galaxy.mutual-gravity-manifest.v1', 'Wrong manifest schema');
  assert(manifest.status === 'passed', 'Capture was not successful');
  sameHashes(manifest.sourceSHA256, sourceHashes());
  assert(Object.keys(manifest.files).sort().join(',') === 'receipt.json,tests.log', 'Unexpected receipt file set');
  for (const [name, expected] of Object.entries(manifest.files))
    assert(sha256(fs.readFileSync(path.join(output, name))) === expected, `${name}: hash mismatch`);
  const receipt = JSON.parse(fs.readFileSync(path.join(output, 'receipt.json'), 'utf8'));
  strictJSON(receipt);
  assert(receipt.schema === 'galaxy.mutual-gravity-receipt.v1', 'Wrong receipt schema');
  sameHashes(receipt.sourceSHA256, manifest.sourceSHA256);
  assert(receipt.status === 'passed', 'Receipt was not successful');
  assert(JSON.stringify(receipt.configuration) === JSON.stringify(OPTIONS), 'Unexpected solver configuration');
  assert(JSON.stringify(receipt.budgets) === JSON.stringify(BUDGET), 'Unexpected qualification budgets');
  assert(receipt.forceCases.length === 14 && receipt.trajectoryCases.length === 6 && receipt.smallNCases.length === 4, 'Incomplete workload matrix');
  assert(new Set(receipt.forceCases.map(row => row.id)).size === 14, 'Duplicate force-case identity');
  for (const row of receipt.forceCases) {
    qualify(row.mutual.error, row.mutual.force, row.id);
    assert(row.mutual.stats.approximated > 0, `${row.id}: no approximate interaction`);
    assert(row.mutual.stats.direct > 0, `${row.id}: no direct interaction`);
  }
  for (const row of receipt.smallNCases) qualify(row.mutual.error, row.mutual.force, row.id);
  for (const row of receipt.trajectoryCases) for (const method of ['direct', 'mutual', 'barnes-hut']) {
    for (const [resolution, dt, steps] of [['coarse', .001, 64], ['fine', .0005, 128]]) {
      const d = row.diagnostics[method][resolution];
      assert(d.dt === dt && d.steps === steps && d.forceSolves === steps + 1 && d.duration === .064,
        `${row.id}: malformed trajectory protocol`);
      if (method === 'mutual') {
        assert(d.maxNormalizedNetForce < BUDGET.normalizedNetForce && d.maxMomentumDrift < 2e-12 && d.maxCOMBallisticDrift < 2e-12,
          `${row.id}: trajectory conservation gate failed`);
      }
    }
  }
  assert(receipt.tests.exitCode === 0, 'Tests did not pass');
  console.log(`Verified source hashes and artifact hashes: ${output}`);
}

let output = path.join(root, 'evidence', 'mutual-gravity-2026-10-15'), verification = false;
for (let i = 2; i < process.argv.length; i++) {
  const arg = process.argv[i];
  if (arg === '--output' && process.argv[i + 1]) output = path.resolve(process.argv[++i]);
  else if (arg === '--verify' && process.argv[i + 1]) { output = path.resolve(process.argv[++i]); verification = true; }
  else if (arg === '--help') {
    console.log('Usage: node scripts/bench-mutual-gravity.mjs [--output NEW_DIRECTORY]\n       node scripts/bench-mutual-gravity.mjs --verify DIRECTORY');
    process.exit(0);
  } else throw new Error(`Unknown or incomplete argument: ${arg}`);
}
if (verification) { verify(output); process.exit(0); }
if (fs.existsSync(output)) throw new Error(`Refusing to overwrite existing evidence: ${output}`);
fs.mkdirSync(path.dirname(output), { recursive: true }); fs.mkdirSync(output);
const sourceSHA256 = sourceHashes(), startedAt = new Date().toISOString();
try {
  const tested = spawnSync(process.execPath, ['tests/mutual-gravity.mjs'], {
    cwd: root, encoding: 'utf8', maxBuffer: 8 * 1024 * 1024, timeout: 120000
  });
  writeNew(output, 'tests.log', (tested.stdout || '') + (tested.stderr || ''));
  assert(tested.status === 0 && !tested.error, `Numerical tests failed: ${tested.error || tested.status}`);
  const forceCases = [];
  const workloads = [];
  for (const count of [128, 512]) for (const seed of [17, 303, 991]) for (const preset of ['disc', 'collision'])
    workloads.push({ count, seed, preset });
  for (const preset of ['disc', 'collision']) workloads.push({ count: 2048, seed: 303, preset });
  for (const { count, seed, preset } of workloads) {
    const id = `${preset}-${count}-${seed}`, bodies = fixture(preset, count, seed);
    const exact = BH.directAccelerations(bodies, OPTIONS), mutual = describe(MUTUAL.accelerations(bodies, OPTIONS), bodies, exact);
    qualify(mutual.error, mutual.force, id);
    assert(mutual.stats.approximated > 0 && mutual.stats.direct > 0, `${id}: inactive mixed interaction path`);
    const calibration = calibrateBH(bodies, exact, mutual.error);
    const barnesHutMatched = describe(BH.accelerations(bodies, { ...OPTIONS, theta: calibration.theta }), bodies, exact);
    const barnesHutTheta05 = describe(BH.accelerations(bodies, { ...OPTIONS, theta: .5 }), bodies, exact);
    forceCases.push({ id, count, seed, preset, initialStateSHA256: sha256(JSON.stringify(bodies)),
      mutual, direct: { force: forceDiagnostics(bodies, exact) }, barnesHutTheta05, calibration,
      barnesHutMatched, wholeSolveTimings: timedSolves(bodies, calibration.theta) });
    console.error(`${id}: RMS ${mutual.error.rmsRelative.toExponential(3)}, max ${mutual.error.maxRelative.toExponential(3)}, net force ${mutual.force.normalizedNetForce.toExponential(3)}, matched BH theta ${calibration.theta}`);
  }
  const smallNCases = [];
  for (const count of [8, 32]) for (const preset of ['disc', 'collision']) {
    const seed = 303, id = `${preset}-${count}-${seed}`, bodies = fixture(preset, count, seed);
    const exact = BH.directAccelerations(bodies, OPTIONS), mutual = describe(MUTUAL.accelerations(bodies, OPTIONS), bodies, exact);
    qualify(mutual.error, mutual.force, id);
    const calibration = calibrateBH(bodies, exact, mutual.error);
    smallNCases.push({ id, count, preset, seed, initialStateSHA256: sha256(JSON.stringify(bodies)), mutual,
      calibration, barnesHutMatched: describe(BH.accelerations(bodies, { ...OPTIONS, theta: calibration.theta }), bodies, exact),
      wholeSolveTimings: timedSolves(bodies, calibration.theta) });
  }
  const trajectoryCases = [];
  for (const seed of [17, 303, 991]) for (const preset of ['disc', 'collision']) {
    const id = `${preset}-128-${seed}`, bodies = fixture(preset, 128, seed);
    const bhTheta = forceCases.find(row => row.id === id).calibration.theta;
    const runs = {};
    for (const method of ['direct', 'mutual', 'barnes-hut']) {
      const options = { ...OPTIONS, theta: method === 'barnes-hut' ? bhTheta : OPTIONS.theta };
      runs[method] = {
        coarse: trajectory(bodies, method, options, .001, 64),
        fine: trajectory(bodies, method, options, .0005, 128)
      };
    }
    const diagnostics = {};
    for (const method of ['direct', 'mutual', 'barnes-hut']) {
      diagnostics[method] = {
        coarse: runs[method].coarse.diagnostic, fine: runs[method].fine.diagnostic,
        timestepControl: stateDifference(runs[method].coarse.bodies, runs[method].fine.bodies),
        versusDirectSameDt: stateDifference(runs[method].coarse.bodies, runs.direct.coarse.bodies),
        versusDirectHalfDt: stateDifference(runs[method].fine.bodies, runs.direct.fine.bodies)
      };
    }
    for (const resolution of ['coarse', 'fine']) {
      assert(runs.mutual[resolution].diagnostic.maxNormalizedNetForce < BUDGET.normalizedNetForce,
        `${id}: trajectory net force exceeded tolerance`);
      assert(runs.mutual[resolution].diagnostic.maxMomentumDrift < 2e-12, `${id}: trajectory momentum drift`);
      assert(runs.mutual[resolution].diagnostic.maxCOMBallisticDrift < 2e-12, `${id}: trajectory COM drift`);
    }
    trajectoryCases.push({ id, count: 128, seed, preset, barnesHutTheta: bhTheta, diagnostics });
    console.error(`${id}: KDK dt and dt/2 trajectory diagnostics complete`);
  }
  sameHashes(sourceSHA256, sourceHashes());
  const receipt = {
    schema: 'galaxy.mutual-gravity-receipt.v1', status: 'passed', startedAt, completedAt: new Date().toISOString(),
    baselineSourceCommit: 'e051324b1263f20ebb88d0e585992c7c1838504a', sourceSHA256,
    environment: { node: process.version, platform: process.platform, arch: process.arch,
      cpuModel: os.cpus()[0]?.model ?? 'unknown', cpuCount: os.cpus().length },
    command: ['node', 'scripts/bench-mutual-gravity.mjs', '--output', path.relative(root, output)],
    scope: 'Bounded CPU/JavaScript Number (binary64), planar Plummer kernel; no GPU execution, production promotion, or throughput claim.',
    configuration: OPTIONS, budgets: BUDGET,
    protocol: {
      selection: 'Mutual theta 0.3 selected in a preceding pilot sweep; these seeded states are qualification fixtures, not an independent held-out accuracy estimate.',
      smallN: 'Four additional N8/32 disc/collision cases retain whole-solve overhead and direct-equivalent behavior; they are not scaling evidence.',
      memory: 'Before/after process.memoryUsage snapshots bracket whole solves outside the timed spans while the returned tree remains live. HeapUsed/RSS/arrayBuffers deltas are noisy process observations, can be negative from GC, and are not peak or isolated retained-allocation estimates. Tree node/leaf counts and 9*8 bytes/node of mutual local-coefficient payload are reproducible structural diagnostics, not total memory.',
      forceWorkloads: '12 seeded disc/collision states at N128/512 plus two bounded N2048 states; full independent symmetric direct forces for every body.',
      accuracy: 'Per-body relative errors use max(|direct acceleration|, 1e-30); p50/p90/p95/p99 use linear interpolation. The inherited browser BH gates are RMS < 0.03 and max < 0.25.',
      balance: 'Normalized internal force is |sum m*a| / max(sum m*|a|,1e-30). No mean-force subtraction or post-hoc balancing.',
      matching: 'BH theta grid 0:0.025:1; choose closest achieved RMS among inherited-budget-passing candidates. RMS match means BH/mutual ratio in [0.8,1.25]. p99 and maximum are separately reported; matching is per state, not a portable theta equivalence.',
      timing: 'Both implementations execute in the same native Node realm (no asymmetric vm wrapper). Two warmup solves and nine measured solves, alternating method order. Timed whole calls include validation, allocations, fresh tree construction, moments, traversal, and propagation. Diagnostics, direct oracle comparison, and calibration are outside the timed call. No forced GC; JIT/GC/scheduling noise remains. Raw wall times only.',
      integration: 'Shared-step kick-drift-kick with a fresh force state after every drift and cached boundary acceleration: steps+1 solves. Six N128 states, duration 0.064, dt=.001 and dt/2=.0005. Every step measures exact all-pairs Plummer potential plus kinetic energy, momentum, ballistic COM, angular momentum, and torque.',
      conservationBoundary: 'Linear momentum qualification does not imply angular momentum or energy conservation. Torque and energy are diagnostics, not qualification guarantees; tree switching can affect both.'
    },
    tests: { command: ['node', 'tests/mutual-gravity.mjs'], exitCode: tested.status, log: 'tests.log' },
    summary: {
      forceCases: forceCases.length, trajectoryCases: trajectoryCases.length,
      maxMutualRMSRelative: Math.max(...forceCases.map(row => row.mutual.error.rmsRelative)),
      maxMutualMaxRelative: Math.max(...forceCases.map(row => row.mutual.error.maxRelative)),
      maxMutualNormalizedNetForce: Math.max(...forceCases.map(row => row.mutual.force.normalizedNetForce)),
      maxDirectNormalizedNetForce: Math.max(...forceCases.map(row => row.direct.force.normalizedNetForce)),
      baselineBHNormalizedNetForceRange: [Math.min(...forceCases.slice(0, 12).map(row => row.barnesHutTheta05.force.normalizedNetForce)),
        Math.max(...forceCases.slice(0, 12).map(row => row.barnesHutTheta05.force.normalizedNetForce))],
      matchedRMSCases: forceCases.filter(row => row.calibration.matchedRMS).length,
      matchedRMSAndP99Cases: forceCases.filter(row => row.calibration.matchedRMS && row.calibration.tailMatchedWithin25Percent).length,
      maxTrajectoryMomentumDrift: Math.max(...trajectoryCases.flatMap(row => ['coarse', 'fine'].map(r => row.diagnostics.mutual[r].maxMomentumDrift))),
      maxTrajectoryCOMBallisticDrift: Math.max(...trajectoryCases.flatMap(row => ['coarse', 'fine'].map(r => row.diagnostics.mutual[r].maxCOMBallisticDrift)))
    }, forceCases, smallNCases, trajectoryCases
  };
  writeNew(output, 'receipt.json', strictJSON(receipt));
  writeNew(output, 'manifest.json', strictJSON({ schema: 'galaxy.mutual-gravity-manifest.v1', status: 'passed',
    sourceSHA256, files: Object.fromEntries(['receipt.json', 'tests.log'].map(name => [name, sha256(fs.readFileSync(path.join(output, name)))])) }));
  verify(output);
  console.log(JSON.stringify(receipt.summary, null, 2));
} catch (error) {
  writeNew(output, 'failed.json', strictJSON({ schema: 'galaxy.mutual-gravity-failed.v1', status: 'failed',
    startedAt, failedAt: new Date().toISOString(), sourceSHA256, error: String(error) }));
  throw error;
}
