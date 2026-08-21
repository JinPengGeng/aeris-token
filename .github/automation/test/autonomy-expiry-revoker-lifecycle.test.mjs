import assert from 'node:assert/strict';
import { generateKeyPairSync } from 'node:crypto';
import test from 'node:test';
import { main, revokeAutonomy } from '../src/autonomy-expiry-revoker.mjs';

const config = Object.freeze({
  repository: 'JinPengGeng/aeris-token', repositoryId: 123, owner: 'JinPengGeng',
  defaultBranch: 'main', branchPrefix: 'agent/issue-', syncBranch: 'automation/sync-upstream',
  writerLogin: 'aeris-token-writer[bot]', appId: 456, appSlug: 'aeris-token-writer',
  installationId: 789, expiresAt: '2026-09-20T14:30:00Z',
});

const appIdentity = Object.freeze({
  id: config.appId,
  slug: config.appSlug,
  name: `Aeris Token Writer ${config.repositoryId}`,
  public: false,
  owner: { id: 36217715, login: config.owner, type: 'User' },
  external_url: `https://github.com/${config.repository}`,
  hook_attributes: { active: false, url: `https://github.com/${config.repository}` },
  permissions: { contents: 'write', metadata: 'read', pull_requests: 'write' },
  events: [],
});

const installationIdentity = () => ({
  id: config.installationId,
  app_id: config.appId,
  account: appIdentity.owner,
  repository_selection: 'selected',
  suspended_at: null,
});

function managedPull(number, overrides = {}) {
  return {
    state: 'open', number, node_id: `PR_${number}`,
    body: `<!-- aeris-autonomy-managed -->\n<!-- aeris-autonomy-task:issue:${number} -->`,
    user: { login: config.writerLogin },
    head: { ref: `agent/issue-${number}`, repo: { full_name: config.repository } },
    base: { ref: 'main', repo: { full_name: config.repository } },
    ...overrides,
  };
}

function governance(pull, armed) {
  const graphQlLogin = pull.user.login === 'github-actions[bot]'
    ? 'github-actions'
    : pull.user.login.replace(/\[bot\]$/, '');
  return {
    id: pull.node_id, number: pull.number, state: 'OPEN', body: pull.body,
    author: { __typename: 'Bot', login: graphQlLogin }, headRefName: pull.head.ref,
    headRepository: { nameWithOwner: pull.head.repo.full_name }, baseRefName: pull.base.ref,
    baseRepository: { nameWithOwner: pull.base.repo.full_name },
    autoMergeRequest: armed ? { enabledAt: '2026-09-20T12:00:00Z', mergeMethod: 'SQUASH' } : null,
  };
}

class FakeClient {
  constructor(pulls) {
    this.pulls = pulls;
    this.armed = new Set(pulls.map((pull) => pull.number));
    this.inventoryReads = 0;
    this.governanceReads = new Map();
    this.disableCalls = [];
    this.deleteCalls = 0;
    this.deleted = false;
    this.onDelete = null;
  }
  async getApp() { return appIdentity; }
  async listInstallations() { return this.deleted ? [] : [installationIdentity()]; }
  async getInstallationOrMissing() { return this.deleted ? null : installationIdentity(); }
  async getRepository() { return { id: config.repositoryId, full_name: config.repository }; }
  async listOpenPulls() { this.inventoryReads += 1; return [...this.pulls]; }
  async getPullGovernance(number) {
    this.governanceReads.set(number, (this.governanceReads.get(number) ?? 0) + 1);
    return governance(this.pulls.find((pull) => pull.number === number), this.armed.has(number));
  }
  async disablePullRequestAutoMerge(nodeId) {
    const number = Number(nodeId.slice(3));
    this.disableCalls.push(number);
    this.armed.delete(number);
  }
  async deleteInstallationOrMissing() {
    this.deleteCalls += 1;
    this.deleted = true;
    if (this.onDelete) await this.onDelete();
    return true;
  }
}

const noWait = async () => {};

test('all Writer-owned PRs are disarmed before and after uninstall', async () => {
  const client = new FakeClient([managedPull(41), managedPull(42)]);
  const result = await revokeAutonomy({
    appClient: client, governanceClient: client, config, force: true, settle: noWait,
  });
  assert.equal(result.action, 'uninstalled');
  assert.deepEqual(client.disableCalls, [41, 42]);
  assert.equal(client.armed.size, 0);
  assert.ok(client.inventoryReads >= 8);
  assert.equal(client.deleteCalls, 1);
});

