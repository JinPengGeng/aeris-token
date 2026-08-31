import assert from 'node:assert/strict';
import test from 'node:test';

import {
  AutonomyPolicyGateError,
  policyConfigFromEnvironment,
  runAutonomyPolicyGate,
} from '../src/run-autonomy-policy.mjs';

const base = Object.freeze({
  GITHUB_REPOSITORY: 'JinPengGeng/aeris-token',
  AERIS_DEFAULT_BRANCH: 'main',
});

const gateEnvironment = Object.freeze({
  ...base,
  GITHUB_REPOSITORY_ID: '1310462380',
  AERIS_PULL_NUMBER: '17',
  AERIS_HEAD_SHA: 'b'.repeat(40),
  AERIS_POLICY_REF: 'main',
  AERIS_POLICY_SHA: 'a'.repeat(40),
});

function policyResult(classification, reasons) {
  return Object.freeze({
    decision: Object.freeze({ classification, reasons: Object.freeze(reasons) }),
    snapshot: Object.freeze({ head: Object.freeze({ sha: gateEnvironment.AERIS_HEAD_SHA }) }),
  });
}

test('Policy config uses a disabled sentinel until the Writer App is configured', () => {
  assert.deepEqual(policyConfigFromEnvironment(base), {
    repository: 'JinPengGeng/aeris-token',
    base_ref: 'main',
    writer_login: 'aeris-disabled-writer[bot]',
    branch_prefix: 'agent/issue-',
    maximum_files: 20,
    maximum_changes: 2000,
  });
});

test('enabled Writer policy requires one valid App slug', () => {
  assert.deepEqual(policyConfigFromEnvironment({
    ...base,
    AERIS_WRITER_ENABLED: 'true',
    AERIS_WRITER_APP_SLUG: 'aeris-writer',
  }).writer_login, 'aeris-writer[bot]');
  assert.throws(
    () => policyConfigFromEnvironment({ ...base, AERIS_WRITER_ENABLED: 'true' }),
    (error) => error instanceof AutonomyPolicyGateError && /required/.test(error.message),
  );
  assert.throws(
    () => policyConfigFromEnvironment({ ...base, AERIS_WRITER_ENABLED: 'sometimes' }),
    /must be true, false, 1, or 0/,
  );
});

test('the workflow gate succeeds for manual pulls and fails for label denials', async () => {
  const manual = await runAutonomyPolicyGate(gateEnvironment, {
    client: {},
    evaluatePolicy: async () => policyResult('manual', ['manual_unmanaged_branch']),
  });
  assert.equal(manual.decision.classification, 'manual');

  await assert.rejects(
    () => runAutonomyPolicyGate(gateEnvironment, {
      client: {},
      evaluatePolicy: async () => policyResult('deny', ['deny_do_not_merge_label']),
    }),
    (error) => error instanceof AutonomyPolicyGateError && /deny_do_not_merge_label/.test(error.message),
  );
  await assert.rejects(
    () => runAutonomyPolicyGate(gateEnvironment, {
      client: {},
      evaluatePolicy: async () => policyResult('deny', ['deny_autonomy_manual_label']),
    }),
    (error) => error instanceof AutonomyPolicyGateError && /deny_autonomy_manual_label/.test(error.message),
  );
});
