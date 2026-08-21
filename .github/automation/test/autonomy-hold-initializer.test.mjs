import assert from 'node:assert/strict';
import test from 'node:test';

import {
  AutonomyHoldInitializerError, GITHUB_ACTIONS_APP_ID, GITHUB_ACTIONS_APP_SLUG,
  HOLD_CHECK_NAME, classifyPull, holdExternalId, initializeAutonomyHold,
} from '../src/autonomy-hold-initializer.mjs';

const SHA = 'a'.repeat(40);
const CONFIG = Object.freeze({ repository: 'JinPengGeng/aeris-token', repository_id: 1, pull_number: 7, default_branch: 'main', writer_login: 'aeris-token-writer[bot]' });

function repository() { return { id: 1, full_name: CONFIG.repository, default_branch: 'main' }; }
function pull(overrides = {}) {
  return { number: 7, state: 'open', body: '<!-- aeris-autonomy-managed -->\n<!-- aeris-autonomy-task:issue:7 -->',
    base: { ref: 'main', repo: { full_name: CONFIG.repository } }, head: { ref: 'agent/issue-7', sha: SHA, repo: { full_name: CONFIG.repository } },
    user: { type: 'Bot', login: CONFIG.writer_login }, ...overrides };
}
function check({ status = 'in_progress', conclusion = null, external_id = holdExternalId({ repository_id: 1, pull_number: 7, head_sha: SHA }), id = 11 } = {}) {
  return { id, name: HOLD_CHECK_NAME, head_sha: SHA, external_id, status, conclusion,
    app: { id: GITHUB_ACTIONS_APP_ID, slug: GITHUB_ACTIONS_APP_SLUG }, pull_requests: [{ number: 7 }] };
}
class FakeClient {
  constructor({ currentPull = pull(), checks = [], lostCreate = false, driftAfterMutation = false } = {}) {
    this.currentPull = currentPull; this.checks = checks; this.lostCreate = lostCreate; this.driftAfterMutation = driftAfterMutation;
    this.created = 0; this.completed = 0; this.pullReads = 0;
  }
  async getRepository() { return repository(); }
  async getPull() {
    this.pullReads += 1;
    if (this.driftAfterMutation && this.created + this.completed > 0 && this.pullReads > 2) return pull({ head: { ...this.currentPull.head, sha: 'b'.repeat(40) } });
    return structuredClone(this.currentPull);
  }
  async listChecks() { return structuredClone(this.checks); }
  async getCheck(id) { return structuredClone(this.checks.find((value) => value.id === id)); }
  async createCheck(_, externalId, desired) {
    this.created += 1; this.checks.push(check({ external_id: externalId, status: desired.conclusion === null ? 'in_progress' : 'completed', conclusion: desired.conclusion }));
    if (this.lostCreate) throw new Error('connection reset after create');
    return structuredClone(this.checks.at(-1));
  }
  async completeCheck(id, _, conclusion) { this.completed += 1; Object.assign(this.checks.find((value) => value.id === id), { status: 'completed', conclusion }); }
}

test('managed Writer Bot creates an exact pending hold', async () => {
  const client = new FakeClient();
  const result = await initializeAutonomyHold({ client, config: CONFIG });
  assert.equal(result.action, 'held'); assert.equal(result.managed, true); assert.equal(client.created, 1);
  assert.deepEqual(client.checks[0], check());
});

test('same login User is not a Writer App and receives a successful hold', async () => {
  const client = new FakeClient({ currentPull: pull({ user: { type: 'User', login: CONFIG.writer_login } }) });
  const result = await initializeAutonomyHold({ client, config: CONFIG });
  assert.equal(result.managed, false); assert.equal(result.action, 'released');
  assert.equal(client.checks[0].conclusion, 'success');
});

test('Writer branch with malformed marker receives a failure hold', async () => {
  const client = new FakeClient({ currentPull: pull({ body: '<!-- aeris-autonomy-managed -->' }) });
  const result = await initializeAutonomyHold({ client, config: CONFIG });
  assert.equal(result.action, 'released'); assert.equal(client.checks[0].conclusion, 'failure');
});

test('classification requires both Writer Bot identity and matching issue marker', () => {
  const normalized = {
    head_ref: 'agent/issue-7', head_repository: CONFIG.repository,
    user_type: 'Bot', user_login: CONFIG.writer_login,
    body: '<!-- aeris-autonomy-managed -->\n<!-- aeris-autonomy-task:issue:8 -->',
  };
  assert.equal(classifyPull(normalized, CONFIG).conclusion, 'failure');
  assert.equal(classifyPull({ ...normalized, user_login: 'other[bot]' }, CONFIG).conclusion, 'success');
  assert.equal(classifyPull({ ...normalized, head_repository: 'someone/fork' }, CONFIG).conclusion, 'success');
});

test('fork pull requests receive a successful non-managed hold', async () => {
  const client = new FakeClient({
    currentPull: pull({ head: { ref: 'feature', sha: SHA, repo: { full_name: 'someone/fork' } } }),
  });
  const result = await initializeAutonomyHold({ client, config: CONFIG });
  assert.equal(result.managed, false);
  assert.equal(result.action, 'released');
  assert.equal(client.checks[0].conclusion, 'success');
});