test('post-uninstall cleanup catches a last-moment rearm and newly created Writer PR', async () => {
  const client = new FakeClient([managedPull(53)]);
  client.onDelete = async () => {
    client.armed.add(53);
    client.pulls.push(managedPull(54));
    client.armed.add(54);
  };
  const result = await revokeAutonomy({
    appClient: client, governanceClient: client, config, force: true, settle: noWait,
  });
  assert.equal(result.action, 'uninstalled');
  assert.deepEqual(result.pullNumbers, [53, 54]);
  assert.deepEqual(client.disableCalls, [53, 53, 54]);
  assert.equal(client.armed.size, 0);
});

test('external marker and branch spoofing cannot block uninstall', async () => {
  const external = managedPull(55, {
    body: '<!-- aeris-autonomy-managed -->\n<!-- upstream-sync-managed -->',
    user: { login: 'external-contributor' },
    head: { ref: config.syncBranch, repo: { full_name: 'external/fork' } },
  });
  const client = new FakeClient([external]);
  const result = await revokeAutonomy({
    appClient: client, governanceClient: client, config, force: true, settle: noWait,
  });
  assert.equal(result.action, 'uninstalled');
  assert.equal(result.managedPulls, 0);
  assert.deepEqual(client.disableCalls, []);
  assert.equal(client.deleteCalls, 1);
});

test('Writer-owned metadata drift remains in the disarm inventory', async () => {
  const drifted = managedPull(56, {
    body: 'markers removed',
    head: { ref: 'renamed', repo: { full_name: 'other/fork' } },
    base: { ref: 'staging', repo: { full_name: config.repository } },
  });
  const client = new FakeClient([drifted]);
  const result = await revokeAutonomy({
    appClient: client, governanceClient: client, config, force: true, settle: noWait,
  });
  assert.equal(result.action, 'uninstalled');
  assert.deepEqual(client.disableCalls, [56]);
});

test('legacy sync uses the distinct REST and GraphQL bot login forms', async () => {
  const legacy = managedPull(65, {
    body: '<!-- upstream-sync-managed -->',
    user: { login: 'github-actions[bot]' },
    head: { ref: config.syncBranch, repo: { full_name: config.repository } },
  });
  const client = new FakeClient([legacy]);
  const result = await revokeAutonomy({
    appClient: client, governanceClient: client, config, force: true, settle: noWait,
  });
  assert.equal(result.action, 'uninstalled');
  assert.deepEqual(client.disableCalls, [65]);
});

test('a transient pre-cleanup failure is recovered after uninstall', async () => {
  const client = new FakeClient([managedPull(57)]);
  const disable = client.disablePullRequestAutoMerge.bind(client);
  client.disablePullRequestAutoMerge = async (nodeId) => {
    if (!client.deleted) throw new Error('pre-cleanup failed');
    return disable(nodeId);
  };
  const result = await revokeAutonomy({
    appClient: client, governanceClient: client, config, force: true, settle: noWait,
  });
  assert.equal(result.action, 'uninstalled');
  assert.equal(client.deleted, true);
  assert.deepEqual(client.disableCalls, [57]);
});

test('persistent cleanup failure is reported after authority is revoked', async () => {
  const client = new FakeClient([managedPull(58)]);
  client.disablePullRequestAutoMerge = async () => { throw new Error('cleanup denied'); };
  await assert.rejects(
    () => revokeAutonomy({ appClient: client, governanceClient: client, config, force: true, settle: noWait }),
    /cleanup is incomplete/,
  );
  assert.equal(client.deleted, true);
  assert.equal(client.deleteCalls, 1);
});

test('pre-cleanup has a hard budget and cannot postpone uninstall to the job timeout', async () => {
  const client = new FakeClient([managedPull(68)]);
  let settleCalls = 0;
  const settle = async () => {
    settleCalls += 1;
    if (settleCalls === 1) return new Promise(() => {});
  };
  const result = await revokeAutonomy({
    appClient: client,
    governanceClient: client,
    config,
    force: true,
    settle,
    preCleanupBudgetMilliseconds: 5,
    postCleanupBudgetMilliseconds: 1_000,
  });
  assert.equal(result.action, 'uninstalled');
  assert.equal(client.deleteCalls, 1);
  assert.deepEqual(client.disableCalls, [68]);
});

