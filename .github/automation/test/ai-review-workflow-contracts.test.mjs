import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import yaml from 'js-yaml';

import { REVIEW_ATTESTATION_CHECK_NAMES } from '../src/review-attestation-contract.mjs';

const root = path.resolve(import.meta.dirname, '..', '..', '..');
const workflowPath = path.join(root, '.github/workflows/automation-ai-review.yml');
const source = fs.readFileSync(workflowPath, 'utf8');
const workflow = yaml.load(source);
const serialized = (value) => JSON.stringify(value);

test('AI attestation is a trusted-main manual workflow with no PR checkout and no cancellation replacement', () => {
  assert.equal(workflow.name, 'Automation AI Review Attestation');
  assert.ok(workflow.on.workflow_dispatch);
  assert.equal(workflow.on.pull_request, undefined);
  assert.equal(workflow.on.pull_request_target, undefined);
  assert.match(workflow.concurrency.group, /repository_id.*pull_request_number/);
  assert.equal(workflow.concurrency['cancel-in-progress'], false);
  for (const job of Object.values(workflow.jobs)) for (const step of job.steps ?? []) {
    if (typeof step.uses === 'string') assert.match(step.uses, /^[^@\s]+@[0-9a-f]{40}$/);
    if (String(step.uses ?? '').startsWith('actions/checkout@')) {
      assert.equal(step.with.ref, '${{ github.event.repository.default_branch }}');
      assert.equal(step.with['persist-credentials'], false);
    }
  }
});

test('only finalize has checks write and only analyze receives the AI key', () => {
  assert.deepEqual(workflow.jobs.prepare.permissions, { contents: 'read', 'pull-requests': 'read' });
  assert.deepEqual(workflow.jobs.analyze.permissions, {});
  assert.deepEqual(workflow.jobs.finalize.permissions, { contents: 'read', 'pull-requests': 'read', checks: 'write' });
  assert.doesNotMatch(serialized(workflow.jobs.prepare), /AERIS_AI_API_KEY/);
  assert.match(serialized(workflow.jobs.analyze), /AERIS_AI_API_KEY/);
  assert.doesNotMatch(serialized(workflow.jobs.analyze), /AERIS_POLICY_PRIVATE_KEY|AERIS_REVIEWER_APP|AERIS_SECURITY_APP/);
  assert.doesNotMatch(serialized(workflow.jobs.finalize), /AERIS_AI_API_KEY/);
  assert.match(serialized(workflow.jobs.finalize), /always\(\)/);
  assert.match(serialized(workflow.jobs.finalize), /AERIS_PREPARE_JOB_RESULT/);
  assert.match(serialized(workflow.jobs.finalize), /AERIS_ANALYZE_JOB_RESULT/);
});

test('activation is default-off and terminal checks are producer-owned by Actions', () => {
  assert.match(source, /AERIS_AI_ATTESTATION_ENABLED == 'true'/);
  assert.match(source, /checks:\s*write/);
  assert.doesNotMatch(source, /create-github-app-token/);
  assert.deepEqual(REVIEW_ATTESTATION_CHECK_NAMES, {
    reviewer: 'Automation Review Attestation / reviewer',
    security: 'Automation Review Attestation / security',
  });
});
