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
function forceRow(preset, count, seed) {
  return { ...identity(preset, count, seed), mutual: { error: { rmsRelative: .001, maxRelative: .01, p99Relative: .005 },
    force: { normalizedNetForce: 0 }, stats: { approximated: 1, direct: 1 } },
    direct: { force: { normalizedNetForce: 0 } },
    barnesHutTheta05: { force: { normalizedNetForce: count === 2048 ? .001 : .0005 } },
    barnesHutMatched: { error: { rmsRelative: .001, p99Relative: .005 } },
    calibration: { targetDirectEquivalent: false, rmsRatioBHToMutual: 1, p99RatioBHToMutual: 1,
      matchedRMS: true, tailMatchedWithin25Percent: true } };

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
const base = { schema: 'galaxy.mutual-gravity-receipt.v1', status: 'passed', sourceSHA256: sources,
  configuration: OPTIONS, budgets: BUDGET, forceCases, smallNCases, trajectoryCases,
  tests: { exitCode: 0 }, syntheticVerifierTestFixture: true,
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
  fs.writeFileSync(path.join(directory, 'tests.log'), 'Synthetic verifier contract fixture only. No scientific measurement is claimed.\n');
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
  rejected('tests-failed', r => { r.tests.exitCode = 1; }, /Tests did not pass/);
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
  rejected('summary-input-calibration-flag', r => { r.forceCases[0].calibration.matchedRMS = false; }, /inconsistent calibration summary inputs/);
  rejected('summary-input-calibration-ratio', r => { r.forceCases[0].calibration.p99RatioBHToMutual = 2; }, /inconsistent calibration summary inputs/);

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

  const before = fs.readFileSync(path.join(valid, 'receipt.json'));
  const duplicate = spawnSync(process.execPath, ['scripts/bench-mutual-gravity.mjs', '--output', valid], { cwd: root, encoding: 'utf8' });
  assert.notEqual(duplicate.status, 0);
  assert.match(duplicate.stderr, /Refusing to overwrite existing evidence/);
  assert.deepEqual(fs.readFileSync(path.join(valid, 'receipt.json')), before); passed++;
  console.log(`Mutual-gravity evidence verifier checks passed (${passed} checks; synthetic fixtures only)`);
} finally {
  fs.rmSync(temp, { recursive: true, force: true });
}