test('post-cleanup runs when DELETE succeeds remotely but its response fails', async () => {
  const client = new FakeClient([managedPull(69)]);
  client.deleteInstallationOrMissing = async () => {
    client.deleteCalls += 1;
    client.deleted = true;
    client.armed.add(69);
    throw new Error('DELETE response timeout');
  };
  const result = await revokeAutonomy({
    appClient: client, governanceClient: client, config, force: true, settle: noWait,
  });
  assert.equal(result.action, 'uninstalled');
  assert.deepEqual(client.disableCalls, [69, 69]);
  assert.equal(client.armed.size, 0);
});

test('post-uninstall cleanup still runs when installation verification fails', async () => {
  const client = new FakeClient([managedPull(66)]);
  client.getInstallationOrMissing = async () => {
    if (client.deleted) throw new Error('verification unavailable');
    return installationIdentity();
  };
  await assert.rejects(
    () => revokeAutonomy({ appClient: client, governanceClient: client, config, force: true, settle: noWait }),
    /cleanup is incomplete/,
  );
  assert.equal(client.deleted, true);
  assert.deepEqual(client.disableCalls, [66]);
  assert.ok(client.inventoryReads >= 8);
});

test('moving paginated inventory cannot be reported as complete', async () => {
  const client = new FakeClient([managedPull(59)]);
  const second = managedPull(60);
  client.listOpenPulls = async () => {
    client.inventoryReads += 1;
    return client.inventoryReads % 2 === 1 ? [client.pulls[0]] : [client.pulls[0], second];
  };
  await assert.rejects(
    () => revokeAutonomy({ appClient: client, governanceClient: client, config, force: true, settle: noWait }),
    /cleanup is incomplete/,
  );
  assert.equal(client.deleted, true);
});

test('core App and repository identity failures precede mutation', async () => {
  const cases = [
    ['App identity', (client) => { client.getApp = async () => ({ id: 999, slug: config.appSlug }); }],
    ['revocation token', (client) => { client.getRepository = async () => ({ id: 999, full_name: config.repository }); }],
  ];
  for (const [message, mutate] of cases) {
    const client = new FakeClient([managedPull(61)]);
    mutate(client);
    await assert.rejects(
      () => revokeAutonomy({ appClient: client, governanceClient: client, config, force: true, settle: noWait }),
      new RegExp(message, 'i'),
    );
    assert.equal(client.disableCalls.length, 0);
    assert.equal(client.deleteCalls, 0);
  }
});

test('a stale installation ID never deletes the wrong account and rediscovers the target', async () => {
  const client = new FakeClient([]);
  client.getInstallationOrMissing = async () => client.deleted ? null : ({
    id: 999, app_id: config.appId, account: { login: 'other-owner', type: 'User' },
  });
  const result = await revokeAutonomy({
    appClient: client, governanceClient: client, config, force: true, settle: noWait,
  });
  assert.equal(result.action, 'uninstalled');
  assert.equal(client.deleteCalls, 1);
});

test('mutable App metadata, expanded permissions, and installation scope cannot block revocation', async () => {
  const renamedSlug = 'renamed-aeris-writer';
  const client = new FakeClient([managedPull(70, { user: { login: `${renamedSlug}[bot]` } })]);
  client.getApp = async () => ({
    ...appIdentity,
    slug: renamedSlug, name: 'renamed', public: true, external_url: 'https://example.invalid',
    owner: { id: 999, login: 'transferred-owner', type: 'Organization' },
    hook_attributes: { active: true, url: 'https://example.invalid' },
    permissions: { administration: 'write', contents: 'write', pull_requests: 'write' },
    events: ['push'],
  });
  client.getInstallationOrMissing = async () => client.deleted ? null : {
    ...installationIdentity(), repository_selection: 'all', suspended_at: '2026-08-20T00:00:00Z',
  };
  const result = await revokeAutonomy({
    appClient: client, governanceClient: client, config, force: true, settle: noWait,
  });
  assert.equal(result.action, 'uninstalled');
  assert.equal(client.deleteCalls, 1);
  assert.deepEqual(client.disableCalls, [70]);
});

