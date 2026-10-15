// SPDX-License-Identifier: Apache-2.0
// Verifier contract tests recompute force records; timings remain synthetic metadata.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { verify, sourceHashes, parseArtifact, recomputeForceCase, recomputeTrajectoryCase, summarizeCases } from '../scripts/bench-mutual-gravity.mjs';
import { ROOT, OPTIONS, BUDGET, fixture, sha256, VERIFIER_CHECK_COUNT, VERIFIER_COMPLETION } from '../scripts/lib/mutual-gravity-evidence.mjs';

const root = fileURLToPath(ROOT), temp = fs.mkdtempSync(path.join(os.tmpdir(), 'galaxy-mutual-verifier-'));
const sources = sourceHashes();
function identity(preset, count, seed) {
  return { id: `${preset}-${count}-${seed}`, preset, count, seed,
    initialStateSHA256: sha256(JSON.stringify(fixture(preset, count, seed))) };
}
const forceCases = [], smallNCases = [], trajectoryCases = [];
function forceRow(preset, count, seed) {
  return { ...identity(preset, count, seed), ...recomputeForceCase(preset, count, seed),
    wholeSolveTimings: Object.fromEntries(['mutual', 'barnesHutMatched', 'direct'].map(method => [method, {
      warmups: 2, repeats: 9, samplesMs: Array(9).fill(1), medianMs: 1, p90Ms: 1,
      memoryObservations: Array.from({ length: 9 }, () => ({ heapUsedBefore: 10, heapUsedAfter: 12, heapUsedDelta: 2, rssDelta: 0, arrayBuffersDelta: 0 }))
    }])) };
}
for (const count of [128, 512]) for (const seed of [17, 303, 991]) for (const preset of ['disc', 'collision'])
  forceCases.push(forceRow(preset, count, seed));
for (const preset of ['disc', 'collision']) forceCases.push(forceRow(preset, 2048, 303));
for (const count of [8, 32]) for (const preset of ['disc', 'collision']) smallNCases.push(forceRow(preset, count, 303));
for (const seed of [17, 303, 991]) for (const preset of ['disc', 'collision']) {
  trajectoryCases.push({ ...identity(preset, 128, seed), ...recomputeTrajectoryCase(preset, 128, seed) });
}

const testCommands = [['node', 'tests/mutual-gravity.mjs'], ['node', 'tests/mutual-gravity-evidence.mjs']];
const testLog = { schema: 'galaxy.mutual-gravity-test-log.v1', runs: [
  { command: testCommands[0], execution: 'retained-source-esm-stdin-v1', exitCode: 0, signal: null, error: null, stderr: '',
    stdout: 'Mutual-gravity CPU/f64 checks passed\n' + JSON.stringify({ cases: Array.from({ length: 27 }, () => ({ synthetic: true })),
      pairedPotential: { synthetic: true }, trajectory: { synthetic: true } }) + '\n' },
  { command: testCommands[1], execution: 'retained-source-esm-stdin-v1', exitCode: 0, signal: null, error: null, stderr: '',
    stdout: VERIFIER_COMPLETION }
] };
function outcome(run) {
  return { command: run.command, execution: run.execution, exitCode: run.exitCode, signal: run.signal, error: run.error,
    stdoutSHA256: sha256(run.stdout), stderrSHA256: sha256(run.stderr) };
}
const base = { schema: 'galaxy.mutual-gravity-receipt.v1', status: 'passed', sourceSHA256: sources,
  configuration: OPTIONS, budgets: BUDGET, forceCases, smallNCases, trajectoryCases,
  tests: { exitCode: 0, log: 'tests.log', commands: testCommands, outcomes: testLog.runs.map(outcome) }, syntheticVerifierTestFixture: true,
  summary: summarizeCases(forceCases, trajectoryCases) };
