#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Bounded, source-bound CPU/f64 evidence. Raw timings are observations, not speed claims.
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import { performance } from 'node:perf_hooks';
import { spawnSync } from 'node:child_process';
import { isDeepStrictEqual, TextDecoder } from 'node:util';
// The file entry is a bootstrap only. Snapshot EVERY project executable before
// evaluating any implementation, then replay this harness and its helper from
// those exact UTF-8 bytes as native-realm ESM data URLs. The Node runtime and
// this small bootstrap are the trust root; no previously imported file module
// or CommonJS cache can supply benchmark functions.
const SOURCE_FILES = ['barnes-hut.js', 'mutual-gravity.js', 'tests/mutual-gravity.mjs',
  'scripts/bench-mutual-gravity.mjs', 'scripts/lib/mutual-gravity-evidence.mjs',
  'tests/mutual-gravity-evidence.mjs'];
function moduleURL(source, name) { return 'data:text/javascript;base64,' + Buffer.from(source + '\n//# sourceURL=retained-' + name + '\n').toString('base64'); }
function utf8(bytes) { return new TextDecoder('utf-8', { fatal: true }).decode(bytes); }
const replay = import.meta.url.startsWith('file:') ? await (async () => {
  const rootURL = new URL('../', import.meta.url);
  const bytes = Object.fromEntries(SOURCE_FILES.map(name => [name, fs.readFileSync(new URL(name, rootURL))]));
  const base64 = Object.fromEntries(Object.entries(bytes).map(([name, value]) => [name, value.toString('base64')]));
  const prefix = `const __EVIDENCE_ROOT = ${JSON.stringify(rootURL.href)};\nconst __EVIDENCE_SOURCE_BASE64 = ${JSON.stringify(base64)};\n`;
  const helperURL = moduleURL(prefix + utf8(bytes['scripts/lib/mutual-gravity-evidence.mjs']), 'mutual-gravity-evidence.mjs');
  const harnessURL = moduleURL(prefix + `const __EVIDENCE_HELPER_URL = ${JSON.stringify(helperURL)};\n` +
    utf8(bytes['scripts/bench-mutual-gravity.mjs']).replace(/^#![^\n]*\n/, ''), 'bench-mutual-gravity.mjs');
  return import(harnessURL);
})() : null;
export const EVIDENCE = replay ? replay.EVIDENCE : await import(__EVIDENCE_HELPER_URL);
const { ROOT, BH, MUTUAL, OPTIONS, BUDGET, sha256, errorMetrics, forceDiagnostics,
  fixture, trajectory, stateDifference, quantile, LOADED_SOURCE_SHA256, VERIFIER_COMPLETION } = EVIDENCE;
const root = fileURLToPath(ROOT);
export function sourceHashes() {
  if (replay) return replay.sourceHashes();
  const hashes = Object.fromEntries(SOURCE_FILES.map(p => [p, sha256(fs.readFileSync(path.join(root, p)))]));
  for (const [name, loadedHash] of Object.entries(LOADED_SOURCE_SHA256))
    assert(hashes[name] === loadedHash, `Loaded executable source differs from capture source: ${name}`);
  return hashes;
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
// JSON.parse alone silently keeps the last duplicate key. Scan the validated
// JSON token stream and compare decoded keys in each object before trusting it.
export function parseArtifact(bytes, label) {
  if (replay) return replay.parseArtifact(bytes, label);
  const text = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  const result = JSON.parse(text);
  let at = 0;
  function whitespace() { while (at < text.length && /\s/.test(text[at])) at++; }
  function string() {
    const start = at++;
    while (text[at] !== '"') { if (text[at] === '\\') at++; at++; }
    at++;
    return JSON.parse(text.slice(start, at));
  }
  function value() {
    whitespace();
    if (text[at] === '{') {
      at++; whitespace(); const keys = new Set();
      if (text[at] === '}') { at++; return; }
      while (true) {
        whitespace(); const key = string();
        assert(!keys.has(key), `${label}: duplicate JSON object key ${JSON.stringify(key)}`);
        keys.add(key); whitespace(); at++; value(); whitespace();
        if (text[at++] === '}') return;
      }
    }
    if (text[at] === '[') {
      at++; whitespace();
      if (text[at] === ']') { at++; return; }
      while (true) { value(); whitespace(); if (text[at++] === ']') return; }
    }
    if (text[at] === '"') { string(); return; }
    while (at < text.length && !/[\s,\]}]/.test(text[at])) at++;
  }
  value(); whitespace();
  assert(at === text.length, `${label}: unconsumed JSON input`);
  strictJSON(result);
  return result;
}
function writeNew(output, name, data) { fs.writeFileSync(path.join(output, name), data, { flag: 'wx' }); }
function nonnegativeFinite(value) { return Number.isFinite(value) && value >= 0; }
function qualify(error, force, name) {
  assert([error.rmsRelative, error.maxRelative, force.normalizedNetForce].every(nonnegativeFinite), `${name}: invalid qualification metric`);
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
  return selectCalibration(candidates, target);
}
function selectCalibration(candidates, target) {
  // Choose closest achieved RMS among candidates that meet the inherited
  // RMS/worst gates. Matching is declared only within an explicit 25% band.
  const qualified = candidates.filter(c => c.error.rmsRelative < BUDGET.rmsRelative && c.error.maxRelative < BUDGET.maxRelative);
  qualified.sort((a, b) => Math.abs(Math.log(Math.max(a.error.rmsRelative, 1e-30) / Math.max(target.rmsRelative, 1e-30))) -
    Math.abs(Math.log(Math.max(b.error.rmsRelative, 1e-30) / Math.max(target.rmsRelative, 1e-30))));
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
const scientificCache = new Map();
function freezeDeep(value) {
  if (value && typeof value === 'object') { for (const item of Object.values(value)) freezeDeep(item); Object.freeze(value); }
  return value;
}
export function recomputeForceCase(preset, count, seed) {
  if (replay) return replay.recomputeForceCase(preset, count, seed);
  const key = JSON.stringify([preset, count, seed]);
  if (!scientificCache.has(key)) {
    const bodies = fixture(preset, count, seed), exact = BH.directAccelerations(bodies, OPTIONS);
    const mutual = describe(MUTUAL.accelerations(bodies, OPTIONS), bodies, exact);
    const calibration = calibrateBH(bodies, exact, mutual.error);
    scientificCache.set(key, freezeDeep({ mutual, direct: { force: forceDiagnostics(bodies, exact) }, calibration,
      barnesHutTheta05: describe(BH.accelerations(bodies, { ...OPTIONS, theta: .5 }), bodies, exact),
      barnesHutMatched: describe(BH.accelerations(bodies, { ...OPTIONS, theta: calibration.theta }), bodies, exact) }));
  }
  return structuredClone(scientificCache.get(key));
}
function validateScientificCase(row, smallN = false) {
  const expected = recomputeForceCase(row.preset, row.count, row.seed);
  for (const key of smallN ? ['mutual', 'calibration', 'barnesHutMatched'] : Object.keys(expected))
    assert(isDeepStrictEqual(row[key], expected[key]), `${row.id}: recomputed scientific record differs: ${key}`);
}
const trajectoryCache = new Map();
export function recomputeTrajectoryCase(preset, count, seed) {
  if (replay) return replay.recomputeTrajectoryCase(preset, count, seed);
  const key = JSON.stringify([preset, count, seed]);
  if (!trajectoryCache.has(key)) {
    const bodies = fixture(preset, count, seed);
    const barnesHutTheta = recomputeForceCase(preset, count, seed).calibration.theta;
    const runs = {}, diagnostics = {};
    for (const method of ['direct', 'mutual', 'barnes-hut']) {
      const options = { ...OPTIONS, theta: method === 'barnes-hut' ? barnesHutTheta : OPTIONS.theta };
      runs[method] = { coarse: trajectory(bodies, method, options, .001, 64), fine: trajectory(bodies, method, options, .0005, 128) };
    }
    for (const method of ['direct', 'mutual', 'barnes-hut']) diagnostics[method] = {
      coarse: runs[method].coarse.diagnostic, fine: runs[method].fine.diagnostic,
      timestepControl: stateDifference(runs[method].coarse.bodies, runs[method].fine.bodies),
      versusDirectSameDt: stateDifference(runs[method].coarse.bodies, runs.direct.coarse.bodies),
      versusDirectHalfDt: stateDifference(runs[method].fine.bodies, runs.direct.fine.bodies)
    };
    trajectoryCache.set(key, freezeDeep({ barnesHutTheta, diagnostics }));
  }
  return structuredClone(trajectoryCache.get(key));
}
function validateTimings(row) {
  const timings = row.wholeSolveTimings;
  assert(timings && isDeepStrictEqual(Object.keys(timings).sort(), ['barnesHutMatched', 'direct', 'mutual']), `${row.id}: invalid timing methods`);
  for (const timing of Object.values(timings)) {
    assert(timing.warmups === 2 && timing.repeats === 9 && Array.isArray(timing.samplesMs) && timing.samplesMs.length === 9 &&
      timing.samplesMs.every(nonnegativeFinite), `${row.id}: invalid timing samples`);
    const sorted = [...timing.samplesMs].sort((a, b) => a - b);
    assert(timing.medianMs === quantile(sorted, .5) && timing.p90Ms === quantile(sorted, .9), `${row.id}: timing summary differs from raw samples`);
    assert(Array.isArray(timing.memoryObservations) && timing.memoryObservations.length === 9, `${row.id}: invalid memory observations`);
    for (const observation of timing.memoryObservations) {
      assert(isDeepStrictEqual(Object.keys(observation).sort(), ['arrayBuffersDelta', 'heapUsedAfter', 'heapUsedBefore', 'heapUsedDelta', 'rssDelta']) &&
        Object.values(observation).every(Number.isSafeInteger) && observation.heapUsedBefore >= 0 && observation.heapUsedAfter >= 0 &&
        observation.heapUsedDelta === observation.heapUsedAfter - observation.heapUsedBefore, `${row.id}: invalid memory observation`);
    }
  }
}
function expectedWorkloads() {
  const forceCases = [], smallNCases = [], trajectoryCases = [];
  const tuple = (preset, count, seed) => ({ id: `${preset}-${count}-${seed}`, preset, count, seed });
  for (const count of [128, 512]) for (const seed of [17, 303, 991]) for (const preset of ['disc', 'collision'])
    forceCases.push(tuple(preset, count, seed));
  for (const preset of ['disc', 'collision']) forceCases.push(tuple(preset, 2048, 303));
  for (const count of [8, 32]) for (const preset of ['disc', 'collision']) smallNCases.push(tuple(preset, count, 303));
  for (const seed of [17, 303, 991]) for (const preset of ['disc', 'collision']) trajectoryCases.push(tuple(preset, 128, seed));
  return { forceCases, smallNCases, trajectoryCases };
}
function verifyWorkloadMatrix(rows, expected, name) {
  assert(Array.isArray(rows) && rows.length === expected.length, `${name}: incomplete workload matrix`);
  const tupleKey = row => JSON.stringify([row.id, row.preset, row.count, row.seed]);
  const wanted = new Set(expected.map(tupleKey)), seen = new Set();
  for (const row of rows) {
    assert(row && typeof row === 'object', `${name}: malformed workload identity`);
    const key = tupleKey(row);
    assert(wanted.has(key), `${name}: unexpected workload identity ${key}`);
    assert(!seen.has(key), `${name}: duplicate workload identity ${key}`);
    seen.add(key);
    const expectedHash = sha256(JSON.stringify(fixture(row.preset, row.count, row.seed)));
    assert(row.initialStateSHA256 === expectedHash, `${name}: initial-state hash mismatch for ${row.id}`);
  }
  assert(seen.size === wanted.size, `${name}: missing workload identity`);
}
export function summarizeCases(forceCases, trajectoryCases) {
  if (replay) return replay.summarizeCases(forceCases, trajectoryCases);
  const baseline = forceCases.filter(row => row.count === 128 || row.count === 512);
  return {
    forceCases: forceCases.length, trajectoryCases: trajectoryCases.length,
    maxMutualRMSRelative: Math.max(...forceCases.map(row => row.mutual.error.rmsRelative)),
    maxMutualMaxRelative: Math.max(...forceCases.map(row => row.mutual.error.maxRelative)),
    maxMutualNormalizedNetForce: Math.max(...forceCases.map(row => row.mutual.force.normalizedNetForce)),
    maxDirectNormalizedNetForce: Math.max(...forceCases.map(row => row.direct.force.normalizedNetForce)),
    baselineBHNormalizedNetForceRange: [Math.min(...baseline.map(row => row.barnesHutTheta05.force.normalizedNetForce)),
      Math.max(...baseline.map(row => row.barnesHutTheta05.force.normalizedNetForce))],
    matchedRMSCases: forceCases.filter(row => row.calibration.matchedRMS).length,
    matchedRMSAndP99Cases: forceCases.filter(row => row.calibration.matchedRMS && row.calibration.tailMatchedWithin25Percent).length,
    maxTrajectoryMomentumDrift: Math.max(...trajectoryCases.flatMap(row => ['coarse', 'fine'].map(r => row.diagnostics.mutual[r].maxMomentumDrift))),
    maxTrajectoryCOMBallisticDrift: Math.max(...trajectoryCases.flatMap(row => ['coarse', 'fine'].map(r => row.diagnostics.mutual[r].maxCOMBallisticDrift)))
  };
}
const ERROR_FIELDS = ['rmsRelative', 'p50Relative', 'p90Relative', 'p95Relative', 'p99Relative',
  'maxRelative', 'rmsAbsolute', 'globalRelativeL2'];
function validateErrors(error, label) {
  assert(error && isDeepStrictEqual(Object.keys(error).sort(), [...ERROR_FIELDS].sort()) &&
    ERROR_FIELDS.every(key => nonnegativeFinite(error[key])), `${label}: invalid force-error record`);
  assert(error.p50Relative <= error.p90Relative && error.p90Relative <= error.p95Relative &&
    error.p95Relative <= error.p99Relative && error.p99Relative <= error.maxRelative &&
    error.rmsRelative <= error.maxRelative, `${label}: inconsistent force-error record`);
}
function validateCalibration(row) {
  validateErrors(row.mutual.error, row.id);
  const candidates = row.calibration?.candidates;
  assert(Array.isArray(candidates) && candidates.length === 41, `${row.id}: invalid calibration grid`);
  for (let i = 0; i < candidates.length; i++) {
    const candidate = candidates[i];
    assert(candidate && isDeepStrictEqual(Object.keys(candidate).sort(), ['error', 'theta']) && candidate.theta === i / 40,
      `${row.id}: invalid calibration grid`);
    validateErrors(candidate.error, `${row.id} candidate ${i}`);
  }
  const selected = selectCalibration(candidates, row.mutual.error);
  assert(isDeepStrictEqual(row.calibration, selected), `${row.id}: inconsistent calibration selection`);
  const selectedError = candidates.find(candidate => candidate.theta === selected.theta).error;
  assert(isDeepStrictEqual(row.barnesHutMatched.error, selectedError), `${row.id}: matched error differs from selected calibration candidate`);
}
function validateSummaryInputs(row) {
  assert([row.direct.force.normalizedNetForce, row.barnesHutTheta05.force.normalizedNetForce].every(nonnegativeFinite),
    `${row.id}: invalid summary input`);
}
const TEST_COMMANDS = Object.freeze([['node', 'tests/mutual-gravity.mjs'], ['node', 'tests/mutual-gravity-evidence.mjs']].map(Object.freeze));
function commandOutcome(run) {
  return { command: run.command, execution: run.execution, exitCode: run.exitCode, signal: run.signal, error: run.error,
    stdoutSHA256: sha256(run.stdout), stderrSHA256: sha256(run.stderr) };
}
function validateTestEvidence(tests, log) {
  assert(tests && tests.exitCode === 0 && tests.log === 'tests.log' && isDeepStrictEqual(tests.commands, TEST_COMMANDS),
    'Tests require the exact successful command set');
  assert(log.schema === 'galaxy.mutual-gravity-test-log.v1' && Array.isArray(log.runs) && log.runs.length === TEST_COMMANDS.length,
    'Malformed structured test log');
  for (let i = 0; i < log.runs.length; i++) {
    const run = log.runs[i];
    assert(isDeepStrictEqual(run.command, TEST_COMMANDS[i]) && run.execution === 'retained-source-esm-stdin-v1' && run.exitCode === 0 && run.signal === null && run.error === null &&
      typeof run.stdout === 'string' && typeof run.stderr === 'string', 'Test command did not complete successfully');
    if (i === 0) {
      const marker = 'Mutual-gravity CPU/f64 checks passed\n';
      assert(run.stdout.startsWith(marker), 'Numerical test log lacks its completion marker');
      const summary = parseArtifact(Buffer.from(run.stdout.slice(marker.length)), 'numerical test output');
      assert(summary && Array.isArray(summary.cases) && summary.cases.length >= 27 && summary.pairedPotential && summary.trajectory,
        'Numerical test output is incomplete');
    } else assert(run.stdout === VERIFIER_COMPLETION,
      'Verifier test log lacks its completion marker');
  }
  assert(isDeepStrictEqual(tests.outcomes, log.runs.map(commandOutcome)), 'Test outcomes disagree with structured log');
}
export function verify(output, { quiet = false } = {}) {
  if (replay) return replay.verify(output, { quiet });
  const manifestBytes = fs.readFileSync(path.join(output, 'manifest.json'));
  const manifest = parseArtifact(manifestBytes, 'manifest.json');
  assert(manifest.schema === 'galaxy.mutual-gravity-manifest.v1', 'Wrong manifest schema');
  assert(manifest.status === 'passed', 'Capture was not successful');
  sameHashes(manifest.sourceSHA256, sourceHashes());
  assert(Object.keys(manifest.files).sort().join(',') === 'receipt.json,tests.log', 'Unexpected receipt file set');
  const receiptBytes = fs.readFileSync(path.join(output, 'receipt.json'));
  assert(sha256(receiptBytes) === manifest.files['receipt.json'], 'receipt.json: hash mismatch');
  const testLogBytes = fs.readFileSync(path.join(output, 'tests.log'));
  assert(sha256(testLogBytes) === manifest.files['tests.log'], 'tests.log: hash mismatch');
  const testLog = parseArtifact(testLogBytes, 'tests.log');
  const receipt = parseArtifact(receiptBytes, 'receipt.json');
  assert(receipt.schema === 'galaxy.mutual-gravity-receipt.v1', 'Wrong receipt schema');
  sameHashes(receipt.sourceSHA256, manifest.sourceSHA256);
  assert(receipt.status === 'passed', 'Receipt was not successful');
  assert(JSON.stringify(receipt.configuration) === JSON.stringify(OPTIONS), 'Unexpected solver configuration');
  assert(JSON.stringify(receipt.budgets) === JSON.stringify(BUDGET), 'Unexpected qualification budgets');
  for (const [name, expected] of Object.entries(expectedWorkloads()))
    verifyWorkloadMatrix(receipt[name], expected, name);
  for (const row of receipt.forceCases) {
    qualify(row.mutual.error, row.mutual.force, row.id);
    validateCalibration(row);
    validateSummaryInputs(row);
    assert(row.mutual.stats.approximated > 0, `${row.id}: no approximate interaction`);
    assert(row.mutual.stats.direct > 0, `${row.id}: no direct interaction`);
  }
  for (const row of receipt.smallNCases) {
    qualify(row.mutual.error, row.mutual.force, row.id); validateCalibration(row);
  }
  for (const row of receipt.trajectoryCases) for (const method of ['direct', 'mutual', 'barnes-hut']) {
    for (const [resolution, dt, steps] of [['coarse', .001, 64], ['fine', .0005, 128]]) {
      const d = row.diagnostics[method][resolution];
      assert(d.dt === dt && d.steps === steps && d.forceSolves === steps + 1 && d.duration === .064,
        `${row.id}: malformed trajectory protocol`);
      if (method === 'mutual') {
        assert([d.maxNormalizedNetForce, d.maxMomentumDrift, d.maxCOMBallisticDrift].every(nonnegativeFinite),
          `${row.id}: invalid trajectory metric`);
        assert(d.maxNormalizedNetForce < BUDGET.normalizedNetForce && d.maxMomentumDrift < 2e-12 && d.maxCOMBallisticDrift < 2e-12,
          `${row.id}: trajectory conservation gate failed`);
      }
    }
  }
  assert(isDeepStrictEqual(receipt.summary, summarizeCases(receipt.forceCases, receipt.trajectoryCases)),
    'Receipt summary does not exactly match validated cases');
  validateTestEvidence(receipt.tests, testLog);
  for (const row of receipt.forceCases) { validateScientificCase(row); validateTimings(row); }
  for (const row of receipt.smallNCases) { validateScientificCase(row, true); validateTimings(row); }
  for (const row of receipt.trajectoryCases) {
    const expected = recomputeTrajectoryCase(row.preset, row.count, row.seed);
    assert(isDeepStrictEqual({ barnesHutTheta: row.barnesHutTheta, diagnostics: row.diagnostics }, expected),
      `${row.id}: recomputed trajectory record differs`);
  }
  sourceHashes();
  if (!quiet) console.log(`Verified source hashes, artifact hashes, and exact workload identities: ${output}`);
}

export function runRetainedTest(index) {
  if (replay) return replay.runRetainedTest(index);
  assert(Number.isInteger(index) && index >= 0 && index < TEST_COMMANDS.length, 'Unknown retained test command');
  sourceHashes();
  const command = TEST_COMMANDS[index];
    const testSource = utf8(Buffer.from(__EVIDENCE_SOURCE_BASE64[command[1]], 'base64'))
      .replaceAll("'../scripts/lib/mutual-gravity-evidence.mjs'", JSON.stringify(__EVIDENCE_HELPER_URL))
      .replaceAll("'../scripts/bench-mutual-gravity.mjs'", JSON.stringify(import.meta.url));
    const run = spawnSync(process.execPath, ['--input-type=module'], { cwd: root, encoding: 'utf8', input: testSource,
      maxBuffer: 8 * 1024 * 1024, timeout: 120000 });
    sourceHashes();
    return { command, execution: 'retained-source-esm-stdin-v1', exitCode: run.status, signal: run.signal ?? null, error: run.error ? String(run.error) : null,
      stdout: run.stdout || '', stderr: run.stderr || '' };
}

export function main() {
if (replay) return replay.main();
let output = path.join(root, 'evidence', 'mutual-gravity-2026-10-15-r6'), verification = false;
for (let i = 2; i < process.argv.length; i++) {
  const arg = process.argv[i];
  if (arg === '--output' && process.argv[i + 1]) output = path.resolve(process.argv[++i]);
  else if (arg === '--verify' && process.argv[i + 1]) { output = path.resolve(process.argv[++i]); verification = true; }
  else if (arg === '--help') {
    console.log('Usage: node scripts/bench-mutual-gravity.mjs [--output NEW_DIRECTORY]\n       node scripts/bench-mutual-gravity.mjs --verify DIRECTORY\nDefault capture directory: evidence/mutual-gravity-2026-10-15-r6 (must not exist)');
    process.exit(0);
  } else throw new Error(`Unknown or incomplete argument: ${arg}`);
}
if (verification) { verify(output); process.exit(0); }
if (fs.existsSync(output)) throw new Error(`Refusing to overwrite existing evidence: ${output}`);
fs.mkdirSync(path.dirname(output), { recursive: true }); fs.mkdirSync(output);
const sourceSHA256 = sourceHashes(), startedAt = new Date().toISOString();
try {
  const testRuns = TEST_COMMANDS.map((_, index) => runRetainedTest(index));
  writeNew(output, 'tests.log', strictJSON({ schema: 'galaxy.mutual-gravity-test-log.v1', runs: testRuns }));
  for (const run of testRuns)
    assert(run.exitCode === 0 && run.error === null && run.signal === null, `${run.command.join(' ')} failed: ${run.error || run.exitCode}`);
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
    const computed = recomputeTrajectoryCase(preset, 128, seed);
    trajectoryCases.push({ id, count: 128, seed, preset, initialStateSHA256: sha256(JSON.stringify(bodies)), ...computed });
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
      verification: 'Recomputes all force/error/balance/tree/stat records, all 41 BH calibration candidates per force/small-N state, and full six-state KDK diagnostics from source-bound deterministic fixtures. Checks timing summaries against raw samples; timings, memory samples, and captured test output remain unsigned observational evidence.',
      matching: 'BH theta grid 0:0.025:1; choose closest achieved RMS among inherited-budget-passing candidates. RMS match means BH/mutual ratio in [0.8,1.25]. p99 and maximum are separately reported; matching is per state, not a portable theta equivalence.',
      timing: 'Both implementations are compiled from retained source-hashed byte buffers in the same native Node realm, bypassing the CommonJS cache (no asymmetric vm context). Two warmup solves and nine measured solves, alternating method order. Timed whole calls include validation, allocations, fresh tree construction, moments, traversal, and propagation. Diagnostics, direct oracle comparison, and calibration are outside the timed call. No forced GC; JIT/GC/scheduling noise remains. Raw wall times only.',
      integration: 'Shared-step kick-drift-kick with a fresh force state after every drift and cached boundary acceleration: steps+1 solves. Six N128 states, duration 0.064, dt=.001 and dt/2=.0005. Every step measures exact all-pairs Plummer potential plus kinetic energy, momentum, ballistic COM, angular momentum, and torque.',
      conservationBoundary: 'Linear momentum qualification does not imply angular momentum or energy conservation. Torque and energy are diagnostics, not qualification guarantees; tree switching can affect both.'
    },
    tests: { commands: TEST_COMMANDS, exitCode: 0, log: 'tests.log', outcomes: testRuns.map(commandOutcome) },
    summary: summarizeCases(forceCases, trajectoryCases), forceCases, smallNCases, trajectoryCases
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

}
if (replay && process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { main(); } catch (error) { console.error(error.stack); process.exitCode = 1; }
}
