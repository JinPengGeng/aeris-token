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
const workflowPath = path.join(repoRoot, '.github', 'workflows', 'writer-identity-bootstrap.yml');

function workflow() {
  return yaml.load(fs.readFileSync(workflowPath, 'utf8'));
}

test('identity bootstrap is no-input manual, fixed-owner, default-branch-only, and default-off', () => {
  const document = workflow();
  assert.deepEqual(Object.keys(document.on), ['workflow_dispatch']);
  assert.deepEqual(document.on.workflow_dispatch, null);
  const job = document.jobs.bootstrap;
  assert.equal(job.environment, 'writer');
  assert.match(job.if, /github\.repository == 'JinPengGeng\/aeris-token'/);
  assert.match(job.if, /github\.actor == 'JinPengGeng'/);
  assert.match(job.if, /github\.event\.repository\.default_branch == 'main'/);
  assert.match(job.if, /vars\.AERIS_WRITER_IDENTITY_BOOTSTRAP_ENABLED == 'true'/);
  assert.match(job.if, /vars\.AERIS_WRITER_GOVERNANCE_CANARY_ENABLED == 'true'/);
  const checkout = job.steps.find((step) => step.uses?.startsWith('actions/checkout@'));
  assert.equal(checkout.with.ref, '${{ github.sha }}');
  assert.equal(checkout.with['persist-credentials'], false);
  for (const name of [
    'AERIS_AGENTS_ENABLED',
    'AERIS_CANDIDATE_AGENTS_ENABLED',
    'AERIS_WRITER_ENABLED',
    'AERIS_UPSTREAM_SYNC_ENABLED',
    'AERIS_AUTONOMOUS_MERGE_ENABLED',
  ]) {
    assert.match(job.if, new RegExp(`vars\\.${name} == 'false'`));
  }
});

test('identity bootstrap keeps GITHUB_TOKEN read-only and confines the temporary control token to its bootstrap step', () => {
  const document = workflow();
  const job = document.jobs.bootstrap;
  assert.deepEqual(document.permissions, { contents: 'read' });
  assert.deepEqual(job.permissions, { contents: 'read' });
  assert.deepEqual(Object.entries(job.permissions).filter(([, value]) => value === 'write'), []);

  const bootstrap = job.steps.find((step) => step.id === 'bootstrap');
  assert.equal(bootstrap.id, 'bootstrap');
  assert.equal(bootstrap.env.AERIS_WRITER_APP_PRIVATE_KEY, '${{ secrets.AERIS_WRITER_APP_PRIVATE_KEY }}');
  assert.equal(bootstrap.env.AERIS_IDENTITY_BOOTSTRAP_TOKEN, '${{ secrets.AERIS_IDENTITY_BOOTSTRAP_TOKEN }}');
  assert.equal(bootstrap.run, 'node .github/automation/src/writer-identity-bootstrap.mjs');
  const controlTokenSteps = job.steps.filter(
    (step) => step.env?.AERIS_IDENTITY_BOOTSTRAP_TOKEN === '${{ secrets.AERIS_IDENTITY_BOOTSTRAP_TOKEN }}',
  );
  assert.deepEqual(controlTokenSteps.map((step) => step.id), ['bootstrap']);
  const privateKeySteps = job.steps.filter((step) => /AERIS_WRITER_APP_PRIVATE_KEY|private-key/.test(JSON.stringify(step)));
  assert.deepEqual(privateKeySteps.map((step) => step.id), ['bootstrap', 'writer_app_attestation', 'writer_token']);
  assert.doesNotMatch(JSON.stringify(job), /permissions:\s*write/);
});

