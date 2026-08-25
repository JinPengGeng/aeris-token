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
const workflowPath = path.join(repoRoot, '.github', 'workflows', 'writer-readonly-attestation.yml');

function workflow() {
  return yaml.load(fs.readFileSync(workflowPath, 'utf8'));
}

test('Writer attestation is a default-branch-only manual workflow without inputs', () => {
  const document = workflow();
  assert.deepEqual(document.on.workflow_dispatch, null);
  assert.deepEqual(Object.keys(document.on), ['workflow_dispatch']);
  assert.match(document.jobs.attest.if, /github\.ref == format\('refs\/heads\/\{0\}', github\.event\.repository\.default_branch\)/);
  assert.equal(document.jobs.attest.environment, 'writer');
  assert.equal(document.permissions.contents, 'read');
  assert.deepEqual(document.jobs.attest.permissions, { contents: 'read' });
  assert.deepEqual(Object.entries(document.jobs.attest.permissions).filter(([, value]) => value === 'write'), []);
});

test('Writer attestation mints one explicitly read-only repository token and has no mutation command', () => {
  const job = workflow().jobs.attest;
  const tokenSteps = job.steps.filter((step) => /create-github-app-token@/.test(step.uses ?? ''));
  assert.equal(tokenSteps.length, 1);
  const permissions = Object.fromEntries(Object.entries(tokenSteps[0].with).filter(([key]) => key.startsWith('permission-')));
  assert.deepEqual(permissions, {
    'permission-administration': 'read',
    'permission-contents': 'read',
    'permission-pull-requests': 'read',
  });
  assert.equal(tokenSteps[0].with.repositories, '${{ github.event.repository.name }}');
  const serialized = JSON.stringify(job);
  assert.match(serialized, /github-app-attestation\.mjs prove-token/);
  assert.doesNotMatch(serialized, /\bgh\s+(pr|issue|api)|git\s+(push|commit)|mergePullRequest|markPullRequestReady|convertPullRequestToDraft/);
  const summary = job.steps.find((step) => /Summarize read-only attestation/.test(step.name));
  assert.equal(summary.env.APP_PERMISSIONS, '${{ steps.writer_app_attestation.outputs.app_permissions }}');
  assert.equal(summary.env.INSTALLATION_PERMISSIONS, '${{ steps.writer_app_attestation.outputs.installation_permissions }}');
  assert.equal(summary.env.REPOSITORY_SELECTION, '${{ steps.writer_app_attestation.outputs.repository_selection }}');
  assert.match(summary.run, /Bot GraphQL identity/);
  assert.match(summary.run, /printf '%s\\n' '- App: `%s` \(#%s\)'/);
  assert.doesNotMatch(summary.run, /`\$\{APP_[A-Z_]+\}`/);
  assert.doesNotMatch(JSON.stringify(summary), /TOKEN|PRIVATE_KEY/);
  for (const step of job.steps) {
    if (step.uses) assert.match(step.uses, /^[^@\s]+@[0-9a-f]{40}$/);
  }
});
