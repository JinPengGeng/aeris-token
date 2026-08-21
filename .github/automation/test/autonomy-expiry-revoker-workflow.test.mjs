import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const yaml = createRequire(import.meta.url)('js-yaml');
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const workflowPath = path.join(root, '.github', 'workflows', 'autonomy-expiry-revoker.yml');

function workflow() { return yaml.load(fs.readFileSync(workflowPath, 'utf8')); }

test('expiry revoker is scheduled ahead of hard expiry and exposes safe manual controls', () => {
  const document = workflow();
  assert.deepEqual(document.on.schedule, [{ cron: '*/5 * * * *' }]);
  assert.equal(document.on.workflow_dispatch.inputs.dry_run.default, false);
  assert.equal(document.on.workflow_dispatch.inputs.force.default, false);
  assert.equal(document.concurrency.group, 'autonomy-expiry-revoker');
});

test('revoker limits GITHUB_TOKEN writes to post-uninstall PR cleanup and pins actions', () => {
  const document = workflow();
  assert.deepEqual(document.permissions, { contents: 'read', 'pull-requests': 'write' });
  const job = document.jobs.revoke;
  assert.equal(job.environment, 'writer');
  assert.deepEqual(job.concurrency, { group: 'aeris-writer-mutation', 'cancel-in-progress': false });
  assert.equal(job.env.AERIS_PRE_UNINSTALL_CLEANUP_BUDGET_SECONDS, '60');
  assert.equal(job.env.AERIS_POST_UNINSTALL_CLEANUP_BUDGET_SECONDS, '180');
  assert.doesNotMatch(job.if, /AERIS_WRITER|vars\./);
  assert.equal(job.env.AERIS_WRITER_APP_PRIVATE_KEY, undefined);
  for (const step of job.steps) {
    if (step.uses) assert.match(step.uses, /^[^@\s]+@[0-9a-f]{40}$/);
  }
  const checkout = job.steps.find((step) => String(step.uses).startsWith('actions/checkout@'));
  assert.equal(checkout.with.ref, '${{ github.sha }}');
  assert.equal(checkout.with['persist-credentials'], false);
  assert.match(JSON.stringify(job), /AERIS_WRITER_APP_PRIVATE_KEY/);
  const revoke = job.steps.find((step) => /Revoke expired Writer authority/.test(step.name));
  assert.match(revoke.env.AERIS_WRITER_APP_PRIVATE_KEY, /secrets\.AERIS_WRITER_APP_PRIVATE_KEY/);
  assert.equal(revoke.env.AERIS_REVOCATION_TOKEN, '${{ github.token }}');
  assert.equal(document.permissions.contents, 'read');
  assert.equal(document.permissions['pull-requests'], 'write');
});

test('all Writer mutation jobs share the revocation lock', () => {
  const cases = [
    ['autonomy-publisher.yml', 'publish'],
    ['autonomy-finalizer.yml', 'finalize'],
    ['sync-upstream.yml', 'sync'],
  ];
  for (const [filename, jobName] of cases) {
    const document = yaml.load(fs.readFileSync(path.join(root, '.github', 'workflows', filename), 'utf8'));
    assert.deepEqual(document.jobs[jobName].concurrency, {
      group: 'aeris-writer-mutation',
      'cancel-in-progress': false,
    });
  }
});

test('workflow keeps the Writer private key in the protected Environment', () => {
  const text = fs.readFileSync(workflowPath, 'utf8');
  assert.match(text, /environment: writer/);
  assert.match(text, /AERIS_WRITER_APP_PRIVATE_KEY/);
  assert.match(text, /GITHUB_TOKEN cannot delete them/);
});
