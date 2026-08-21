import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const yaml = require('js-yaml');
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const document = yaml.load(fs.readFileSync(path.join(root, '.github/workflows/autonomy-hold.yml'), 'utf8'));

test('hold initializer accepts trusted PR events and explicit dispatch', () => {
  assert.deepEqual(document.on.pull_request_target.types, ['opened', 'reopened', 'synchronize', 'edited']);
  assert.equal(document.on.workflow_dispatch.inputs.pull_number.required, true);
  assert.equal(document.concurrency.group, 'aeris-autonomy-pr-${{ github.event.pull_request.number || inputs.pull_number }}');
  assert.equal(document.concurrency['cancel-in-progress'], false);
});

test('hold initializer has only required token permissions and trusted checkout', () => {
  assert.deepEqual(document.permissions, { checks: 'write', contents: 'read', 'pull-requests': 'read' });
  const steps = document.jobs.initialize.steps;
  const checkout = steps.find((step) => String(step.uses).startsWith('actions/checkout@'));
  assert.equal(checkout.with.ref, '${{ github.event.repository.default_branch }}');
  assert.equal(checkout.with['persist-credentials'], false);
  assert.doesNotMatch(JSON.stringify(document), /github\.event\.pull_request\.head/);
  assert.equal(steps.find((step) => /Set up Node/.test(step.name)).with['node-version'], 22);
  for (const step of steps) if (step.uses) assert.match(step.uses, /^[^@\s]+@[0-9a-f]{40}$/);
});

test('initializer receives only trusted metadata and never a Writer credential', () => {
  const env = document.jobs.initialize.steps.find((step) => /Initialize exact-head/.test(step.name)).env;
  assert.deepEqual(Object.keys(env).sort(), ['AERIS_DEFAULT_BRANCH', 'AERIS_PULL_NUMBER', 'AERIS_WRITER_APP_SLUG', 'GITHUB_REPOSITORY', 'GITHUB_REPOSITORY_ID', 'GITHUB_TOKEN']);
  assert.equal(env.AERIS_PULL_NUMBER, '${{ github.event.pull_request.number || inputs.pull_number }}');
});
