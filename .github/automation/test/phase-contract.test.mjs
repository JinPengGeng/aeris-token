import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  MAX_ARTIFACT_BYTES,
  readArtifact,
  readJsonFile,
  resolvePhasePaths,
  validateAnalysisArtifact,
  validatePhaseArtifact,
  validatePreflightArtifact,
  validateReservationArtifact,
  writeArtifactAtomic,
} from '../src/phase-contract.mjs';
import { runPhaseCli } from '../src/run-phase.mjs';

const policySha = 'a'.repeat(40);
const inputSha = 'b'.repeat(64);

function context(overrides = {}) {
  return {
    kind: 'issue',
    event_name: 'issues',
    number: 12,
    agent: 'triage',
    source_key: 'issue:opened:101:2026-08-12T00:00:00Z',
    object_id: 'issue:12',
    object_generation: '2026-08-12T00:00:00Z',
    input_sha: inputSha,
    policy_sha: policySha,
    run_id: '501',
    ...overrides,
  };
}

function preflight(overrides = {}) {
  return {
    schema_version: 1,
    artifact_type: 'preflight',
    state: 'ready',
    decision: { action: 'analyze', reason: 'issue_opened' },
    context: context(),
    input: null,
    ...overrides,
  };
}

function reservation(overrides = {}) {
  return {
    schema_version: 1,
    artifact_type: 'reservation',
    state: 'reserved',
    preflight: preflight(),
    reservation: {
      comment_id: 91,
      comment_updated_at: '2026-08-12T00:00:01.000Z',
      lease_expires_at: '2026-08-12T00:15:00.000Z',
      lease_token: 'c'.repeat(64),
      cancel_epoch: 0,
    },
    result: null,
    ...overrides,
  };
}

function analysis(overrides = {}) {
  return {
    schema_version: 1,
    artifact_type: 'analysis',
    state: 'completed',
    reservation: reservation(),
    output: { schema_version: 1, agent: 'triage', summary: 'bounded result' },
    model: { alias: 'default', id: 'model-1', duration_ms: 20, usage: { total_tokens: 4 } },
    failure: null,
    ...overrides,
  };
}

test('preflight contract accepts a ready fingerprint without model source text', () => {
  const source = preflight();
  const artifact = validatePreflightArtifact(source);
  assert.deepEqual(artifact, source);
  assert.notEqual(artifact, source);
  assert.equal(artifact.input, null);
});

test('terminal preflight cannot carry model input or an analysis decision', () => {
  const terminal = preflight({
    state: 'terminal',
    decision: { action: 'skip', reason: 'unsupported_event' },
    context: context({ agent: null, input_sha: null }),
    input: null,
  });
  assert.equal(validatePreflightArtifact(terminal).state, 'terminal');
  assert.throws(() => validatePreflightArtifact({ ...terminal, input: {} }), /must not contain model input/);
  assert.throws(
    () => validatePreflightArtifact({ ...terminal, decision: { action: 'analyze', reason: 'bad' } }),
    /state and decision disagree/,
  );
});

test('contracts reject unknown keys, wrong versions, and malformed hashes', () => {
  assert.throws(() => validatePhaseArtifact({ ...preflight(), surprise: true }), /unexpected keys/);
  assert.throws(() => validatePreflightArtifact({ ...preflight(), schema_version: 2 }), /schema_version/);
  assert.throws(
    () => validatePreflightArtifact({ ...preflight(), context: context({ input_sha: 'not-a-hash' }) }),
    /input_sha format/,
  );
});

test('reservation and analysis enforce legal state transitions', () => {
  assert.equal(validateReservationArtifact(reservation()).state, 'reserved');
  assert.equal(validateAnalysisArtifact(analysis()).state, 'completed');
  assert.throws(
    () => validateReservationArtifact({ ...reservation(), preflight: preflight({ state: 'terminal' }) }),
    /preflight state and decision disagree|requires ready preflight/,
  );
  assert.throws(
    () => validateAnalysisArtifact({ ...analysis(), state: 'failed', failure: { code: 'timeout' } }),
    /must not contain model output/,
  );
  assert.throws(
    () => validateReservationArtifact({
      ...reservation(),
      reservation: { ...reservation().reservation, lease_token: 'short' },
    }),
    /lease_token format/,
  );
  assert.throws(
    () => validateReservationArtifact({
      ...reservation(),
      reservation: { ...reservation().reservation, cancel_epoch: -1 },
    }),
    /cancel_epoch/,
  );
  assert.throws(
    () => validateReservationArtifact({
      ...reservation(),
      reservation: { ...reservation().reservation, cancel_epoch: null },
    }),
    /cancel_epoch/,
  );
});

