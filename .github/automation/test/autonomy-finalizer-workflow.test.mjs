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
const workflowPath = path.join(repoRoot, '.github', 'workflows', 'autonomy-finalizer.yml');

function workflow() {
  return yaml.load(fs.readFileSync(workflowPath, 'utf8'));
}

test('Finalizer listens to every required workflow to avoid completion-order races', () => {
  const document = workflow();
  assert.deepEqual(document.on.workflow_run.workflows, ['Automation Policy', 'Rust CI', 'Frontend CI', 'Autonomy Publisher']);
  assert.deepEqual(document.on.workflow_run.types, ['completed']);
  assert.match(document.jobs.evaluate.if, /pull_request/);
  assert.match(document.jobs.evaluate.if, /agent\/issue-/);
  assert.match(document.jobs.finalize.if, /AERIS_AUTONOMOUS_MERGE_ENABLED/);
  assert.equal(
    document.concurrency.group,
    "aeris-autonomy-pr-${{ github.event.workflow_run.pull_requests[0].number || format('run-{0}', github.event.workflow_run.id) }}",
  );
  assert.doesNotMatch(document.concurrency.group, /head_sha/);
  assert.doesNotMatch(document.concurrency.group, /head_branch/);
  assert.match(document.jobs.evaluate.if, /pull_requests\[0\]\.number != null/);
  assert.match(document.jobs.evaluate.if, /Autonomy Publisher/);
  assert.match(document.jobs.evaluate.if, /autonomy-publisher\.yml/);
  assert.match(document.jobs.evaluate.if, /workflow_run/);
});

