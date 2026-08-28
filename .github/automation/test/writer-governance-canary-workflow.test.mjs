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
const workflowPath = path.join(repoRoot, '.github', 'workflows', 'writer-governance-canary.yml');

function workflow() {
  return yaml.load(fs.readFileSync(workflowPath, 'utf8'));
}

test('governance canary is manually callable and inputless reusable, default-disabled, and default-branch-only', () => {
  const document = workflow();
  assert.deepEqual(document.on, { workflow_dispatch: null, workflow_call: null });
  const job = document.jobs.prove;
  assert.match(job.if, /github\.repository == 'JinPengGeng\/aeris-token'/);
  assert.match(job.if, /github\.actor == 'JinPengGeng'/);
  assert.match(job.if, /github\.event\.repository\.default_branch == 'main'/);
  assert.match(job.if, /github\.ref == format\('refs\/heads\/\{0\}', github\.event\.repository\.default_branch\)/);
  assert.match(job.if, /vars\.AERIS_WRITER_GOVERNANCE_CANARY_ENABLED == 'true'/);
  for (const flag of [
    'AERIS_AGENTS_ENABLED',
    'AERIS_CANDIDATE_AGENTS_ENABLED',
    'AERIS_WRITER_ENABLED',
    'AERIS_UPSTREAM_SYNC_ENABLED',
    'AERIS_AUTONOMOUS_MERGE_ENABLED',
  ]) {
    assert.match(job.if, new RegExp(`vars\\.${flag} == 'false'`));
  }
  assert.equal(job.environment, 'writer');
  assert.deepEqual(document.permissions, { contents: 'read' });
  assert.deepEqual(job.permissions, { contents: 'read' });
});

test('governance canary mints one bounded read-only token and exposes no mutation surface', () => {
  const job = workflow().jobs.prove;
  const tokenSteps = job.steps.filter((step) => /create-github-app-token@/.test(step.uses ?? ''));
  assert.equal(tokenSteps.length, 1);
  assert.deepEqual(
    Object.fromEntries(Object.entries(tokenSteps[0].with).filter(([key]) => key.startsWith('permission-'))),
    {
      'permission-administration': 'read',
      'permission-contents': 'read',
      'permission-pull-requests': 'read',
    },
  );
  assert.equal(tokenSteps[0].with.repositories, '${{ github.event.repository.name }}');
  const serialized = JSON.stringify(job);
  assert.match(serialized, /github-app-attestation\.mjs prove-token/);
  assert.match(serialized, /writer-governance-canary\.mjs/);
  assert.doesNotMatch(
    serialized,
    /\bgh\s+(pr|issue|api)|git\s+(push|commit)|mergePullRequest|markPullRequestReady|convertPullRequestToDraft/,
  );
  assert.deepEqual(Object.entries(job.permissions).filter(([, value]) => value === 'write'), []);
});

test('governance canary binds attested owner and emits only closed non-secret proof fields', () => {
  const steps = workflow().jobs.prove.steps;
  const app = steps.find((step) => step.id === 'writer_app_attestation');
  const governance = steps.find((step) => step.id === 'governance');
  assert.equal(app.env.AERIS_WRITER_APP_NODE_ID, '${{ vars.AERIS_WRITER_APP_NODE_ID }}');
  assert.equal(app.env.AERIS_WRITER_APP_OWNER_DATABASE_ID, '${{ vars.AERIS_WRITER_APP_OWNER_DATABASE_ID }}');
  assert.equal(governance.env.AERIS_WRITER_APP_ID, '${{ steps.writer_app_attestation.outputs.app_id }}');
  assert.equal(governance.env.AERIS_WRITER_APP_SLUG, '${{ steps.writer_app_attestation.outputs.app_slug }}');
  assert.equal(
    governance.env.AERIS_WRITER_APP_OWNER_LOGIN,
    '${{ steps.writer_app_attestation.outputs.app_owner_login }}',
  );
  assert.equal(
    governance.env.AERIS_WRITER_APP_OWNER_DATABASE_ID,
    '${{ steps.writer_app_attestation.outputs.app_owner_database_id }}',
  );
  const summary = steps.find((step) => /Summarize governance proof/.test(step.name));
  assert.deepEqual(Object.keys(summary.env).sort(), ['RULESET_ID', 'SNAPSHOT_SHA256', 'SNAPSHOT_SUMMARY']);
  assert.match(summary.run, /Snapshot SHA-256/);
  assert.match(summary.run, /Governance fence ruleset ID/);
  assert.match(summary.run, /Snapshot summary/);
  assert.doesNotMatch(JSON.stringify(summary), /TOKEN|PRIVATE_KEY|SECRET/);
  for (const step of steps) {
    if (step.uses) assert.match(step.uses, /^[^@\s]+@[0-9a-f]{40}$/);
  }
});