function response(status, value = null) {
  return {
    ok: status >= 200 && status < 300,
    status,
    text: async () => value === null ? '' : JSON.stringify(value),
  };
}

function environment(privateKey) {
  return {
    GITHUB_REPOSITORY: config.repository,
    GITHUB_REPOSITORY_ID: String(config.repositoryId),
    GITHUB_API_URL: 'https://api.github.test',
    AERIS_DEFAULT_BRANCH: config.defaultBranch,
    AERIS_WRITER_APP_ID: String(config.appId),
    AERIS_WRITER_APP_SLUG: config.appSlug,
    AERIS_WRITER_INSTALLATION_ID: String(config.installationId),
    AERIS_WRITER_APP_PRIVATE_KEY: privateKey.export({ type: 'pkcs8', format: 'pem' }),
    AERIS_REVOCATION_TOKEN: 'github-token',
    AERIS_AUTONOMY_EXPIRES_AT: config.expiresAt,
    AERIS_FORCE: 'true',
    AERIS_AGENTS_ENABLED: 'false',
    AERIS_WRITER_ENABLED: 'false',
    AERIS_UPSTREAM_SYNC_ENABLED: 'false',
    AERIS_AUTONOMOUS_MERGE_ENABLED: 'false',
  };
}

test('main uses the native cleanup token and repeated cron runs are idempotent', async () => {
  const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  const env = environment(privateKey);
  let deleted = false;
  let deleteCalls = 0;
  const fetchImpl = async (url, options) => {
    const parsed = new URL(url);
    const path = parsed.pathname + parsed.search;
    const isGovernance = path === `/repos/${config.repository}` || path.startsWith(`/repos/${config.repository}/pulls?`) || path === '/graphql';
    assert.equal(options.headers.authorization === 'Bearer github-token', isGovernance);
    assert.doesNotMatch(path, /access_tokens|installation\/token/);
    if (path === '/app') return response(200, appIdentity);
    if (path === '/app/installations?per_page=100&page=1') {
      return response(200, deleted ? [] : [installationIdentity()]);
    }
    if (path === `/app/installations/${config.installationId}` && options.method === 'GET') {
      return deleted ? response(404, { message: 'Not Found' }) : response(200, installationIdentity());
    }
    if (path === `/repos/${config.repository}`) return response(200, { id: config.repositoryId, full_name: config.repository });
    if (path.startsWith(`/repos/${config.repository}/pulls?`)) return response(200, []);
    if (path === `/app/installations/${config.installationId}` && options.method === 'DELETE') {
      deleted = true;
      deleteCalls += 1;
      return response(204);
    }
    throw new Error(`unexpected request ${options.method} ${path}`);
  };
  const first = await main(env, { nowMs: Date.parse(config.expiresAt), fetchImpl, settle: noWait });
  const second = await main(env, { nowMs: Date.parse(config.expiresAt), fetchImpl, settle: noWait });
  assert.equal(first.action, 'uninstalled');
  assert.equal(second.action, 'already_uninstalled');
  assert.equal(deleteCalls, 1);
});

test('force refuses to race enabled production mutation lanes', async () => {
  const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  const env = environment(privateKey);
  env.AERIS_WRITER_ENABLED = 'true';
  await assert.rejects(() => main(env, { nowMs: Date.parse(config.expiresAt) }), /requires disabled production switches/);
});

test('dynamic installation discovery fails closed when target identity is not unique', async () => {
  const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  const env = environment(privateKey);
  delete env.AERIS_WRITER_INSTALLATION_ID;
  const fetchImpl = async (url) => {
    const path = new URL(url).pathname;
    if (path === '/app') return response(200, appIdentity);
    if (path === `/repos/${config.repository}`) return response(200, { id: config.repositoryId, full_name: config.repository });
    if (path === '/app/installations') {
      return response(200, [{ ...installationIdentity(), id: 1 }, { ...installationIdentity(), id: 2 }]);
    }
    throw new Error(`unexpected request ${path}`);
  };
  await assert.rejects(
    () => main(env, { nowMs: Date.parse(config.expiresAt), fetchImpl, settle: noWait }),
    /uniquely discovered/,
  );
});
