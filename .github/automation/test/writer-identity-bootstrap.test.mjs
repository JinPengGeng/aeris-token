import assert from 'node:assert/strict';
import test from 'node:test';

import {
  doubleReadInstallationRepositoryInventory,
  doubleReadWriterIdentity,
  doubleReadWriterIdentityControlState,
  normalizeWriterIdentityBootstrapSnapshot,
  runWriterIdentityBootstrap,
  WriterIdentityBootstrapControlClient,
  WriterIdentityBootstrapError,
} from '../src/writer-identity-bootstrap.mjs';
import {
  GitHubAppAttestationClient,
  GitHubInstallationTokenProofClient,
} from '../src/github-app-attestation.mjs';

const nodeId = 'MDExOkludGVncmF0aW9uNDY2NzI1Ng==';
const permissions = Object.freeze({
  administration: 'read',
  checks: 'write',
  contents: 'write',
  pull_requests: 'write',
  metadata: 'read',
});
const nowMs = Date.parse('2026-08-27T10:00:00Z');
const workflowSha = '73696c6624b4191c98f92fa8e24877e365932703';

function app(overrides = {}) {
  return {
    id: 4667256,
    slug: 'aeris-token-writer',
    node_id: nodeId,
    owner: { id: 36217715, login: 'JinPengGeng', type: 'User' },
    permissions: { ...permissions },
    events: [],
    ...overrides,
  };
}

function installation(overrides = {}) {
  return {
    id: 155342531,
    app_id: 4667256,
    app_slug: 'aeris-token-writer',
    account: { id: 36217715, login: 'JinPengGeng', type: 'User' },
    target_id: 36217715,
    target_type: 'User',
    repository_selection: 'selected',
    permissions: { ...permissions },
    events: [],
    suspended_at: null,
    suspended_by: null,
    ...overrides,
  };
}

function environment(overrides = {}) {
  return {
    GITHUB_REPOSITORY: 'JinPengGeng/aeris-token',
    GITHUB_REPOSITORY_ID: '1316750512',
    GITHUB_REF: 'refs/heads/main',
    GITHUB_SHA: workflowSha,
    GITHUB_ACTOR: 'JinPengGeng',
    AERIS_DEFAULT_BRANCH: 'main',
    AERIS_WRITER_IDENTITY_BOOTSTRAP_ENABLED: 'true',
    AERIS_WRITER_GOVERNANCE_CANARY_ENABLED: 'true',
    AERIS_AGENTS_ENABLED: 'false',
    AERIS_CANDIDATE_AGENTS_ENABLED: 'false',
    AERIS_WRITER_ENABLED: 'false',
    AERIS_UPSTREAM_SYNC_ENABLED: 'false',
    AERIS_AUTONOMOUS_MERGE_ENABLED: 'false',
    AERIS_WRITER_APP_PRIVATE_KEY: 'private-key-present',
    AERIS_IDENTITY_BOOTSTRAP_TOKEN: 'bootstrap-token-present',
    ...overrides,
  };
}

function appClient(sequence = [app(), installation(), app(), installation()]) {
  let index = 0;
  return {
    async getApp() { return sequence[index++]; },
    async getInstallation(id) {
      assert.equal(id, 155342531);
      return sequence[index++];
    },
    async createReadOnlyInstallationInventoryToken(id) {
      assert.equal(id, 155342531);
      return {
        token: 'short-lived-installation-token',
        expires_at: '2026-08-27T11:00:00Z',
        permissions: { contents: 'read', metadata: 'read' },
        repository_selection: 'selected',
      };
    },
    reads() { return index; },
  };
}

function repository(overrides = {}) {
  return {
    id: 1316750512,
    name: 'aeris-token',
    full_name: 'JinPengGeng/aeris-token',
    default_branch: 'main',
    owner: { id: 36217715, login: 'JinPengGeng', type: 'User' },
    ...overrides,
  };
}

function inventory(overrides = {}) {
  return { total_count: 1, repositories: [repository()], ...overrides };
}

function inventoryClient(sequence = [inventory(), inventory()]) {
  let index = 0;
  return {
    async getCompleteInstallationRepositoryInventory() { return sequence[index++]; },
    reads() { return index; },
  };
}

