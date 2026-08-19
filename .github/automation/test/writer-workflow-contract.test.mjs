import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..', '..', '..');
const yaml = require(path.join(root, '.github', 'automation', 'node_modules', 'js-yaml'));
const source = fs.readFileSync(path.join(root, '.github', 'workflows', 'writer.yml'), 'utf8');
const workflow = yaml.load(source);
const jobs = workflow.jobs;
const serialized = (value) => JSON.stringify(value);

test('Writer workflow is trusted-main, four-stage, Issue-serialized, and globally read-only', () => {
  assert.deepEqual(Object.keys(jobs), ['preflight', 'analyze', 'build', 'publish']);
  assert.deepEqual(workflow.permissions, { contents: 'read' });
  assert.equal(workflow.concurrency['cancel-in-progress'], false);
  assert.match(workflow.concurrency.group, /event\.issue\.number/);
  assert.equal(source.includes('pull_request_target'), false);
  assert.match(String(jobs.preflight.if), /!github\.event\.issue\.pull_request/);
  for (const [name, job] of Object.entries(jobs)) {
    assert.equal(job.permissions.contents, 'read', `${name} contents permission`);
    assert.equal(Object.values(job.permissions).includes('write'), false, `${name} GITHUB_TOKEN write`);
    assert.ok(Number.isSafeInteger(job['timeout-minutes']) && job['timeout-minutes'] <= 20, `${name} hard deadline`);
    const checkout = job.steps.find((step) => String(step.uses ?? '').startsWith('actions/checkout@'));
    assert.equal(checkout.with.ref, '${{ github.event.repository.default_branch }}');
    assert.equal(checkout.with['persist-credentials'], false);
    assert.equal(checkout.with['fetch-depth'], 0);
  }
  assert.equal(jobs.publish.environment, 'writer');
});

test('Writer credentials are isolated to their exact live phases', () => {
  assert.equal(serialized(jobs.preflight).includes('AERIS_AI_API_KEY'), false);
  assert.equal(serialized(jobs.preflight).includes('AERIS_WRITER_PRIVATE_KEY'), false);
  assert.equal(serialized(jobs.build).includes('secrets.'), false);
  assert.equal(serialized(jobs.analyze).includes('AERIS_WRITER_PRIVATE_KEY'), false);
  assert.equal(serialized(jobs.publish).includes('AERIS_AI_API_KEY'), false);
  const appSecretSteps = jobs.publish.steps.filter((step) => serialized(step).includes('AERIS_WRITER_PRIVATE_KEY'));
  assert.equal(appSecretSteps.length, 1);
  assert.match(String(appSecretSteps[0].if), /state\.outputs\.state == 'ready'/);
  const aiSecretSteps = jobs.analyze.steps.filter((step) => serialized(step).includes('AERIS_AI_API_KEY'));
  assert.equal(aiSecretSteps.length, 1);
  assert.match(String(aiSecretSteps[0].if), /state\.outputs\.state == 'ready'/);
  assert.equal(source.includes('AERIS_WRITER_FIX_CYCLE'), false, 'caller-controlled retry budget input');
  assert.match(serialized(jobs.preflight), /AERIS_WRITER_APP_ID/);
  assert.match(serialized(jobs.preflight), /AERIS_WRITER_APP_SLUG/);
  assert.match(serialized(jobs.preflight), /AERIS_WRITER_PUBLIC_KEY/);
  assert.equal(serialized(jobs.preflight).includes('AERIS_WRITER_PRIVATE_KEY'), false);
});

test('Writer workflow keeps live switches and destructive operations outside the workflow contract', () => {
  assert.match(source, /AERIS_WRITER_ENABLED/);
  assert.match(source, /AERIS_WRITER_CANARY/);
  for (const forbidden of ['merge_exact_sha', 'enableAutoMerge', 'createReview', 'markReady', 'deleteRef', 'force: true']) {
    assert.equal(source.includes(forbidden), false, forbidden);
  }
  for (const phase of ['preflight', 'analyze', 'build', 'publish']) {
    assert.match(source, new RegExp(`run-writer-phase\\.mjs ${phase}`));
  }
});
