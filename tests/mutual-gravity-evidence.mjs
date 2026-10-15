// SPDX-License-Identifier: Apache-2.0
// Verifier contract tests use synthetic metadata, not newly measured evidence.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { verify, sourceHashes, parseArtifact } from '../scripts/bench-mutual-gravity.mjs';
import { ROOT, OPTIONS, BUDGET, fixture, sha256 } from '../scripts/lib/mutual-gravity-evidence.mjs';

const root = fileURLToPath(ROOT), temp = fs.mkdtempSync(path.join(os.tmpdir(), 'galaxy-mutual-verifier-'));
const sources = sourceHashes();
function identity(preset, count, seed) {
  return { id: `${preset}-${count}-${seed}`, preset, count, seed,
    initialStateSHA256: sha256(JSON.stringify(fixture(preset, count, seed))) };
}
const forceCases = [], smallNCases = [], trajectoryCases = [];
function errors(rms = .001) {
  return { rmsRelative: rms, p50Relative: .0001, p90Relative: .001, p95Relative: .002,
    p99Relative: .005, maxRelative: .01, rmsAbsolute: .001, globalRelativeL2: .001 };
}
function forceRow(preset, count, seed) {
  const candidates = Array.from({ length: 41 }, (_, i) => ({ theta: i / 40, error: errors(i === 18 ? .001 : .002) }));
  return { ...identity(preset, count, seed), mutual: { error: errors(),
    force: { normalizedNetForce: 0 }, stats: { approximated: 1, direct: 1 } },
    direct: { force: { normalizedNetForce: 0 } },
    barnesHutTheta05: { force: { normalizedNetForce: count === 2048 ? .001 : .0005 } },
    barnesHutMatched: { error: errors() },
    calibration: { theta: .45, targetDirectEquivalent: false, rmsRatioBHToMutual: 1, p99RatioBHToMutual: 1,
      matchedRMS: true, tailMatchedWithin25Percent: true, candidates } };
}
for (const count of [128, 512]) for (const seed of [17, 303, 991]) for (const preset of ['disc', 'collision'])
  forceCases.push(forceRow(preset, count, seed));
