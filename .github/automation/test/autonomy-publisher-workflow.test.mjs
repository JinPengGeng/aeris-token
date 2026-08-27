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
const workflowPath = path.join(repoRoot, '.github', 'workflows', 'autonomy-publisher.yml');

function workflow() {
  return yaml.load(fs.readFileSync(workflowPath, 'utf8'));
}

function uses(document) {
  return Object.values(document.jobs).flatMap((job) => job.steps).map((step) => step.uses).filter(Boolean);
}

test('Publisher is driven only by completed Candidate workflow runs', () => {
  const document = workflow();
  assert.deepEqual(Object.keys(document.on), ['workflow_run']);
  assert.deepEqual(document.on.workflow_run.workflows, ['Agent candidate']);
  assert.deepEqual(document.on.workflow_run.types, ['completed']);
  assert.match(document.jobs.verify.if, /workflow_dispatch/);
  assert.match(document.jobs.verify.if, /default_branch/);
  assert.match(document.jobs.publish.if, /AERIS_WRITER_ENABLED/);
  assert.equal(document.jobs.publish.environment, 'writer');
});

test('Publisher verifies outside checkout twice before the only Writer token mint', () => {
  const document = workflow();
  const verifySerialized = JSON.stringify(document.jobs.verify);
  assert.doesNotMatch(verifySerialized, /AERIS_WRITER_APP_PRIVATE_KEY|AERIS_WRITER_TOKEN/);
  const steps = document.jobs.publish.steps;
  const mintIndex = steps.findIndex((step) => /Mint bounded Writer App token/.test(step.name));
  const verifyIndex = steps.findIndex((step) => /Verify rebound candidate/.test(step.name));
  assert.ok(verifyIndex >= 0 && verifyIndex < mintIndex);
  assert.equal(steps.filter((step) => /create-github-app-token@/.test(step.uses ?? '')).length, 1);
  const mint = steps[mintIndex];
  assert.deepEqual(
    Object.keys(mint.with).filter((key) => key.startsWith('permission-')).sort(),
    ['permission-checks', 'permission-contents', 'permission-pull-requests'],
  );
  assert.equal(mint.with['permission-checks'], 'write');
  for (const job of Object.values(document.jobs)) {
    for (const step of job.steps.filter((value) => /Download/.test(value.name))) {
      assert.match(step.with.path, /runner\.temp/);
    }
  }
});

test('Publisher actions are immutable and no job grants GitHub write permissions', () => {
  const document = workflow();
  for (const action of uses(document)) assert.match(action, /^[^@\s]+@[0-9a-f]{40}$/);
  assert.doesNotMatch(JSON.stringify(document.jobs.verify.permissions), /write/);
  assert.doesNotMatch(JSON.stringify(document.jobs.publish.permissions), /write/);
});

test('Publisher uploads exactly one short-lived, fixed-name finalizer target after publication', () => {
  const document = workflow();
  const steps = document.jobs.publish.steps;
  const publishIndex = steps.findIndex((step) => /Publish exact managed Draft PR/.test(step.name));
  const uploadIndex = steps.findIndex((step) => /Upload finalizer target/.test(step.name));
  const publish = steps[publishIndex];
  const upload = steps[uploadIndex];

  assert.ok(publishIndex >= 0 && uploadIndex > publishIndex);
  assert.equal(publish.env.AERIS_PUBLISHER_TARGET_PATH, '${{ runner.temp }}/aeris-publisher-target/publisher-target.json');
  assert.equal(upload.uses, 'actions/upload-artifact@330a01c490aca151604b8cf639adc76d48f6c5d4');
  assert.equal(upload.with.name, 'aeris-publisher-target-${{ github.run_id }}-${{ github.run_attempt }}');
  assert.equal(upload.with.path, '${{ runner.temp }}/aeris-publisher-target/publisher-target.json');
  assert.equal(upload.with['if-no-files-found'], 'error');
  assert.equal(upload.with['retention-days'], 1);
  assert.equal(upload.with.overwrite, false);
});
