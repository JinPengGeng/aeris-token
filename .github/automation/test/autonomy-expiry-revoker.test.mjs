import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import {
  evaluateRevocationWindow,
  identifyManagedPull,
} from '../src/autonomy-expiry-revoker.mjs';

const require = createRequire(import.meta.url);
const yaml = require('js-yaml');
const testDirectory = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(testDirectory, '..', '..', '..');

test('revocation window becomes due 45 minutes before expiry', () => {
  const expiry = '2026-09-20T14:30:00Z';
  const expiryMs = Date.parse(expiry);
  assert.equal(evaluateRevocationWindow({ expiresAt: expiry, nowMs: expiryMs - 2_700_000 - 1 }).due, false);
  assert.equal(evaluateRevocationWindow({ expiresAt: expiry, nowMs: expiryMs - 2_700_000 }).due, true);
  assert.equal(evaluateRevocationWindow({ expiresAt: expiry, nowMs: expiryMs }).expired, true);
  assert.throws(() => evaluateRevocationWindow({ expiresAt: '2026-99-99T99:99:99Z' }));
});

test('managed sync ownership uses an unforgeable author and same-repository legacy branch', () => {
  const config = {
    repository: 'JinPengGeng/aeris-token', defaultBranch: 'main', branchPrefix: 'agent/issue-',
    syncBranch: 'automation/sync-upstream', writerLogin: 'aeris-token-writer[bot]',
  };
  const pull = {
    state: 'open', number: 18, node_id: 'PR_node_18', body: '<!-- upstream-sync-managed -->',
    user: { login: config.writerLogin },
    head: { ref: config.syncBranch, repo: { full_name: config.repository } },
    base: { ref: 'main', repo: { full_name: config.repository } },
  };
  assert.deepEqual(identifyManagedPull(pull, config), {
    number: 18, nodeId: 'PR_node_18', issue: null, sync: true,
    author: config.writerLogin, legacySync: false,
  });
  assert.deepEqual(identifyManagedPull({ ...pull, user: { login: 'github-actions[bot]' } }, config), {
    number: 18, nodeId: 'PR_node_18', issue: null, sync: true,
    author: 'github-actions[bot]', legacySync: true,
  });
  assert.equal(identifyManagedPull({ ...pull, user: { login: 'attacker' } }, config), null);
  assert.equal(identifyManagedPull({
    ...pull,
    user: { login: 'github-actions[bot]' },
    head: { ...pull.head, repo: { full_name: 'attacker/fork' } },
  }, config), null);
});

test('all Writer-authored PRs are owned even when forgeable metadata drifts', () => {
  const config = {
    repository: 'JinPengGeng/aeris-token',
    defaultBranch: 'main',
    branchPrefix: 'agent/issue-',
    syncBranch: 'automation/sync-upstream',
    writerLogin: 'aeris-token-writer[bot]',
  };
  const pull = {
    state: 'open', number: 17, node_id: 'PR_node_17',
    body: '<!-- aeris-autonomy-managed -->\n<!-- aeris-autonomy-task:issue:123 -->',
    user: { login: config.writerLogin },
    head: { ref: 'agent/issue-123', repo: { full_name: config.repository } },
    base: { ref: 'main', repo: { full_name: config.repository } },
  };
  assert.deepEqual(identifyManagedPull(pull, config), {
    number: 17, nodeId: 'PR_node_17', issue: '123', sync: false,
    author: config.writerLogin, legacySync: false,
  });
  const drifted = {
    ...pull,
    body: 'marker removed',
    head: { ref: 'unexpected-branch', repo: { full_name: 'attacker/other' } },
    base: { ref: 'unexpected-base', repo: { full_name: config.repository } },
  };
  assert.deepEqual(identifyManagedPull(drifted, config), {
    number: 17, nodeId: 'PR_node_17', issue: null, sync: false,
    author: config.writerLogin, legacySync: false,
  });
});

test('external PRs cannot enter the managed inventory with markers or branch names', () => {
  const config = {
    repository: 'JinPengGeng/aeris-token', defaultBranch: 'main', branchPrefix: 'agent/issue-',
    syncBranch: 'automation/sync-upstream', writerLogin: 'aeris-token-writer[bot]',
  };
  const external = {
    state: 'open', number: 99, node_id: 'PR_node_99',
    body: '<!-- aeris-autonomy-managed -->\n<!-- upstream-sync-managed -->\n<!-- aeris-autonomy-task:issue:99 -->',
    user: { login: 'external-contributor' },
    head: { ref: 'agent/issue-99', repo: { full_name: 'external/fork' } },
    base: { ref: 'main', repo: { full_name: config.repository } },
  };
  assert.equal(identifyManagedPull(external, config), null);
  assert.equal(identifyManagedPull({ ...external, head: { ...external.head, ref: config.syncBranch } }, config), null);
});

test('expiry workflow is scheduled, protected, and action-pinned', () => {
  const workflow = yaml.load(fs.readFileSync(path.join(repoRoot, '.github', 'workflows', 'autonomy-expiry-revoker.yml'), 'utf8'));
  assert.deepEqual(workflow.on.schedule, [{ cron: '*/5 * * * *' }]);
  assert.equal(workflow.jobs.revoke.environment, 'writer');
  assert.deepEqual(workflow.jobs.revoke.concurrency, { group: 'aeris-writer-mutation', 'cancel-in-progress': false });
  assert.match(workflow.jobs.revoke.steps[0].uses, /^[^@]+@[0-9a-f]{40}$/);
  const revokeStep = workflow.jobs.revoke.steps.find((step) => /Revoke expired Writer authority/.test(step.name));
  assert.match(revokeStep.run, /autonomy-expiry-revoker\.mjs/);
  assert.match(JSON.stringify(workflow.jobs.revoke), /AERIS_WRITER_APP_PRIVATE_KEY/);
  assert.deepEqual(workflow.permissions, { contents: 'read', 'pull-requests': 'write' });
});
