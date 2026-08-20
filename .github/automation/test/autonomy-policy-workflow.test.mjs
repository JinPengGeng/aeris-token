import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const yaml = require('js-yaml');
const testDirectory = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(testDirectory, '..', '..', '..');
const workflowPath = path.join(repoRoot, '.github', 'workflows', 'automation-policy.yml');

function workflow() {
  return yaml.load(fs.readFileSync(workflowPath, 'utf8'));
}

test('Policy is a secretless pull_request job sourced from the exact base SHA', () => {
  const document = workflow();
  assert.deepEqual(Object.keys(document.on), ['pull_request']);
  assert.deepEqual(document.on.pull_request.types, ['opened', 'reopened', 'synchronize', 'labeled', 'unlabeled']);
  assert.deepEqual(document.permissions, { contents: 'read', 'pull-requests': 'read' });
  assert.equal(document.jobs.gate.name, 'gate');
  assert.equal(document.jobs.gate.environment, undefined);
  assert.doesNotMatch(JSON.stringify(document), /secrets\.|checks:\s*write|statuses:\s*write/i);
  const checkout = document.jobs.gate.steps.find((step) => String(step.uses).startsWith('actions/checkout@'));
  assert.equal(checkout.with.ref, '${{ github.event.pull_request.base.sha }}');
  assert.equal(checkout.with['persist-credentials'], false);
  const run = document.jobs.gate.steps.find((step) => /Evaluate deterministic policy/.test(step.name));
  assert.equal(run.run, 'node .github/automation/src/run-autonomy-policy.mjs');
  assert.equal(run.env.AERIS_HEAD_SHA, '${{ github.event.pull_request.head.sha }}');
});

test('Policy serializes all head and label events for one pull request', () => {
  const document = workflow();
  assert.equal(document.concurrency.group, 'automation-policy-${{ github.event.pull_request.number }}');
  assert.equal(document.concurrency['cancel-in-progress'], false);
});

test('all Policy workflow actions are pinned to immutable commits', () => {
  for (const step of workflow().jobs.gate.steps) {
    if (step.uses) assert.match(step.uses, /^[^@\s]+@[0-9a-f]{40}$/);
  }
});
