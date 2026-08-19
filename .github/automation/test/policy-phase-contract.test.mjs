import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  readPolicyArtifact,
  validatePolicyEvaluationArtifact,
  validatePolicyReceiptArtifact,
  writePolicyArtifactAtomic,
} from '../src/policy-phase-contract.mjs';

const sha = (character) => character.repeat(40);

function artifact() {
  return {
    schema_version: 1,
    artifact_type: 'policy_evaluation',
    repository_id: 123,
    repository: 'JinPengGeng/aeris-token',
    pull_number: 37,
    head_sha: sha('a'),
    base_sha: sha('b'),
    policy_sha: sha('c'),
    snapshot_sha: 'd'.repeat(64),
    evaluated_at: '2026-08-18T12:00:00.000Z',
    result: {
      mode: 'shadow',
      verdict: 'human_required',
      enforcement: 'advisory',
      eligible_for_automatic_merge: false,
      reason_codes: ['human_review_path_changed'],
      unsuccessful_checks: [],
      human_review_paths: ['.github/workflows/policy.yml'],
      changed_file_count: 1,
    },
  };
}

test('policy artifact round-trips through strict atomic IO', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-policy-contract-'));
  try {
    const file = path.join(directory, 'evaluation.json');
    const expected = validatePolicyEvaluationArtifact(artifact());
    assert.deepEqual(writePolicyArtifactAtomic(file, artifact()), expected);
    assert.deepEqual(readPolicyArtifact(file), expected);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test('policy artifact rejects unknown keys and inconsistent eligibility', () => {
  const unknown = artifact();
  unknown.token = 'secret';
  assert.throws(() => validatePolicyEvaluationArtifact(unknown), /unexpected keys/);

  const inconsistent = artifact();
  inconsistent.result.eligible_for_automatic_merge = true;
  assert.throws(() => validatePolicyEvaluationArtifact(inconsistent), /cannot authorize/);
});

test('policy artifact rejects path traversal, duplicate reasons, and oversized arrays', () => {
  const traversal = artifact();
  traversal.result.human_review_paths = ['../secret'];
  assert.throws(() => validatePolicyEvaluationArtifact(traversal), /invalid/);

  const duplicate = artifact();
  duplicate.result.reason_codes = ['human_review_path_changed', 'human_review_path_changed'];
  assert.throws(() => validatePolicyEvaluationArtifact(duplicate), /duplicates/);

  const oversized = artifact();
  oversized.result.unsuccessful_checks = Array.from({ length: 21 }, (_, index) => `check-${index}`);
  assert.throws(() => validatePolicyEvaluationArtifact(oversized), /invalid/);
});

test('policy artifact rejects malformed SHA, repository, timestamp, and empty changes', () => {
  for (const mutate of [
    (value) => { value.head_sha = sha('A'); },
    (value) => { value.snapshot_sha = sha('d'); },
    (value) => { value.repository = 'missing-owner'; },
    (value) => { value.evaluated_at = 'tomorrow'; },
    (value) => { value.result.changed_file_count = 0; },
  ]) {
    const value = artifact();
    mutate(value);
    assert.throws(() => validatePolicyEvaluationArtifact(value));
  }
});

test('policy receipt binds the App-owned check to its exact evaluation', () => {
  const receipt = {
    schema_version: 1,
    artifact_type: 'policy_receipt',
    state: 'published',
    evaluation: artifact(),
    run_id: '32129031246.1',
    policy_app_id: 9001,
    check_run_id: 77,
    check_url: 'https://github.com/JinPengGeng/aeris-token/runs/77',
    conclusion: 'neutral',
    published_at: '2026-08-18T12:01:00.000Z',
  };
  assert.deepEqual(validatePolicyReceiptArtifact(receipt), receipt);

  const drift = structuredClone(receipt);
  drift.check_url = 'https://example.com/check';
  assert.throws(() => validatePolicyReceiptArtifact(drift), /format is invalid/);

  const conclusionDrift = structuredClone(receipt);
  conclusionDrift.conclusion = 'failure';
  assert.throws(() => validatePolicyReceiptArtifact(conclusionDrift), /does not match its evaluation/);
});