for (const preset of ['disc', 'collision']) forceCases.push(forceRow(preset, 2048, 303));
for (const count of [8, 32]) for (const preset of ['disc', 'collision']) smallNCases.push(forceRow(preset, count, 303));
for (const seed of [17, 303, 991]) for (const preset of ['disc', 'collision']) {
  const diagnostics = {};
  for (const method of ['direct', 'mutual', 'barnes-hut']) {
    diagnostics[method] = {};
    for (const [resolution, dt, steps] of [['coarse', .001, 64], ['fine', .0005, 128]])
      diagnostics[method][resolution] = { dt, steps, forceSolves: steps + 1, duration: .064,
        maxNormalizedNetForce: 0, maxMomentumDrift: 0, maxCOMBallisticDrift: 0 };
  }
  trajectoryCases.push({ ...identity(preset, 128, seed), diagnostics });
}
const testCommands = [['node', 'tests/mutual-gravity.mjs'], ['node', 'tests/mutual-gravity-evidence.mjs']];
const testLog = { schema: 'galaxy.mutual-gravity-test-log.v1', runs: [
  { command: testCommands[0], exitCode: 0, signal: null, error: null, stderr: '',
    stdout: 'Mutual-gravity CPU/f64 checks passed\n' + JSON.stringify({ cases: Array.from({ length: 27 }, () => ({ synthetic: true })),
      pairedPotential: { synthetic: true }, trajectory: { synthetic: true } }) + '\n' },
  { command: testCommands[1], exitCode: 0, signal: null, error: null, stderr: '',
    stdout: 'Mutual-gravity evidence verifier checks passed (1 checks; synthetic fixtures only)\n' }
] };
function outcome(run) {
  return { command: run.command, exitCode: run.exitCode, signal: run.signal, error: run.error,
    stdoutSHA256: sha256(run.stdout), stderrSHA256: sha256(run.stderr) };
}
const base = { schema: 'galaxy.mutual-gravity-receipt.v1', status: 'passed', sourceSHA256: sources,
  configuration: OPTIONS, budgets: BUDGET, forceCases, smallNCases, trajectoryCases,
  tests: { exitCode: 0, log: 'tests.log', commands: testCommands, outcomes: testLog.runs.map(outcome) }, syntheticVerifierTestFixture: true,
  summary: { forceCases: 14, trajectoryCases: 6, maxMutualRMSRelative: .001, maxMutualMaxRelative: .01,
    maxMutualNormalizedNetForce: 0, maxDirectNormalizedNetForce: 0, baselineBHNormalizedNetForceRange: [.0005, .0005],
    matchedRMSCases: 14, matchedRMSAndP99Cases: 14, maxTrajectoryMomentumDrift: 0, maxTrajectoryCOMBallisticDrift: 0 } };
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
  rejected('summary-input-calibration-flag', r => { r.forceCases[0].calibration.matchedRMS = false; }, /inconsistent calibration selection/);
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
    rejected(`${matrix}-calibration-candidate-error`, r => { r[matrix][0].calibration.candidates[18].error.rmsRelative = .009; }, /inconsistent calibration selection/);
    rejected(`${matrix}-calibration-candidate-null`, r => { r[matrix][0].calibration.candidates[0].error.p50Relative = null; }, /invalid force-error record/);
    rejected(`${matrix}-calibration-candidate-tail`, r => { r[matrix][0].calibration.candidates[0].error.p95Relative = .2; }, /inconsistent force-error record/);
    rejected(`${matrix}-calibration-matched-error`, r => { r[matrix][0].barnesHutMatched.error.rmsAbsolute = .009; }, /matched error differs/);
  }
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
  rejected('test-log-nonzero-exit', () => {}, /did not complete successfully/, { mutateLog: log => { log.runs[0].exitCode = 1; } });
  rejected('test-log-signal', () => {}, /did not complete successfully/, { mutateLog: log => { log.runs[0].signal = 'SIGTERM'; } });
  rejected('test-log-spawn-error', () => {}, /did not complete successfully/, { mutateLog: log => { log.runs[0].error = 'spawn failed'; } });
  rejected('test-log-numerical-incomplete', () => {}, /lacks its completion marker/, { mutateLog: log => { log.runs[0].stdout = 'tests failed before completion'; } });
  rejected('test-log-verifier-incomplete', () => {}, /lacks its completion marker/, { mutateLog: log => { log.runs[1].stdout = 'tests failed before completion'; } });
  rejected('test-log-numerical-payload-incomplete', () => {}, /Numerical test output is incomplete/, {
    mutateLog: log => { log.runs[0].stdout = 'Mutual-gravity CPU/f64 checks passed\n{}'; }
  });
  rejected('test-log-stderr-mismatch', () => {}, /outcomes disagree/, { mutateLog: log => { log.runs[0].stderr = 'unexpected diagnostic'; } });

  // Subprocess isolation keeps deliberate require-cache poisoning and changed
  // disk-read simulation away from the verifier test runner itself.
  const bindingProbe = spawnSync(process.execPath, ['--input-type=module', '-e', `
    import assert from 'node:assert/strict';
    import fs from 'node:fs';
    import path from 'node:path';
    import { fileURLToPath } from 'node:url';
    import { createRequire } from 'node:module';
    import { createHash } from 'node:crypto';
    const require = createRequire(import.meta.url), names = ['barnes-hut.js','mutual-gravity.js'];
    const originals = Object.fromEntries(names.map(name => [name,fs.readFileSync(name)]));
    for(const name of names) { require('./'+name); require.cache[require.resolve('./'+name)].exports = { poisoned: true }; }
    for (const key of ['GalaxyBarnesHut','GalaxyMutualGravity']) Object.defineProperty(globalThis,key, {
      configurable: true, get() { throw new Error('real global getter must not run'); },
      set() { throw new Error('real global setter must not run'); }
    });
    const read = fs.readFileSync, counts = Object.fromEntries(names.map(name => [name,0]));
    fs.readFileSync = function(file,...args) {
      const full = file instanceof URL ? fileURLToPath(file) : String(file);
      const name = path.basename(full);
      if (names.includes(name)) {
        counts[name]++;
        return counts[name] === 1 ? originals[name] : Buffer.from('throw new Error("changed source");');
      }
      return read.call(this,file,...args);
    };
    const helper = await import('./scripts/lib/mutual-gravity-evidence.mjs');
    for(const name of names) {
      assert.equal(counts[name],1,'solver source must be read exactly once before execution');
      assert.equal(helper.LOADED_SOURCE_SHA256[name],createHash('sha256').update(originals[name]).digest('hex'));
    }
    assert.equal(helper.BH.poisoned,undefined); assert.equal(helper.MUTUAL.poisoned,undefined);
    const bodies=[{x:0,y:0,mass:1},{x:1,y:0,mass:1}];
    assert.ok(helper.MUTUAL.accelerations(bodies).values[0].ax>0);
    assert.ok(helper.BH.accelerations(bodies).values[0].ax>0);
    const harness = await import('./scripts/bench-mutual-gravity.mjs');
    assert.throws(()=>harness.sourceHashes(),/Loaded solver source differs from capture source/);
    fs.readFileSync=read;
  `], { cwd: root, encoding: 'utf8' });
  assert.equal(bindingProbe.status, 0, `Source binding/cache regression: ${bindingProbe.stdout}\n${bindingProbe.stderr}`); passed++;

  const before = fs.readFileSync(path.join(valid, 'receipt.json'));
  const duplicate = spawnSync(process.execPath, ['scripts/bench-mutual-gravity.mjs', '--output', valid], { cwd: root, encoding: 'utf8' });
  assert.notEqual(duplicate.status, 0);
  assert.match(duplicate.stderr, /Refusing to overwrite existing evidence/);
  assert.deepEqual(fs.readFileSync(path.join(valid, 'receipt.json')), before); passed++;
  console.log(`Mutual-gravity evidence verifier checks passed (${passed} checks; synthetic fixtures only)`);
} finally {
  fs.rmSync(temp, { recursive: true, force: true });
}
