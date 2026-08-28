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

test('identity bootstrap keeps GITHUB_TOKEN read-only and confines both secrets to one audited step', () => {
  const document = workflow();
  const job = document.jobs.bootstrap;
  assert.deepEqual(document.permissions, { contents: 'read' });
  assert.deepEqual(job.permissions, { contents: 'read' });
  assert.deepEqual(Object.entries(job.permissions).filter(([, value]) => value === 'write'), []);

  const secretSteps = job.steps.filter((step) => /secrets\./.test(JSON.stringify(step)));
  assert.equal(secretSteps.length, 1);
  const bootstrap = secretSteps[0];
  assert.equal(bootstrap.id, 'bootstrap');
  assert.equal(bootstrap.env.AERIS_WRITER_APP_PRIVATE_KEY, '${{ secrets.AERIS_WRITER_APP_PRIVATE_KEY }}');
  assert.equal(bootstrap.env.AERIS_IDENTITY_BOOTSTRAP_TOKEN, '${{ secrets.AERIS_IDENTITY_BOOTSTRAP_TOKEN }}');
  assert.equal(bootstrap.run, 'node .github/automation/src/writer-identity-bootstrap.mjs');
  assert.doesNotMatch(JSON.stringify(job), /permissions:\s*write|actions\/create-github-app-token/);
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

test('identity bootstrap calls local reusable proofs at the bound caller SHA without forwarding its control token', () => {
  const { jobs } = workflow();
  assert.equal(jobs.bootstrap.outputs, undefined);
  assert.equal(jobs.bootstrap.steps.some((step) => step.id === 'caller_sha'), false);
  assert.equal(jobs.readonly_attestation.uses, './.github/workflows/writer-readonly-attestation.yml');
  assert.deepEqual(jobs.readonly_attestation.needs, ['bootstrap']);
  assert.equal(jobs.readonly_attestation.if, undefined);
  assert.equal(jobs.governance_canary.uses, './.github/workflows/writer-governance-canary.yml');
  assert.deepEqual(jobs.governance_canary.needs, ['bootstrap', 'readonly_attestation']);
  assert.equal(jobs.governance_canary.if, undefined);
  for (const reusable of [jobs.readonly_attestation, jobs.governance_canary]) {
    assert.doesNotMatch(JSON.stringify(reusable), /secrets|AERIS_IDENTITY_BOOTSTRAP_TOKEN|@main|workflow_dispatch/);
  }
  assert.doesNotMatch(JSON.stringify(jobs), /actions\/workflows\/[^\s]*\/dispatches|gh api .*dispatch/);
});