test('Finalizer repeats preliminary gates before the sole Writer token mint', () => {
  const document = workflow();
  assert.doesNotMatch(JSON.stringify(document.jobs.evaluate), /secrets\.|AERIS_WRITER_TOKEN/);
  const evaluate = document.jobs.evaluate.steps.find((step) => step.id === 'evaluate');
  const steps = document.jobs.finalize.steps;
  const recheck = steps.findIndex((step) => /Recompute gates before token mint/.test(step.name));
  const proof = steps.findIndex((step) => /Attest Writer App and installation identity/.test(step.name));
  const mint = steps.findIndex((step) => /Mint bounded Writer App token/.test(step.name));
  assert.ok(recheck >= 0 && recheck < proof && proof < mint);
  assert.equal(evaluate.env.AERIS_FINALIZER_PROOF_LEVEL, 'preliminary');
  assert.equal(evaluate.env.AERIS_FINALIZER_TRIGGER_SOURCE, "${{ github.event.workflow_run.name == 'Autonomy Publisher' && 'publisher' || 'required_check' }}");
  assert.equal(evaluate.env.AERIS_WRITER_APP_ID, '${{ vars.AERIS_WRITER_APP_ID }}');
  assert.equal(steps[recheck].env.AERIS_FINALIZER_PROOF_LEVEL, 'preliminary');
  assert.equal(steps[recheck].env.AERIS_FINALIZER_TRIGGER_SOURCE, "${{ github.event.workflow_run.name == 'Autonomy Publisher' && 'publisher' || 'required_check' }}");
  assert.equal(steps[recheck].env.AERIS_WRITER_APP_ID, '${{ vars.AERIS_WRITER_APP_ID }}');
  assert.equal(document.jobs.evaluate.outputs.proof_level, '${{ steps.evaluate.outputs.proof_level }}');
  assert.equal(steps.filter((step) => /create-github-app-token@/.test(step.uses ?? '')).length, 1);
  for (const job of [document.jobs.evaluate, document.jobs.finalize]) {
    assert.equal(job.permissions.issues, 'read');
    const target = job.steps.find((step) => step.name === 'Download Publisher target');
    assert.equal(target.if, "github.event.workflow_run.name == 'Autonomy Publisher'");
    assert.equal(target.with.name, 'aeris-publisher-target-${{ github.event.workflow_run.id }}-${{ github.event.workflow_run.run_attempt }}');
    assert.equal(target.with['run-id'], '${{ github.event.workflow_run.id }}');
    assert.equal(target.with['github-token'], '${{ github.token }}');
  }
  assert.deepEqual(
    Object.keys(steps[mint].with).filter((key) => key.startsWith('permission-')).sort(),
    ['permission-administration', 'permission-contents', 'permission-pull-requests'],
  );
  assert.equal(steps[mint].with['permission-administration'], 'read');
  assert.equal(steps[mint].with['permission-checks'], undefined);
  assert.equal(document.jobs.finalize.permissions.checks, 'read');
  assert.deepEqual(
    Object.entries(document.jobs.finalize.permissions).filter(([, permission]) => permission === 'write'),
    [],
  );
  const finalize = steps.find((step) => /Directly squash merge exact eligible pull request/.test(step.name));
  assert.equal(steps[proof].env.AERIS_WRITER_APP_ID, '${{ vars.AERIS_WRITER_APP_ID }}');
  assert.equal(steps[proof].env.AERIS_WRITER_APP_SLUG, '${{ vars.AERIS_WRITER_APP_SLUG }}');
  assert.equal(steps[proof].env.AERIS_WRITER_APP_NODE_ID, '${{ vars.AERIS_WRITER_APP_NODE_ID }}');
  assert.equal(steps[proof].env.AERIS_WRITER_APP_OWNER_DATABASE_ID, '${{ vars.AERIS_WRITER_APP_OWNER_DATABASE_ID }}');
  assert.equal(steps[proof].env.AERIS_WRITER_INSTALLATION_ID, '${{ vars.AERIS_WRITER_INSTALLATION_ID }}');
  assert.equal(steps[proof].env.AERIS_WRITER_APP_PRIVATE_KEY, '${{ secrets.AERIS_WRITER_APP_PRIVATE_KEY }}');
  assert.equal(steps[proof].run, 'node .github/automation/src/github-app-attestation.mjs');
  assert.equal(steps[mint].with['app-id'], '${{ vars.AERIS_WRITER_APP_ID }}');
  assert.equal(finalize.env.AERIS_WRITER_APP_ID, '${{ vars.AERIS_WRITER_APP_ID }}');
  assert.equal(finalize.env.AERIS_WRITER_PROOF_APP_ID, '${{ steps.writer_app_attestation.outputs.app_id }}');
  assert.equal(finalize.env.AERIS_WRITER_PROOF_APP_SLUG, '${{ steps.writer_app_attestation.outputs.app_slug }}');
  assert.equal(finalize.env.AERIS_WRITER_PROOF_APP_NODE_ID, '${{ steps.writer_app_attestation.outputs.app_node_id }}');
  assert.equal(finalize.env.AERIS_WRITER_PROOF_APP_OWNER_LOGIN, '${{ steps.writer_app_attestation.outputs.app_owner_login }}');
  assert.equal(finalize.env.AERIS_WRITER_PROOF_APP_OWNER_DATABASE_ID, '${{ steps.writer_app_attestation.outputs.app_owner_database_id }}');
  assert.equal(finalize.env.AERIS_WRITER_PROOF_APP_OWNER_TYPE, '${{ steps.writer_app_attestation.outputs.app_owner_type }}');
  assert.equal(finalize.env.AERIS_WRITER_PROOF_APP_PERMISSIONS, '${{ steps.writer_app_attestation.outputs.app_permissions }}');
  assert.equal(finalize.env.AERIS_WRITER_INSTALLATION_ID, '${{ vars.AERIS_WRITER_INSTALLATION_ID }}');
  assert.equal(finalize.env.AERIS_WRITER_PROOF_INSTALLATION_ID, '${{ steps.writer_app_attestation.outputs.installation_id }}');
  assert.equal(finalize.env.AERIS_WRITER_PROOF_INSTALLATION_ACCOUNT_LOGIN, '${{ steps.writer_app_attestation.outputs.installation_account_login }}');
  assert.equal(finalize.env.AERIS_WRITER_PROOF_INSTALLATION_ACCOUNT_TYPE, '${{ steps.writer_app_attestation.outputs.installation_account_type }}');
  assert.equal(finalize.env.AERIS_WRITER_PROOF_INSTALLATION_PERMISSIONS, '${{ steps.writer_app_attestation.outputs.installation_permissions }}');
  assert.equal(finalize.env.AERIS_WRITER_PROOF_REPOSITORY_SELECTION, '${{ steps.writer_app_attestation.outputs.repository_selection }}');
  assert.equal(finalize.env.AERIS_WRITER_TOKEN_INSTALLATION_ID, '${{ steps.writer_token.outputs.installation-id }}');
  assert.equal(finalize.env.AERIS_WRITER_TOKEN_APP_SLUG, '${{ steps.writer_token.outputs.app-slug }}');
  assert.equal(finalize.env.AERIS_WRITER_TOKEN, '${{ steps.writer_token.outputs.token }}');
  assert.equal(finalize.env.AERIS_FINALIZER_PROOF_LEVEL, 'full');
  assert.equal(finalize.env.AERIS_FINALIZER_MUTATE, 'true');
  assert.equal(finalize.env.AERIS_FINALIZER_RESPONSE_LOSS_CANARY, '${{ vars.AERIS_FINALIZER_RESPONSE_LOSS_CANARY }}');
  assert.doesNotMatch(JSON.stringify(finalize.env), /PRIVATE_KEY/);
  const summary = steps.find((step) => /Summarize finalization/.test(step.name));
  assert.equal(summary.env.CANARY_MARKER, '${{ steps.finalize.outputs.canary_marker }}');
  assert.match(summary.run, /Canary/);
});

test('Finalizer never checks out the pull request head and pins every action', () => {
  const document = workflow();
  for (const job of Object.values(document.jobs)) {
    const checkout = job.steps.find((step) => String(step.uses).startsWith('actions/checkout@'));
    assert.equal(checkout.with.ref, '${{ github.sha }}');
    assert.equal(checkout.with['persist-credentials'], false);
    for (const step of job.steps) {
      if (step.uses) assert.match(step.uses, /^[^@\s]+@[0-9a-f]{40}$/);
    }
  }
});