test('terminal artifacts pass through without model output', () => {
  const terminalPreflight = preflight({
    state: 'terminal',
    decision: { action: 'cancel', reason: 'cancel_command' },
    context: context({ agent: null, input_sha: null }),
    input: null,
  });
  const terminalReservation = reservation({
    state: 'terminal',
    preflight: terminalPreflight,
    reservation: null,
    result: { state: 'cancelled', reason: 'cancel_command', comment_id: 91 },
  });
  const terminalAnalysis = analysis({
    state: 'terminal',
    reservation: terminalReservation,
    output: null,
    model: null,
    failure: null,
  });
  assert.equal(validateAnalysisArtifact(terminalAnalysis).state, 'terminal');
});

test('publication rejects an unknown terminal state', () => {
  assert.throws(
    () => validatePhaseArtifact({
      schema_version: 1,
      artifact_type: 'publication',
      state: 'invented_state',
      analysis: analysis(),
      result: { state: 'invented_state', reason: null, comment_id: null },
    }),
    /publication state is invalid/,
  );
});

test('phase paths are confined to the configured artifact root', () => {
  const root = path.join(os.tmpdir(), 'aeris-contract-root');
  const paths = resolvePhasePaths('reserve', {
    AERIS_ARTIFACT_ROOT: root,
    AERIS_INPUT_PATH: path.join(root, 'download', 'preflight.json'),
    AERIS_OUTPUT_PATH: path.join(root, 'output', 'reservation.json'),
  });
  assert.equal(paths.inputPath, path.resolve(root, 'download', 'preflight.json'));
  assert.throws(
    () => resolvePhasePaths('preflight', { AERIS_ARTIFACT_ROOT: root, AERIS_OUTPUT_PATH: path.join(root, '..', 'escape.json') }),
    /within artifact root/,
  );
});

test('bounded reader rejects oversized and symbolic-link inputs', (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-contract-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const oversized = path.join(directory, 'oversized.json');
  fs.writeFileSync(oversized, Buffer.alloc(MAX_ARTIFACT_BYTES + 1, 0x20));
  assert.throws(() => readJsonFile(oversized), /exceeds maximum size/);

  const target = path.join(directory, 'target.json');
  const link = path.join(directory, 'link.json');
  fs.writeFileSync(target, '{}');
  try {
    fs.symlinkSync(target, link);
    assert.throws(() => readJsonFile(link), /regular file/);
  } catch (error) {
    if (error.code !== 'EPERM') throw error;
  }
});

test('atomic writer replaces a validated artifact without leaving temp files', (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-contract-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const file = path.join(directory, 'nested', 'preflight.json');
  writeArtifactAtomic(file, preflight(), 'preflight');
  const replacement = preflight({ decision: { action: 'analyze', reason: 'workflow_dispatch' } });
  writeArtifactAtomic(file, replacement, 'preflight');
  assert.deepEqual(readArtifact(file, 'preflight'), replacement);
  assert.deepEqual(fs.readdirSync(path.dirname(file)), ['preflight.json']);
});

test('runPhaseCli dispatches a phase with validated input and output', async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-contract-cli-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const inputPath = path.join(root, 'preflight.json');
  const outputPath = path.join(root, 'reservation.json');
  writeArtifactAtomic(inputPath, preflight(), 'preflight');
  let received;
  const result = await runPhaseCli({
    argv: ['reserve'],
    environment: {
      AERIS_ARTIFACT_ROOT: root,
      AERIS_INPUT_PATH: inputPath,
      AERIS_OUTPUT_PATH: outputPath,
    },
    repoRoot: root,
    engine: {
      async runReservationPhase(options) {
        received = options;
        return reservation();
      },
    },
  });
  assert.equal(received.artifact.artifact_type, 'preflight');
  assert.equal(result.artifact.artifact_type, 'reservation');
  assert.equal(readArtifact(outputPath, 'reservation').state, 'reserved');
});