function controlVariables(bootstrap = 'true') {
  return {
    AERIS_AGENTS_ENABLED: 'false',
    AERIS_CANDIDATE_AGENTS_ENABLED: 'false',
    AERIS_WRITER_ENABLED: 'false',
    AERIS_UPSTREAM_SYNC_ENABLED: 'false',
    AERIS_AUTONOMOUS_MERGE_ENABLED: 'false',
    AERIS_WRITER_IDENTITY_BOOTSTRAP_ENABLED: bootstrap,
    AERIS_WRITER_GOVERNANCE_CANARY_ENABLED: 'true',
  };
}

function controlClient() {
  const calls = [];
  return {
    calls,
    async doubleReadTrustedState({ expectedVariables, expectedIdentityVariables, trustedSha }) {
      calls.push([
        'guard',
        expectedVariables.AERIS_WRITER_IDENTITY_BOOTSTRAP_ENABLED,
        expectedIdentityVariables?.AERIS_WRITER_APP_NODE_ID ?? null,
        trustedSha,
      ]);
      return {
        snapshot: {
          default_branch_head_sha: trustedSha,
          control_token_expiration: null,
          identity_variables: expectedIdentityVariables ?? {},
        },
        sha256: 'a'.repeat(64),
      };
    },
    async upsertRepositoryVariable(name, value) { calls.push(['upsert', name, value]); },
    async assertRepositoryVariable(name, value) { calls.push(['verify', name, value]); },
    async updateExistingRepositoryVariable(name, value) { calls.push(['update', name, value]); },
  };
}

test('identity bootstrap double-reads before binding variables, disables itself, and verifies the final binding', async () => {
  const reader = appClient();
  const inventoryReader = inventoryClient();
  const control = controlClient();
  const result = await runWriterIdentityBootstrap(environment(), {
    appClient: reader,
    inventoryClientFactory(token) {
      assert.equal(token, 'short-lived-installation-token');
      return inventoryReader;
    },
    controlClient: control,
    nowMs,
  });
  assert.equal(reader.reads(), 4);
  assert.equal(inventoryReader.reads(), 2);
  assert.match(result.snapshot_sha256, /^[0-9a-f]{64}$/);
  assert.equal(result.installation_repository_count, 1);
  assert.equal(result.identity_variables_verified, true);
  assert.equal(control.calls.filter(([kind]) => kind === 'guard').length, 8);
  assert.deepEqual(control.calls.filter(([kind]) => kind !== 'guard'), [
    ['upsert', 'AERIS_WRITER_APP_NODE_ID', nodeId],
    ['verify', 'AERIS_WRITER_APP_NODE_ID', nodeId],
    ['upsert', 'AERIS_WRITER_APP_OWNER_DATABASE_ID', '36217715'],
    ['verify', 'AERIS_WRITER_APP_OWNER_DATABASE_ID', '36217715'],
    ['update', 'AERIS_WRITER_IDENTITY_BOOTSTRAP_ENABLED', 'false'],
  ]);
  assert.deepEqual(control.calls.at(-1), ['guard', 'false', nodeId, workflowSha]);
});

test('missing bootstrap credential and unsafe flags cause zero writes', async (t) => {
  const cases = [
    ['missing control credential', { AERIS_IDENTITY_BOOTSTRAP_TOKEN: '' }],
    ['missing private key', { AERIS_WRITER_APP_PRIVATE_KEY: '' }],
    ['bootstrap disabled', { AERIS_WRITER_IDENTITY_BOOTSTRAP_ENABLED: 'false' }],
    ['canary disabled', { AERIS_WRITER_GOVERNANCE_CANARY_ENABLED: 'false' }],
    ['Writer enabled', { AERIS_WRITER_ENABLED: 'true' }],
    ['agents ambiguously disabled', { AERIS_AGENTS_ENABLED: '' }],
    ['wrong actor', { GITHUB_ACTOR: 'another-admin' }],
    ['wrong branch', { GITHUB_REF: 'refs/heads/feature' }],
    ['missing immutable workflow SHA', { GITHUB_SHA: '' }],
  ];
  for (const [name, overrides] of cases) {
    await t.test(name, async () => {
      const control = controlClient();
      await assert.rejects(
        runWriterIdentityBootstrap(environment(overrides), {
          appClient: appClient(),
          inventoryClientFactory: () => inventoryClient(),
          controlClient: control,
          nowMs,
        }),
        WriterIdentityBootstrapError,
      );
      assert.deepEqual(control.calls, []);
    });
  }
});