test('identity bootstrap passes every closed-state guard to the CLI and emits only non-sensitive outputs', () => {
  const job = workflow().jobs.bootstrap;
  const bootstrap = job.steps.find((step) => step.id === 'bootstrap');
  for (const name of [
    'AERIS_WRITER_IDENTITY_BOOTSTRAP_ENABLED',
    'AERIS_WRITER_GOVERNANCE_CANARY_ENABLED',
    'AERIS_AGENTS_ENABLED',
    'AERIS_CANDIDATE_AGENTS_ENABLED',
    'AERIS_WRITER_ENABLED',
    'AERIS_UPSTREAM_SYNC_ENABLED',
    'AERIS_AUTONOMOUS_MERGE_ENABLED',
  ]) {
    assert.equal(bootstrap.env[name], `\${{ vars.${name} }}`);
  }
  const summary = job.steps.find((step) => /Summarize non-sensitive identity proof/.test(step.name));
  assert.ok(summary);
  assert.match(summary.run, /Bootstrap flag was set to `false`/);
  assert.match(summary.run, /Remove the temporary `AERIS_IDENTITY_BOOTSTRAP_TOKEN`/);
  assert.doesNotMatch(JSON.stringify(summary.env), /TOKEN|PRIVATE_KEY|RAW|RESPONSE|HEADER/i);
  assert.doesNotMatch(summary.run, /\$\{AERIS_IDENTITY_BOOTSTRAP_TOKEN\}|\$\{AERIS_WRITER_APP_PRIVATE_KEY\}/);
});

test('identity bootstrap pins every external action to an immutable commit', () => {
  for (const step of workflow().jobs.bootstrap.steps) {
    if (step.uses) assert.match(step.uses, /^[^@\s]+@[0-9a-f]{40}$/);
  }
});

test('identity bootstrap runs both proofs inline with its newly bound identity and one bounded read-only token', () => {
  const { jobs } = workflow();
  assert.deepEqual(Object.keys(jobs), ['bootstrap']);
  const steps = jobs.bootstrap.steps;
  const app = steps.find((step) => step.id === 'writer_app_attestation');
  const token = steps.find((step) => step.id === 'writer_token');
  const tokenProof = steps.find((step) => step.id === 'writer_token_proof');
  const governance = steps.find((step) => step.id === 'governance');
  assert.equal(app.env.AERIS_WRITER_APP_NODE_ID, '${{ steps.bootstrap.outputs.app_node_id }}');
  assert.equal(
    app.env.AERIS_WRITER_APP_OWNER_DATABASE_ID,
    '${{ steps.bootstrap.outputs.app_owner_database_id }}',
  );
  assert.deepEqual(
    Object.fromEntries(Object.entries(token.with).filter(([key]) => key.startsWith('permission-'))),
    {
      'permission-administration': 'read',
      'permission-contents': 'read',
      'permission-pull-requests': 'read',
    },
  );
  assert.equal(tokenProof.env.AERIS_WRITER_TOKEN, '${{ steps.writer_token.outputs.token }}');
  assert.equal(governance.env.AERIS_WRITER_TOKEN, '${{ steps.writer_token.outputs.token }}');
  assert.equal(governance.env.AERIS_WRITER_APP_ID, '${{ steps.writer_app_attestation.outputs.app_id }}');
  assert.equal(
    governance.env.AERIS_WRITER_APP_OWNER_DATABASE_ID,
    '${{ steps.writer_app_attestation.outputs.app_owner_database_id }}',
  );
  assert.ok(steps.findIndex((step) => step.id === 'bootstrap') < steps.findIndex((step) => step.id === 'writer_app_attestation'));
  assert.ok(steps.findIndex((step) => step.id === 'writer_token_proof') < steps.findIndex((step) => step.id === 'governance'));
  const proofSteps = steps.filter((step) => step.id !== 'bootstrap');
  assert.equal(proofSteps.some((step) => step.env?.AERIS_IDENTITY_BOOTSTRAP_TOKEN !== undefined), false);
  assert.doesNotMatch(JSON.stringify(jobs), /\.\/\.github\/workflows\/writer-(readonly-attestation|governance-canary)\.yml/);
  assert.doesNotMatch(JSON.stringify(jobs), /actions\/workflows\/[^\s]*\/dispatches|gh api .*dispatch/);
});