test('ordinary pull requests with no body receive a successful non-managed hold', async () => {
  const client = new FakeClient({
    currentPull: pull({ body: null, head: { ref: 'feature', sha: SHA, repo: { full_name: CONFIG.repository } }, user: { type: 'User', login: 'person' } }),
  });
  const result = await initializeAutonomyHold({ client, config: CONFIG });
  assert.equal(result.managed, false);
  assert.equal(result.action, 'released');
  assert.equal(client.checks[0].conclusion, 'success');
});

test('Writer App pull requests outside approved lanes receive a failure hold', async () => {
  const client = new FakeClient({ currentPull: pull({ head: { ref: 'writer-feature', sha: SHA, repo: { full_name: CONFIG.repository } } }) });
  const result = await initializeAutonomyHold({ client, config: CONFIG });
  assert.equal(result.managed, false);
  assert.equal(result.action, 'released');
  assert.equal(client.checks[0].conclusion, 'failure');
});

test('exact managed upstream synchronization pull requests receive a successful hold', async () => {
  const body = [
    '<!-- upstream-sync-managed -->',
    `<!-- upstream-sync-owned-tip:${SHA} -->`,
    `<!-- upstream-sync-source:fawney19/Aether@${'b'.repeat(40)} -->`,
  ].join('\n');
  const currentPull = pull({ body, head: { ref: 'automation/sync-upstream', sha: SHA, repo: { full_name: CONFIG.repository } } });
  const result = await initializeAutonomyHold({ client: new FakeClient({ currentPull }), config: CONFIG });
  assert.equal(result.managed, true);
  assert.equal(result.action, 'released');

  const malformed = pull({
    body: '<!-- upstream-sync-managed -->',
    head: { ref: 'automation/sync-upstream', sha: SHA, repo: { full_name: CONFIG.repository } },
  });
  const client = new FakeClient({ currentPull: malformed });
  const rejected = await initializeAutonomyHold({ client, config: CONFIG });
  assert.equal(rejected.managed, false);
  assert.equal(client.checks[0].conclusion, 'failure');
});

test('lost create response adopts one exact persisted check', async () => {
  const client = new FakeClient({ lostCreate: true });
  const result = await initializeAutonomyHold({ client, config: CONFIG });
  assert.equal(result.action, 'held'); assert.equal(client.created, 1);
});

test('duplicate exact and foreign exact-head hold checks fail closed', async () => {
  await assert.rejects(() => initializeAutonomyHold({ client: new FakeClient({ checks: [check(), check({ id: 12 })] }), config: CONFIG }), /duplicate exact/);
  await assert.rejects(() => initializeAutonomyHold({ client: new FakeClient({ checks: [check({ external_id: 'foreign' })] }), config: CONFIG }), /foreign hold/);
});

test('existing terminal hold is idempotent only for the requested terminal state', async () => {
  const client = new FakeClient({ currentPull: pull({ user: { type: 'User', login: 'person' } }), checks: [check({ status: 'completed', conclusion: 'success' })] });
  const result = await initializeAutonomyHold({ client, config: CONFIG });
  assert.equal(result.action, 'released'); assert.equal(client.created, 0);
  await assert.rejects(() => initializeAutonomyHold({ client: new FakeClient({ checks: [check({ status: 'completed', conclusion: 'success' })] }), config: CONFIG }), /completed hold/);
});

test('a managed pending hold never becomes successful after Writer identity drift', async (t) => {
  await t.test('configured Writer slug drift', async () => {
    const client = new FakeClient();
    await initializeAutonomyHold({ client, config: CONFIG });
    const driftedConfig = Object.freeze({ ...CONFIG, writer_login: 'replacement-writer[bot]' });

    await assert.rejects(
      () => initializeAutonomyHold({ client, config: driftedConfig }),
      /pending hold cannot be released as unmanaged/,
    );
    assert.equal(client.checks[0].status, 'in_progress');
    assert.equal(client.checks[0].conclusion, null);
    assert.equal(client.completed, 0);
  });

  await t.test('pull request author drift', async () => {
    const client = new FakeClient();
    await initializeAutonomyHold({ client, config: CONFIG });
    client.currentPull = pull({ user: { type: 'User', login: CONFIG.writer_login } });

    await assert.rejects(
      () => initializeAutonomyHold({ client, config: CONFIG }),
      /pending hold cannot be released as unmanaged/,
    );
    assert.equal(client.checks[0].status, 'in_progress');
    assert.equal(client.checks[0].conclusion, null);
    assert.equal(client.completed, 0);
  });
});

test('a managed pending hold becomes failure after its managed marker drifts', async () => {
  const client = new FakeClient();
  await initializeAutonomyHold({ client, config: CONFIG });
  client.currentPull = pull({ body: '<!-- aeris-autonomy-managed -->' });

  const result = await initializeAutonomyHold({ client, config: CONFIG });
  assert.equal(result.action, 'released');
  assert.equal(result.managed, false);
  assert.equal(client.checks[0].status, 'completed');
  assert.equal(client.checks[0].conclusion, 'failure');
  assert.equal(client.completed, 1);
});

test('a pull request changing after mutation fails closed', async () => {
  await assert.rejects(() => initializeAutonomyHold({ client: new FakeClient({ driftAfterMutation: true }), config: CONFIG }), /drifted after hold mutation/);
});

test('external ids bind repository, pull number, and exact head', () => {
  assert.equal(holdExternalId({ repository_id: 1, pull_number: 7, head_sha: SHA }), `aeris-finalizer-hold:v1:1:7:${SHA}`);
  assert.throws(() => holdExternalId({ repository_id: 1, pull_number: 7, head_sha: 'short' }), AutonomyHoldInitializerError);
});