test('snapshot drift causes zero writes', async () => {
  const reader = appClient([app(), installation(), app({ node_id: 'MDExOkludGVncmF0aW9uNDY2NzI1Nw==' }), installation()]);
  const control = controlClient();
  await assert.rejects(
    runWriterIdentityBootstrap(environment(), { appClient: reader, controlClient: control }),
    /snapshot drifted/,
  );
  assert.deepEqual(control.calls, []);
});

test('installation inventory is double-read and must be the exact trusted repository', async () => {
  const proof = await doubleReadInstallationRepositoryInventory(inventoryClient());
  assert.equal(proof.snapshot.total_count, 1);
  assert.equal(proof.snapshot.repository_id, 1316750512);

  await assert.rejects(
    doubleReadInstallationRepositoryInventory(inventoryClient([
      inventory(),
      inventory({ repositories: [repository({ id: 99 })] }),
    ])),
    WriterIdentityBootstrapError,
  );
  await assert.rejects(
    doubleReadInstallationRepositoryInventory(inventoryClient([
      { total_count: 2, repositories: [repository(), repository({ id: 99, name: 'other', full_name: 'JinPengGeng/other' })] },
    ])),
    /not complete and exact/,
  );
});

test('installation inventory client exhausts bounded pagination before returning', async () => {
  const pages = [];
  const client = new GitHubInstallationTokenProofClient({
    token: 'short-lived-installation-token',
    fetchImpl: async (url) => {
      const page = Number(new URL(url).searchParams.get('page'));
      pages.push(page);
      const count = page === 1 ? 100 : 1;
      return new Response(JSON.stringify({
        total_count: 101,
        repositories: Array.from({ length: count }, (_, index) => ({ id: (page - 1) * 100 + index + 1 })),
      }), { status: 200 });
    },
  });
  const result = await client.getCompleteInstallationRepositoryInventory();
  assert.equal(result.total_count, 101);
  assert.equal(result.repositories.length, 101);
  assert.deepEqual(pages, [1, 2]);
});

test('App JWT mints only the fixed read-only inventory token shape', async () => {
  const requests = [];
  const client = new GitHubAppAttestationClient({
    jwt: 'app-jwt',
    fetchImpl: async (url, options) => {
      requests.push({ url, options });
      return new Response(JSON.stringify({
        token: 'short-lived-installation-token',
        expires_at: '2026-08-27T11:00:00Z',
        permissions: { contents: 'read', metadata: 'read' },
        repository_selection: 'selected',
      }), { status: 201 });
    },
  });
  await client.createReadOnlyInstallationInventoryToken(155342531);
  assert.equal(requests.length, 1);
  assert.equal(new URL(requests[0].url).pathname, '/app/installations/155342531/access_tokens');
  assert.equal(requests[0].options.method, 'POST');
  assert.deepEqual(JSON.parse(requests[0].options.body), { permissions: { contents: 'read' } });
  assert.equal(requests[0].options.headers.authorization, 'Bearer app-jwt');
});

test('read-only inventory token rejects write or unrelated permissions before control writes', async () => {
  const reader = appClient();
  reader.createReadOnlyInstallationInventoryToken = async () => ({
    token: 'unsafe-installation-token',
    expires_at: '2026-08-27T11:00:00Z',
    permissions: { contents: 'read', checks: 'read' },
    repository_selection: 'selected',
  });
  const control = controlClient();
  await assert.rejects(
    runWriterIdentityBootstrap(environment(), {
      appClient: reader,
      inventoryClientFactory: () => inventoryClient(),
      controlClient: control,
      nowMs,
    }),
    /permissions are not exact/,
  );
  assert.deepEqual(control.calls, []);
});

