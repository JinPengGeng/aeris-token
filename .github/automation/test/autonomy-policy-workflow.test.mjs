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

test('Policy is a secretless pull_request and dispatch job sourced from the trusted revision', () => {
  const document = workflow();
  assert.deepEqual(Object.keys(document.on), ['pull_request', 'workflow_dispatch']);
  assert.deepEqual(document.on.pull_request.types, ['opened', 'reopened', 'synchronize', 'labeled', 'unlabeled']);
  assert.deepEqual(document.permissions, { contents: 'read', 'pull-requests': 'read' });
  assert.equal(document.jobs.gate.name, 'Automation Policy / gate');
  assert.equal(document.jobs.gate.environment, undefined);
  assert.doesNotMatch(JSON.stringify(document), /secrets\.|checks:\s*write|statuses:\s*write/i);
  const checkout = document.jobs.gate.steps.find((step) => String(step.uses).startsWith('actions/checkout@'));
  assert.equal(checkout.with.ref, '${{ github.event.pull_request.base.sha || inputs.policy_sha }}');
  assert.equal(checkout.with['persist-credentials'], false);
  const run = document.jobs.gate.steps.find((step) => /Evaluate deterministic policy/.test(step.name));
  assert.match(run.run, /if \[ ! -f \.github\/automation\/src\/run-autonomy-policy\.mjs \]; then/);
  assert.match(run.run, /Automation policy runtime missing/);
  assert.match(run.run, /exit 1/);
  assert.doesNotMatch(run.run, /exit 0/);
  assert.match(run.run, /node \.github\/automation\/src\/run-autonomy-policy\.mjs/);
  assert.equal(run.run.trim().endsWith('node .github/automation/src/run-autonomy-policy.mjs'), true);
  assert.equal(run.env.AERIS_HEAD_SHA, '${{ github.event.pull_request.head.sha || github.sha }}');
});

test('Policy dispatch mode pins the exact pull request and trusted policy SHA', () => {
  const inputs = workflow().on.workflow_dispatch.inputs;
  assert.deepEqual(Object.keys(inputs).sort(), ['policy_sha', 'pull_number', 'ref']);
  for (const name of Object.keys(inputs)) assert.equal(inputs[name].required, true);
  assert.equal(inputs.ref.type, 'string');
  assert.equal(inputs.pull_number.type, 'number');
  assert.equal(inputs.policy_sha.type, 'string');
  const run = workflow().jobs.gate.steps.find((step) => /Evaluate deterministic policy/.test(step.name));
  assert.equal(run.env.AERIS_PULL_NUMBER, '${{ github.event.pull_request.number || inputs.pull_number }}');
  assert.equal(run.env.AERIS_POLICY_REF, '${{ github.event.pull_request.base.ref || github.event.repository.default_branch }}');
  assert.equal(run.env.AERIS_POLICY_SHA, '${{ github.event.pull_request.base.sha || inputs.policy_sha }}');
  // The job check run attaches to the dispatched ref tip, so the ref input
  // must name exactly that ref; a mismatch fails closed before evaluation.
  assert.equal(run.env.AERIS_DISPATCH_REF, '${{ inputs.ref }}');
  assert.match(run.run, /GITHUB_EVENT_NAME.*workflow_dispatch/);
  assert.match(run.run, /AERIS_DISPATCH_REF.*GITHUB_REF_NAME/);
});

test('Policy serializes all head and label events for one pull request', () => {
  const document = workflow();
  assert.equal(document.concurrency.group, 'automation-policy-${{ github.event.pull_request.number || inputs.pull_number }}');
  assert.equal(document.concurrency['cancel-in-progress'], false);
});

test('all Policy workflow actions are pinned to immutable commits', () => {
  for (const step of workflow().jobs.gate.steps) {
    if (step.uses) assert.match(step.uses, /^[^@\s]+@[0-9a-f]{40}$/);
  }
});