let passed = 0;
function writeFixture(name, mutate = () => {}, options = {}) {
  const directory = path.join(temp, name); fs.mkdirSync(directory);
  const receipt = structuredClone(base);
  const manifest = { schema: 'galaxy.mutual-gravity-manifest.v1', status: 'passed', sourceSHA256: structuredClone(sources), files: {} };
  mutate(receipt, manifest);
  let serialized = JSON.stringify(receipt);
  if (options.nonfinite) serialized = serialized.replace('"syntheticVerifierTestFixture":true', '"syntheticVerifierTestFixture":1e999');
  if (options.rawReceipt) serialized = options.rawReceipt(serialized);
  fs.writeFileSync(path.join(directory, 'receipt.json'), serialized);
  const log = structuredClone(testLog);
  if (options.mutateLog) options.mutateLog(log);
  const logText = options.rawLog ? options.rawLog(JSON.stringify(log)) : JSON.stringify(log);
  fs.writeFileSync(path.join(directory, 'tests.log'), logText);
  manifest.files = Object.fromEntries(['receipt.json', 'tests.log'].map(name => [name, sha256(fs.readFileSync(path.join(directory, name)))]));
  if (options.badFileHash) manifest.files['receipt.json'] = '0'.repeat(64);
  let serializedManifest = JSON.stringify(manifest);
  if (options.rawManifest) serializedManifest = options.rawManifest(serializedManifest);
  fs.writeFileSync(path.join(directory, 'manifest.json'), serializedManifest);
  return directory;
}
function rejected(name, mutate, pattern, options) {
  const directory = writeFixture(name, mutate, options);
  assert.throws(() => verify(directory, { quiet: true }), pattern, name); passed++;
}
try {
  const valid = writeFixture('valid');
  verify(valid, { quiet: true }); passed++;
  // Identity is a set contract, not a storage-order contract.
  verify(writeFixture('valid-reordered', r => {
    r.forceCases.reverse(); r.smallNCases.reverse(); r.trajectoryCases.reverse();
    r.summary = Object.fromEntries(Object.entries(r.summary).reverse());
  }), { quiet: true }); passed++;
  for (const matrix of ['forceCases', 'smallNCases', 'trajectoryCases']) {
    rejected(`${matrix}-duplicate`, r => { r[matrix][1] = structuredClone(r[matrix][0]); }, /duplicate workload identity/);
    // A same-length matrix with a valid but unrequested seed and a valid hash
    // must fail even after its manifest has been correctly rehashed.
    rejected(`${matrix}-substituted`, r => {
      const row = r[matrix][0]; Object.assign(row, identity(row.preset, row.count, 997));
    }, /unexpected workload identity/);
    rejected(`${matrix}-mislabeled-id`, r => { r[matrix][0].id = 'other-state'; }, /unexpected workload identity/);
    rejected(`${matrix}-wrong-preset`, r => { r[matrix][0].preset = 'collision'; }, /unexpected workload identity/);
    rejected(`${matrix}-wrong-count`, r => { r[matrix][0].count += 1; }, /unexpected workload identity/);
    rejected(`${matrix}-wrong-seed`, r => { r[matrix][0].seed += 1; }, /unexpected workload identity/);
    rejected(`${matrix}-wrong-state-hash`, r => { r[matrix][0].initialStateSHA256 = '0'.repeat(64); }, /initial-state hash mismatch/);
    rejected(`${matrix}-missing-state-hash`, r => { delete r[matrix][0].initialStateSHA256; }, /initial-state hash mismatch/);
    rejected(`${matrix}-missing-case`, r => { r[matrix].pop(); }, /incomplete workload matrix/);
    rejected(`${matrix}-not-array`, r => { r[matrix] = null; }, /incomplete workload matrix/);
  }
  rejected('file-hash', () => {}, /hash mismatch/, { badFileHash: true });
  rejected('manifest-source-hash', (_r, m) => { m.sourceSHA256['mutual-gravity.js'] = '0'.repeat(64); }, /Source changed/);
  rejected('receipt-source-hash', r => { r.sourceSHA256['mutual-gravity.js'] = '0'.repeat(64); }, /Source changed/);
  rejected('manifest-schema', (_r, m) => { m.schema = 'wrong'; }, /Wrong manifest schema/);
  rejected('receipt-schema', r => { r.schema = 'wrong'; }, /Wrong receipt schema/);
  rejected('nonfinite', () => {}, /Non-finite JSON field/, { nonfinite: true });
  rejected('negative-force-metric', r => { r.forceCases[0].mutual.error.rmsRelative = -1; }, /invalid qualification metric/);
  rejected('null-force-metric', r => { r.smallNCases[0].mutual.error.maxRelative = null; }, /invalid qualification metric/);
  rejected('string-force-metric', r => { r.forceCases[0].mutual.force.normalizedNetForce = '0'; }, /invalid qualification metric/);
  rejected('negative-trajectory-metric', r => { r.trajectoryCases[0].diagnostics.mutual.coarse.maxMomentumDrift = -1; }, /invalid trajectory metric/);
  rejected('force-gate', r => { r.forceCases[0].mutual.error.rmsRelative = 1; }, /RMS budget failed/);
  rejected('small-n-gate', r => { r.smallNCases[0].mutual.error.maxRelative = 1; }, /maximum-relative budget failed/);
  rejected('net-force-gate', r => { r.forceCases[0].mutual.force.normalizedNetForce = 1; }, /linear-momentum budget failed/);
  rejected('trajectory-gate', r => { r.trajectoryCases[0].diagnostics.mutual.coarse.maxMomentumDrift = 1; }, /trajectory conservation gate failed/);
  rejected('trajectory-protocol', r => { r.trajectoryCases[0].diagnostics.direct.fine.steps = 127; }, /malformed trajectory protocol/);
  rejected('configuration', r => { r.configuration.theta = .5; }, /Unexpected solver configuration/);
  rejected('budgets', r => { r.budgets.rmsRelative = 1; }, /Unexpected qualification budgets/);
  rejected('tests-failed', r => { r.tests.exitCode = 1; }, /Tests require the exact successful command set/);
  // Every summary field is recomputed; a correctly rehashed summary cannot
  // inflate success counts or alter published error/conservation claims.
  for (const key of Object.keys(base.summary)) {
    rejected(`summary-${key}`, r => {
      if (Array.isArray(r.summary[key])) r.summary[key][0] += 1;
      else r.summary[key] += 1;
    }, /Receipt summary does not exactly match/);
  }
  rejected('summary-missing-field', r => { delete r.summary.maxMutualRMSRelative; }, /Receipt summary does not exactly match/);
  rejected('summary-extra-field', r => { r.summary.extra = 0; }, /Receipt summary does not exactly match/);
  rejected('summary-wrong-type', r => { r.summary.forceCases = '14'; }, /Receipt summary does not exactly match/);
  rejected('summary-missing', r => { delete r.summary; }, /Receipt summary does not exactly match/);
  rejected('summary-input-negative-direct', r => { r.forceCases[0].direct.force.normalizedNetForce = -1; }, /invalid summary input/);
  rejected('summary-input-negative-bh', r => { r.forceCases[0].barnesHutTheta05.force.normalizedNetForce = -1; }, /invalid summary input/);
  rejected('summary-input-calibration-flag', r => { r.forceCases[0].calibration.matchedRMS = !r.forceCases[0].calibration.matchedRMS; }, /inconsistent calibration selection/);
  rejected('summary-input-calibration-ratio', r => { r.forceCases[0].calibration.p99RatioBHToMutual = 2; }, /inconsistent calibration selection/);

  for (const document of ['Receipt', 'Manifest']) {
    const option = `raw${document}`;
    rejected(`duplicate-${document}-plain`, () => {}, /duplicate JSON object key/, {
      [option]: text => text.replace('"status":"passed"', '"status":"failed","status":"passed"')
    });
    rejected(`duplicate-${document}-escaped`, () => {}, /duplicate JSON object key/, {
      [option]: text => text.replace('"status":"passed"', String.raw`"st\u0061tus":"failed","status":"passed"`)
    });
    rejected(`duplicate-${document}-nested`, () => {}, /duplicate JSON object key/, {
      [option]: text => text.replace('"mutual-gravity.js":', '"mutual-gravity.js":"bad","mutual-gravity.js":')
    });
    rejected(`duplicate-${document}-nested-escaped`, () => {}, /duplicate JSON object key/, {
      [option]: text => text.replace('"mutual-gravity.js":', String.raw`"mutual-gravity.\u006as":"bad","mutual-gravity.js":`)
    });
  }
  for (const text of [String.raw`{"a":[{"x":1,"\u0078":2}]}`, String.raw`{"é":1,"\u00e9":2}`,
    String.raw`{"\uD834\uDD1E":1,"𝄞":2}`, '{"__proto__":{},"__proto__":{}}']) {
    assert.throws(() => parseArtifact(Buffer.from(text), 'parser-test'), /duplicate JSON object key/); passed++;
  }
  // Braces/commas inside strings and escaped backslashes are not structure or
  // Unicode escapes. Duplicate names in different objects remain legitimate.
  assert.deepEqual(parseArtifact(Buffer.from(String.raw`{"a\\u0062":"},:\"[","ab":1,"items":[{"x":1},{"x":2}]}`), 'parser-test'),
    { 'a\\u0062': '},:"[', ab: 1, items: [{ x: 1 }, { x: 2 }] }); passed++;
  assert.throws(() => parseArtifact(Buffer.from('{"a":1,}'), 'parser-test'), SyntaxError); passed++;
  assert.throws(() => parseArtifact(Buffer.from([0x7b, 0x22, 0xff, 0x22, 0x3a, 0x31, 0x7d]), 'parser-test'), TypeError); passed++;

  // Model a FIFO or changing file: the second read would supply a different
  // receipt. The verifier must hash and parse only the first returned Buffer.
  const originalRead = fs.readFileSync, receiptPath = path.join(valid, 'receipt.json');
  const pinnedBytes = originalRead(receiptPath);
  let receiptReads = 0;
  fs.readFileSync = function (file, ...args) {
    if (String(file) === receiptPath) {
      receiptReads++;
      return receiptReads === 1 ? pinnedBytes : Buffer.from('{"forgedSecondRead":true}');
    }
    return originalRead.call(this, file, ...args);
  };
  try { verify(valid, { quiet: true }); assert.equal(receiptReads, 1); passed++; }
  finally { fs.readFileSync = originalRead; }
  receiptReads = 0;
  fs.readFileSync = function (file, ...args) {
    if (String(file) === receiptPath) {
      receiptReads++;
      return receiptReads === 1 ? Buffer.from('{"forgedFirstRead":true}') : pinnedBytes;
    }
    return originalRead.call(this, file, ...args);
  };
  try {
    assert.throws(() => verify(valid, { quiet: true }), /receipt.json: hash mismatch/);
    assert.equal(receiptReads, 1); passed++;
  } finally { fs.readFileSync = originalRead; }

  for (const matrix of ['forceCases', 'smallNCases']) {
    rejected(`${matrix}-calibration-theta`, r => { r[matrix][0].calibration.theta = .975; }, /inconsistent calibration selection/);
    rejected(`${matrix}-calibration-grid-missing`, r => { r[matrix][0].calibration.candidates.pop(); }, /invalid calibration grid/);
    rejected(`${matrix}-calibration-grid-duplicate`, r => { r[matrix][0].calibration.candidates[1].theta = 0; }, /invalid calibration grid/);
    rejected(`${matrix}-calibration-grid-offpoint`, r => { r[matrix][0].calibration.candidates[1].theta = .024; }, /invalid calibration grid/);
    rejected(`${matrix}-calibration-grid-reordered`, r => { r[matrix][0].calibration.candidates.reverse(); }, /invalid calibration grid/);
    rejected(`${matrix}-calibration-candidate-error`, r => { r[matrix][0].calibration.candidates.find(c => c.theta === r[matrix][0].calibration.theta).error.rmsRelative *= .5; }, /inconsistent calibration selection|inconsistent force-error record/);
    rejected(`${matrix}-calibration-candidate-null`, r => { r[matrix][0].calibration.candidates[0].error.p50Relative = null; }, /invalid force-error record/);
    rejected(`${matrix}-calibration-candidate-tail`, r => { r[matrix][0].calibration.candidates[0].error.p95Relative = 1e9; }, /inconsistent force-error record/);
    rejected(`${matrix}-calibration-matched-error`, r => { r[matrix][0].barnesHutMatched.error.rmsAbsolute = .009; }, /matched error differs/);
  }
  for (const matrix of ['forceCases', 'smallNCases']) {
    rejected(`${matrix}-consistent-selected-absolute-error`, r => {
      const row = r[matrix][0];
      row.calibration.candidates.find(c => c.theta === row.calibration.theta).error.rmsAbsolute = 999;
      row.barnesHutMatched.error.rmsAbsolute = 999;
    }, /recomputed scientific record differs/);
    rejected(`${matrix}-unselected-absolute-error`, r => {
      const row = r[matrix][0];
      row.calibration.candidates.find(c => c.theta !== row.calibration.theta).error.rmsAbsolute = 999;
    }, /recomputed scientific record differs/);
    rejected(`${matrix}-mutual-absolute-error`, r => { r[matrix][0].mutual.error.rmsAbsolute = 999; }, /recomputed scientific record differs/);
    rejected(`${matrix}-mutual-torque`, r => { r[matrix][0].mutual.force.torqueAboutCOM = 999; }, /recomputed scientific record differs/);
    rejected(`${matrix}-mutual-stats`, r => { r[matrix][0].mutual.stats.direct += 1; }, /recomputed scientific record differs/);
    rejected(`${matrix}-matched-tree`, r => { r[matrix][0].barnesHutMatched.tree.nodes += 1; }, /recomputed scientific record differs/);
  }
  rejected('baseline-error-forged', r => { r.forceCases[0].barnesHutTheta05.error.rmsAbsolute = 999; }, /recomputed scientific record differs/);
  rejected('direct-force-forged', r => {
    r.forceCases[0].direct.force.normalizedNetForce = .01;
    r.summary = summarizeCases(r.forceCases, r.trajectoryCases);
  }, /recomputed scientific record differs/);
  const isolated = recomputeForceCase('disc', 8, 303), original = structuredClone(isolated);
  isolated.calibration.candidates[0].error.rmsAbsolute = 999; isolated.mutual.stats.direct = -1;
  assert.deepEqual(recomputeForceCase('disc', 8, 303), original); passed++;
  for (const matrix of ['forceCases', 'smallNCases']) {
    rejected(`${matrix}-timing-method`, r => { delete r[matrix][0].wholeSolveTimings.direct; }, /invalid timing methods/);
    rejected(`${matrix}-timing-count`, r => { r[matrix][0].wholeSolveTimings.mutual.samplesMs.pop(); }, /invalid timing samples/);
    rejected(`${matrix}-timing-negative`, r => { r[matrix][0].wholeSolveTimings.mutual.samplesMs[0] = -1; }, /invalid timing samples/);
    rejected(`${matrix}-timing-median`, r => { r[matrix][0].wholeSolveTimings.mutual.medianMs = 999; }, /timing summary differs/);
    rejected(`${matrix}-timing-p90`, r => { r[matrix][0].wholeSolveTimings.mutual.p90Ms = 999; }, /timing summary differs/);
    rejected(`${matrix}-memory-count`, r => { r[matrix][0].wholeSolveTimings.mutual.memoryObservations.pop(); }, /invalid memory observations/);
    rejected(`${matrix}-memory-delta`, r => { r[matrix][0].wholeSolveTimings.mutual.memoryObservations[0].heapUsedDelta = 999; }, /invalid memory observation/);
  }
  for (const [name, mutate] of [
    ['theta', row => { row.barnesHutTheta = .975; }],
    ['energy', row => { row.diagnostics.mutual.fine.maxRelativeEnergyDrift = 999; }],
    ['torque', row => { row.diagnostics.mutual.coarse.maxNormalizedAbsTorque = 999; }],
    ['final-state', row => { row.diagnostics.direct.fine.final.energy = 999; }],
    ['checkpoint', row => { row.diagnostics['barnes-hut'].coarse.checkpoints[0].netForceX = 999; }],
    ['control', row => { row.diagnostics.mutual.timestepControl.rmsPosition = 999; }],
    ['claimed-momentum', row => { row.diagnostics.mutual.fine.maxMomentumDrift = 0; }],
  ]) rejected(`trajectory-${name}-forged`, r => {
    mutate(r.trajectoryCases[0]); r.summary = summarizeCases(r.forceCases, r.trajectoryCases);
  }, /recomputed trajectory record differs/);
  const isolatedTrajectory = recomputeTrajectoryCase('disc', 128, 17), originalTrajectory = structuredClone(isolatedTrajectory);
  isolatedTrajectory.diagnostics.mutual.coarse.final.energy = 999;
  assert.deepEqual(recomputeTrajectoryCase('disc', 128, 17), originalTrajectory); passed++;
  rejected('test-commands-missing', r => { delete r.tests.commands; }, /exact successful command set/);
  rejected('test-commands-substituted', r => { r.tests.commands[0][1] = 'tests/other.mjs'; }, /exact successful command set/);
  rejected('test-outcomes-missing', r => { delete r.tests.outcomes; }, /outcomes disagree/);
  rejected('test-outcomes-forged-hash', r => { r.tests.outcomes[0].stdoutSHA256 = '0'.repeat(64); }, /outcomes disagree/);
  rejected('test-outcomes-forged-exit', r => { r.tests.outcomes[0].exitCode = 1; }, /outcomes disagree/);
  rejected('test-log-replaced', () => {}, /Unexpected token|Unexpected non-whitespace|JSON/, {
    rawLog: () => 'tests failed before completion'
  });
  rejected('test-log-no-runs', () => {}, /Malformed structured test log/, { mutateLog: log => { log.runs = []; } });
  rejected('test-log-command-mismatch', () => {}, /did not complete successfully/, { mutateLog: log => { log.runs[1].command[1] = 'tests/other.mjs'; } });
  rejected('test-log-execution-mismatch', () => {}, /did not complete successfully/, { mutateLog: log => { log.runs[0].execution = 'mutable-file'; } });
  rejected('test-log-nonzero-exit', () => {}, /did not complete successfully/, { mutateLog: log => { log.runs[0].exitCode = 1; } });
  rejected('test-log-signal', () => {}, /did not complete successfully/, { mutateLog: log => { log.runs[0].signal = 'SIGTERM'; } });
  rejected('test-log-spawn-error', () => {}, /did not complete successfully/, { mutateLog: log => { log.runs[0].error = 'spawn failed'; } });
  rejected('test-log-numerical-incomplete', () => {}, /lacks its completion marker/, { mutateLog: log => { log.runs[0].stdout = 'tests failed before completion'; } });
  rejected('test-log-verifier-incomplete', () => {}, /lacks its completion marker/, { mutateLog: log => { log.runs[1].stdout = 'tests failed before completion'; } });
  rejected('test-log-numerical-payload-incomplete', () => {}, /Numerical test output is incomplete/, {
    mutateLog: log => { log.runs[0].stdout = 'Mutual-gravity CPU/f64 checks passed\n{}'; }
  });
  rejected('test-log-stderr-mismatch', () => {}, /outcomes disagree/, { mutateLog: log => { log.runs[0].stderr = 'unexpected diagnostic'; } });

  for (const wrongCount of [0, 1, 120, 121, VERIFIER_CHECK_COUNT + 1, VERIFIER_CHECK_COUNT + 10]) {
    const stdout = `Mutual-gravity evidence verifier checks passed (${wrongCount} checks; deterministic fixtures)\n`;
    rejected(`test-log-wrong-check-count-${wrongCount}`, r => {
      r.tests.outcomes[1].stdoutSHA256 = sha256(stdout);
    }, /lacks its completion marker/, { mutateLog: log => { log.runs[1].stdout = stdout; } });
  }
  // Subprocess isolation keeps deliberate require-cache poisoning and changed
  // disk-read simulation away from the verifier test runner itself.
  const bindingProbe = spawnSync(process.execPath, ['--input-type=module', '-e', `
    import assert from 'node:assert/strict';
    import fs from 'node:fs';
    import path from 'node:path';
    import { fileURLToPath } from 'node:url';
    import { createRequire } from 'node:module';
    import { createHash } from 'node:crypto';
    const require = createRequire(import.meta.url);
    const names = ${JSON.stringify(Object.keys(sources))};
    const originals = Object.fromEntries(names.map(name => [name,fs.readFileSync(name)]));
    for(const name of ['barnes-hut.js','mutual-gravity.js']) { require('./'+name); require.cache[require.resolve('./'+name)].exports = { poisoned: true }; }
    const staleHelper = await import('./scripts/lib/mutual-gravity-evidence.mjs');
    for (const key of ['GalaxyBarnesHut','GalaxyMutualGravity']) Object.defineProperty(globalThis,key, {
      configurable: true, get() { throw new Error('real global getter must not run'); },
      set() { throw new Error('real global setter must not run'); }
    });
    const read = fs.readFileSync, counts = Object.fromEntries(names.map(name => [name,0]));
    let changed = null;
    fs.readFileSync = function(file,...args) {
      const full = file instanceof URL ? fileURLToPath(file) : String(file);
      const name = path.relative(process.cwd(), full);
      if (names.includes(name)) {
        if (file instanceof URL) counts[name]++;
        if (name === changed) return Buffer.from('throw new Error("changed source");');
      }
      return read.call(this,file,...args);
    };
    const harness = await import('./scripts/bench-mutual-gravity.mjs');
    const helper = harness.EVIDENCE;
    assert.notEqual(helper.BH, staleHelper.BH, 'must not reuse a previously evaluated ESM helper');
    for(const name of names) {
      if (name === 'scripts/bench-mutual-gravity.mjs') assert.ok(counts[name] >= 1 && counts[name] <= 2, 'retained snapshot plus optional Node entry-loader read');
      else assert.equal(counts[name], 1, name + ': one retained snapshot before execution');
      assert.equal(helper.LOADED_SOURCE_SHA256[name],createHash('sha256').update(originals[name]).digest('hex'));
    }
    assert.equal(helper.BH.poisoned,undefined); assert.equal(helper.MUTUAL.poisoned,undefined);
    const bodies=[{x:0,y:0,mass:1},{x:1,y:0,mass:1}];
    assert.ok(helper.MUTUAL.accelerations(bodies).values[0].ax>0);
    assert.ok(helper.BH.accelerations(bodies).values[0].ax>0);
    for (const name of names) {
      changed = name;
      assert.throws(()=>harness.sourceHashes(),/Loaded executable source differs from capture source/);
    }
    fs.readFileSync=read;
  `], { cwd: root, encoding: 'utf8', maxBuffer: 8 * 1024 * 1024 });
  assert.equal(bindingProbe.status, 0, `Source binding/cache regression: ${bindingProbe.stdout}\n${bindingProbe.stderr}`); passed++;

  const exactByteProbe = spawnSync(process.execPath, ['--input-type=module', '-e', `
    import assert from 'node:assert/strict';
    import fs from 'node:fs';
    import { createHash } from 'node:crypto';
    const read = fs.readFileSync;
    let retainedHelper;
    fs.readFileSync = function(file,...args) {
      const bytes = read.call(this,file,...args);
      if (String(file).endsWith('/scripts/lib/mutual-gravity-evidence.mjs')) {
        retainedHelper = Buffer.concat([bytes, Buffer.from('\\nexport const RETAINED_HELPER_MARKER = "exact bytes";\\n')]);
        return retainedHelper;
      }
      if (String(file).endsWith('/scripts/bench-mutual-gravity.mjs'))
        return Buffer.concat([bytes, Buffer.from('\\nif(import.meta.url.startsWith("data:")) globalThis.__retainedHarnessMarker = "exact bytes";\\n')]);
      return bytes;
    };
    const harness = await import('./scripts/bench-mutual-gravity.mjs');
    assert.equal(harness.EVIDENCE.RETAINED_HELPER_MARKER, 'exact bytes');
    assert.equal(globalThis.__retainedHarnessMarker, 'exact bytes');
    assert.equal(harness.EVIDENCE.LOADED_SOURCE_SHA256['scripts/lib/mutual-gravity-evidence.mjs'],
      createHash('sha256').update(retainedHelper).digest('hex'));
    fs.readFileSync = read;
    assert.throws(() => harness.sourceHashes(), /Loaded executable source differs/);
  `], { cwd: root, encoding: 'utf8', maxBuffer: 8 * 1024 * 1024 });
  assert.equal(exactByteProbe.status, 0, `Exact retained-byte execution: ${exactByteProbe.stdout}\n${exactByteProbe.stderr}`); passed++;

  const before = fs.readFileSync(path.join(valid, 'receipt.json'));
  const duplicate = spawnSync(process.execPath, ['scripts/bench-mutual-gravity.mjs', '--output', valid], { cwd: root, encoding: 'utf8' });
  assert.notEqual(duplicate.status, 0);
  assert.match(duplicate.stderr, /Refusing to overwrite existing evidence/);
  assert.deepEqual(fs.readFileSync(path.join(valid, 'receipt.json')), before); passed++;
  assert.equal(passed, VERIFIER_CHECK_COUNT, 'Update the source-bound completion count after changing the suite');
  process.stdout.write(VERIFIER_COMPLETION);
} finally {
  fs.rmSync(temp, { recursive: true, force: true });
}