test('control state canonical double-read binds owner, repository, seven flags, and immutable main head', async () => {
  const variables = controlVariables();
  const calls = [];
  const client = {
    async getAuthenticatedUser() {
      calls.push('/user');
      return { user: { id: 36217715, login: 'JinPengGeng', type: 'User', site_admin: false }, token_expiration: null };
    },
    async getRepository() { calls.push('/repo'); return repository(); },
    async getDefaultBranchRef() {
      calls.push('/ref');
      return { ref: 'refs/heads/main', object: { type: 'commit', sha: workflowSha } };
    },
    async getRepositoryVariable(name) {
      calls.push(`/variable/${name}`);
      return { name, value: variables[name] };
    },
  };
  const proof = await doubleReadWriterIdentityControlState(client, {
    expectedVariables: variables,
    trustedSha: workflowSha,
    nowMs,
  });
  assert.match(proof.sha256, /^[0-9a-f]{64}$/);
  assert.equal(calls.filter((value) => value === '/user').length, 2);
  assert.equal(calls.filter((value) => value.startsWith('/variable/')).length, 14);
});

test('main head, flag, control identity, or repository drift fails before any guarded action', async (t) => {
  for (const kind of ['head', 'flag', 'user', 'repository']) {
    await t.test(kind, async () => {
      let refReads = 0;
      const variables = controlVariables();
      const client = {
        async getAuthenticatedUser() {
          return {
            user: {
              id: kind === 'user' ? 99 : 36217715,
              login: 'JinPengGeng',
              type: 'User',
              site_admin: false,
            },
            token_expiration: null,
          };
        },
        async getRepository() { return repository(kind === 'repository' ? { id: 99 } : {}); },
        async getDefaultBranchRef() {
          refReads += 1;
          return {
            ref: 'refs/heads/main',
            object: { type: 'commit', sha: kind === 'head' && refReads === 2 ? 'f'.repeat(40) : workflowSha },
          };
        },
        async getRepositoryVariable(name) {
          return { name, value: kind === 'flag' && name === 'AERIS_WRITER_ENABLED' ? 'true' : variables[name] };
        },
      };
      await assert.rejects(
        doubleReadWriterIdentityControlState(client, {
          expectedVariables: variables,
          trustedSha: workflowSha,
          nowMs,
        }),
        WriterIdentityBootstrapError,
      );
    });
  }
});

test('control-state drift after one write causes zero further writes', async () => {
  const control = controlClient();
  let guards = 0;
  control.doubleReadTrustedState = async ({ expectedVariables, trustedSha }) => {
    guards += 1;
    control.calls.push(['guard', expectedVariables.AERIS_WRITER_IDENTITY_BOOTSTRAP_ENABLED, trustedSha]);
    if (guards === 4) throw new WriterIdentityBootstrapError('simulated live control drift');
    return {
      snapshot: { default_branch_head_sha: trustedSha, control_token_expiration: null },
      sha256: 'a'.repeat(64),
    };
  };
  await assert.rejects(
    runWriterIdentityBootstrap(environment(), {
      appClient: appClient(),
      inventoryClientFactory: () => inventoryClient(),
      controlClient: control,
      nowMs,
    }),
    /simulated live control drift/,
  );
  assert.deepEqual(control.calls.filter(([kind]) => kind !== 'guard'), [
    ['upsert', 'AERIS_WRITER_APP_NODE_ID', nodeId],
    ['verify', 'AERIS_WRITER_APP_NODE_ID', nodeId],
  ]);
});

