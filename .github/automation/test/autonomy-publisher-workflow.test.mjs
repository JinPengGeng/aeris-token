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
    ['permission-contents', 'permission-pull-requests'],
  );
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