test('identity-variable drift after binding blocks bootstrap disable and success', async () => {
  const control = controlClient();
  let guards = 0;
  control.doubleReadTrustedState = async ({ expectedVariables, expectedIdentityVariables, trustedSha }) => {
    guards += 1;
    control.calls.push([
      'guard',
      expectedVariables.AERIS_WRITER_IDENTITY_BOOTSTRAP_ENABLED,
      expectedIdentityVariables?.AERIS_WRITER_APP_NODE_ID ?? null,
      trustedSha,
    ]);
    if (guards === 6) throw new WriterIdentityBootstrapError('simulated identity variable drift');
    return {
      snapshot: {
        default_branch_head_sha: trustedSha,
        control_token_expiration: null,
        identity_variables: expectedIdentityVariables ?? {},
      },
      sha256: 'a'.repeat(64),
    };
  };
  await assert.rejects(
    runWriterIdentityBootstrap(environment(), {
      appClient: appClient(),
      inventoryClientFactory: () => inventoryClient(),
      controlClient: control,
      nowMs,
    }),
    /simulated identity variable drift/,
  );
  assert.deepEqual(control.calls.filter(([kind]) => kind !== 'guard'), [
    ['upsert', 'AERIS_WRITER_APP_NODE_ID', nodeId],
    ['verify', 'AERIS_WRITER_APP_NODE_ID', nodeId],
    ['upsert', 'AERIS_WRITER_APP_OWNER_DATABASE_ID', '36217715'],
    ['verify', 'AERIS_WRITER_APP_OWNER_DATABASE_ID', '36217715'],
  ]);
});

test('strict identity snapshot rejects permissions, events, selection, suspension, and owner drift', () => {
  const invalid = [
    [app({ events: ['issues'] }), installation()],
    [app({ permissions: { ...permissions, issues: 'read' } }), installation()],
    [app(), installation({ events: ['pull_request'] })],
    [app(), installation({ repository_selection: 'all' })],
    [app(), installation({ suspended_at: '2026-08-27T00:00:00Z' })],
    [app(), installation({ account: { id: 99, login: 'JinPengGeng', type: 'User' } })],
    [
      app({ owner: { id: 99, login: 'JinPengGeng', type: 'User' } }),
      installation({
        account: { id: 99, login: 'JinPengGeng', type: 'User' },
        target_id: 99,
      }),
    ],
  ];
  for (const [liveApp, liveInstallation] of invalid) {
    assert.throws(
      () => normalizeWriterIdentityBootstrapSnapshot({ app: liveApp, installation: liveInstallation }),
      WriterIdentityBootstrapError,
    );
  }
});

test('double-read performs App then installation twice and returns only normalized proof', async () => {
  const order = [];
  const proof = await doubleReadWriterIdentity({
    async getApp() { order.push('/app'); return app({ extra_untrusted_field: 'ignored' }); },
    async getInstallation(id) { order.push(`/app/installations/${id}`); return installation(); },
  });
  assert.deepEqual(order, [
    '/app',
    '/app/installations/155342531',
    '/app',
    '/app/installations/155342531',
  ]);
  assert.deepEqual(Object.keys(proof), ['snapshot', 'canonical', 'sha256']);
  assert.doesNotMatch(proof.canonical, /extra_untrusted_field/);
  assert.match(proof.sha256, /^[0-9a-f]{64}$/);
});

test('control client uses only exact repository-variable endpoints', async () => {
  const requests = [];
  const responses = [
    [404, '{}'],
    [201, '{}'],
    [200, '{"name":"AERIS_WRITER_APP_OWNER_DATABASE_ID","value":"old"}'],
    [204, ''],
    [204, ''],
  ];
  const client = new WriterIdentityBootstrapControlClient({
    token: 'bootstrap-token',
    fetchImpl: async (url, options) => {
      requests.push({ url, options });
      const [status, body] = responses.shift();
      return { status, ok: status >= 200 && status < 300, async text() { return body; } };
    },
  });
  await client.upsertRepositoryVariable('AERIS_WRITER_APP_NODE_ID', nodeId);
  await client.upsertRepositoryVariable('AERIS_WRITER_APP_OWNER_DATABASE_ID', '36217715');
  await client.updateExistingRepositoryVariable('AERIS_WRITER_IDENTITY_BOOTSTRAP_ENABLED', 'false');

  assert.deepEqual(requests.map(({ url, options }) => [options.method, new URL(url).pathname]), [
    ['GET', '/repos/JinPengGeng/aeris-token/actions/variables/AERIS_WRITER_APP_NODE_ID'],
    ['POST', '/repos/JinPengGeng/aeris-token/actions/variables'],
    ['GET', '/repos/JinPengGeng/aeris-token/actions/variables/AERIS_WRITER_APP_OWNER_DATABASE_ID'],
    ['PATCH', '/repos/JinPengGeng/aeris-token/actions/variables/AERIS_WRITER_APP_OWNER_DATABASE_ID'],
    ['PATCH', '/repos/JinPengGeng/aeris-token/actions/variables/AERIS_WRITER_IDENTITY_BOOTSTRAP_ENABLED'],
  ]);
  for (const request of requests) {
    assert.equal(request.options.headers.authorization, 'Bearer bootstrap-token');
    assert.doesNotMatch(JSON.stringify(request.options.body ?? ''), /bootstrap-token/);
  }
  assert.equal(responses.length, 0);
});

test('control token is bound to the exact owner and rejects classic OAuth scopes', async () => {
  const client = new WriterIdentityBootstrapControlClient({
    token: 'bootstrap-token',
    fetchImpl: async () => new Response(JSON.stringify({
      id: 36217715,
      login: 'JinPengGeng',
      type: 'User',
      site_admin: false,
    }), {
      status: 200,
      headers: {
        'x-oauth-scopes': '',
        'github-authentication-token-expiration': '2026-09-03 10:00:00 UTC',
      },
    }),
  });
  const authenticated = await client.getAuthenticatedUser(nowMs);
  assert.equal(authenticated.user.login, 'JinPengGeng');
  assert.equal(authenticated.token_expiration, '2026-09-03T10:00:00.000Z');

  const noExpirationHeader = new WriterIdentityBootstrapControlClient({
    token: 'bootstrap-token',
    fetchImpl: async () => new Response('{}', { status: 200, headers: { 'x-oauth-scopes': '' } }),
  });
  assert.equal((await noExpirationHeader.getAuthenticatedUser(nowMs)).token_expiration, null);

  const longLived = new WriterIdentityBootstrapControlClient({
    token: 'bootstrap-token',
    fetchImpl: async () => new Response('{}', {
      status: 200,
      headers: {
        'x-oauth-scopes': '',
        'github-authentication-token-expiration': '2026-09-03 10:00:01 UTC',
      },
    }),
  });
  await assert.rejects(() => longLived.getAuthenticatedUser(nowMs), WriterIdentityBootstrapError);

  const noScopesHeader = new WriterIdentityBootstrapControlClient({
    token: 'bootstrap-token',
    fetchImpl: async () => new Response('{}', { status: 200, headers: {} }),
  });
  assert.equal((await noScopesHeader.getAuthenticatedUser(nowMs)).token_expiration, null);

  for (const headers of [{ 'x-oauth-scopes': 'repo, workflow' }]) {
    const unsafe = new WriterIdentityBootstrapControlClient({
      token: 'bootstrap-token',
      fetchImpl: async () => new Response('{}', { status: 200, headers }),
    });
    await assert.rejects(() => unsafe.getAuthenticatedUser(nowMs), WriterIdentityBootstrapError);
  }
});

test('control client reads the exact repository, main ref, and named variable endpoints', async () => {
  const paths = [];
  const client = new WriterIdentityBootstrapControlClient({
    token: 'bootstrap-token',
    fetchImpl: async (url) => {
      const pathname = new URL(url).pathname;
      paths.push(pathname);
      const body = pathname.endsWith('/git/ref/heads/main')
        ? { ref: 'refs/heads/main', object: { type: 'commit', sha: workflowSha } }
        : pathname.includes('/actions/variables/')
          ? { name: 'AERIS_WRITER_ENABLED', value: 'false' }
          : repository();
      return new Response(JSON.stringify(body), { status: 200 });
    },
  });
  await client.getRepository();
  await client.getDefaultBranchRef();
  await client.getRepositoryVariable('AERIS_WRITER_ENABLED');
  assert.deepEqual(paths, [
    '/repos/JinPengGeng/aeris-token',
    '/repos/JinPengGeng/aeris-token/git/ref/heads/main',
    '/repos/JinPengGeng/aeris-token/actions/variables/AERIS_WRITER_ENABLED',
  ]);
});

test('control client exposes no workflow-dispatch capability', () => {
  const client = new WriterIdentityBootstrapControlClient({
    token: 'bootstrap-token',
    fetchImpl: async () => { throw new Error('unexpected'); },
  });
  assert.equal(typeof client.dispatchWorkflow, 'undefined');
});
