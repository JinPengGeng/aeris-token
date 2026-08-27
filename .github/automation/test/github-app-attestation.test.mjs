import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  GitHubAppAttestationClient,
  GitHubAppAttestationError,
  GitHubInstallationTokenProofClient,
  attestGitHubApp,
  createGitHubAppJwt,
  proveGitHubInstallationToken,
  runGitHubAppAttestation,
  runGitHubInstallationTokenProof,
  validateGitHubInstallationTokenProof,
  validateGitHubAppAttestation,
} from '../src/github-app-attestation.mjs';

const expected = Object.freeze({
  owner_login: 'JinPengGeng',
  owner_database_id: 11525733,
  app_id: 4667256,
  app_slug: 'aeris-token-writer',
  app_node_id: 'MDExOkludGVncmF0aW9uNDY2NzI1Ng==',
  installation_id: 155342531,
});

function app(overrides = {}) {
  return {
    id: expected.app_id,
    slug: expected.app_slug,
    node_id: expected.app_node_id,
    owner: { id: expected.owner_database_id, login: expected.owner_login, type: 'User' },
    permissions: { administration: 'read', checks: 'write', contents: 'write', pull_requests: 'write', metadata: 'read' },
    ...overrides,
  };
}

function installation(overrides = {}) {
  return {
    id: expected.installation_id,
    app_id: expected.app_id,
    app_slug: expected.app_slug,
    account: { login: expected.owner_login, type: 'User' },
    repository_selection: 'selected',
    suspended_at: null,
    permissions: { administration: 'read', checks: 'write', contents: 'write', pull_requests: 'write', metadata: 'read' },
    ...overrides,
  };
}

test('RS256 App JWT has GitHub bounded claims and verifies with the matching key', () => {
  const { privateKey, publicKey } = crypto.generateKeyPairSync('rsa', { modulusLength: 2048 });
  const nowMs = Date.parse('2026-08-21T08:00:00Z');
  const jwt = createGitHubAppJwt({
    appId: expected.app_id,
    privateKey: privateKey.export({ format: 'pem', type: 'pkcs8' }).toString(),
    nowMs,
  });
  const [header, payload, signature] = jwt.split('.');
  assert.deepEqual(JSON.parse(Buffer.from(header, 'base64url')), { alg: 'RS256', typ: 'JWT' });
  assert.deepEqual(JSON.parse(Buffer.from(payload, 'base64url')), {
    iat: Math.floor(nowMs / 1000) - 60,
    exp: Math.floor(nowMs / 1000) + 540,
    iss: String(expected.app_id),
  });
  assert.equal(
    crypto.verify('RSA-SHA256', Buffer.from(`${header}.${payload}`), publicKey, Buffer.from(signature, 'base64url')),
    true,
  );
  assert.throws(
    () => createGitHubAppJwt({ appId: expected.app_id, privateKey: 'not a key', nowMs }),
    GitHubAppAttestationError,
  );
});

test('App JWT attestation binds App, owner, selected installation, and account identity', async () => {
  const calls = [];
  const result = await attestGitHubApp({
    expected,
    client: {
      async getApp() { calls.push('app'); return app(); },
      async getInstallation(id) { calls.push(`installation:${id}`); return installation(); },
    },
  });
  assert.deepEqual(result, {
    app_id: expected.app_id,
    app_slug: expected.app_slug,
    app_node_id: expected.app_node_id,
    app_owner_login: expected.owner_login,
    app_owner_database_id: expected.owner_database_id,
    app_owner_type: 'User',
    installation_id: expected.installation_id,
    installation_account_login: expected.owner_login,
    installation_account_type: 'User',
    repository_selection: 'selected',
    app_permissions: { administration: 'read', checks: 'write', contents: 'write', metadata: 'read', pull_requests: 'write' },
    installation_permissions: { administration: 'read', checks: 'write', contents: 'write', metadata: 'read', pull_requests: 'write' },
  });
  assert.deepEqual(calls, ['app', `installation:${expected.installation_id}`]);
  assert.equal(Object.isFrozen(result), true);
});

test('App JWT attestation fails closed on wrong or missing live identity fields', () => {
  const cases = [
    [app({ id: expected.app_id + 1 }), installation()],
    [app({ id: undefined }), installation()],
    [app({ slug: 'other-writer' }), installation()],
    [app({ node_id: 'MDExOkludGVncmF0aW9uOTk=' }), installation()],
    [app({ node_id: '' }), installation()],
    [app({ owner: { login: 'other-owner', type: 'User' } }), installation()],
    [app({ owner: { id: expected.owner_database_id + 1, login: expected.owner_login, type: 'User' } }), installation()],
    [app({ owner: { id: 0, login: expected.owner_login, type: 'User' } }), installation()],
    [app({ owner: null }), installation()],
    [app(), installation({ id: expected.installation_id + 1 })],
    [app(), installation({ app_id: expected.app_id + 1 })],
    [app(), installation({ app_slug: 'other-writer' })],
    [app(), installation({ account: { login: 'other-owner', type: 'User' } })],
    [app(), installation({ account: null })],
    [app(), installation({ repository_selection: 'all' })],
    [app(), installation({ suspended_at: '2026-08-21T00:00:00Z' })],
    [app({ permissions: undefined }), installation()],
    [app({ permissions: { administration: 'read', contents: 'write' } }), installation()],
    [app({ permissions: { administration: 'read', checks: 'read', contents: 'write', pull_requests: 'write' } }), installation()],
    [app({ permissions: { administration: 'read', contents: false, pull_requests: 'write' } }), installation()],
    [app({ permissions: { administration: 'write', contents: 'write', pull_requests: 'write' } }), installation()],
    [app({ permissions: { administration: 'read', contents: 'write', pull_requests: 'write', metadata: 'write' } }), installation()],
    [app({ permissions: { administration: 'read', contents: 'write', pull_requests: 'write', issues: 'read' } }), installation()],
    [app(), installation({ permissions: undefined })],
    [app(), installation({ permissions: { administration: 'read', contents: 'write' } })],
    [app(), installation({ permissions: { administration: 'read', checks: 'read', contents: 'write', pull_requests: 'write' } })],
    [app(), installation({ permissions: { administration: 'read', contents: 'write', pull_requests: null } })],
    [app(), installation({ permissions: { administration: 'write', contents: 'write', pull_requests: 'write' } })],
    [app(), installation({ permissions: { administration: 'read', contents: 'write', pull_requests: 'write', metadata: 'write' } })],
    [app(), installation({ permissions: { administration: 'read', contents: 'write', pull_requests: 'write', issues: 'read' } })],
  ];
  for (const [liveApp, liveInstallation] of cases) {
    assert.throws(
      () => validateGitHubAppAttestation({ app: liveApp, installation: liveInstallation, expected }),
      GitHubAppAttestationError,
    );
  }
});

test('attestation HTTP client uses only App JWT endpoints and real response shapes', async () => {
  const calls = [];
  const client = new GitHubAppAttestationClient({
    jwt: 'header.payload.signature',
    fetchImpl: async (url, options) => {
      calls.push({ url, options });
      const body = url.endsWith('/app') ? app() : installation();
      return new Response(JSON.stringify(body), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      });
    },
  });
  const result = await attestGitHubApp({ client, expected });
  assert.equal(result.installation_id, expected.installation_id);
  assert.deepEqual(calls.map(({ url }) => new URL(url).pathname), [
    '/app',
    `/app/installations/${expected.installation_id}`,
  ]);
  for (const { options } of calls) {
    assert.equal(options.method, 'GET');
    assert.equal(options.headers.authorization, 'Bearer header.payload.signature');
    assert.equal(options.headers['x-github-api-version'], '2022-11-28');
  }
});

test('attestation HTTP failures, timeouts, invalid JSON, and body failures are fail closed', async (t) => {
  const cases = [
    ['HTTP error', async () => new Response('{"message":"no"}', { status: 401 })],
    ['invalid JSON', async () => new Response('not-json', { status: 200 })],
    ['body failure', async () => ({ ok: true, status: 200, async text() { throw new Error('body secret'); } })],
    ['network failure', async () => { throw new Error('network secret'); }],
    ['timeout', async (_url, { signal }) => new Promise((_resolve, rejectPromise) => {
      signal.addEventListener('abort', () => rejectPromise(new Error('timeout secret')), { once: true });
    })],
  ];
  for (const [name, fetchImpl] of cases) {
    await t.test(name, async () => {
      const client = new GitHubAppAttestationClient({ jwt: 'jwt', fetchImpl, timeoutMs: 5 });
      await assert.rejects(() => client.getApp(), (error) => {
        assert.equal(error instanceof GitHubAppAttestationError, true);
        assert.doesNotMatch(error.message, /secret|jwt/i);
        return true;
      });
    });
  }
});

test('attestation timeout remains active while reading the response body', async () => {
  const fetchImpl = async (_url, { signal }) => ({
    ok: true,
    status: 200,
    text: () => new Promise((_resolve, rejectPromise) => {
      signal.addEventListener('abort', () => rejectPromise(new Error('body timeout secret')), { once: true });
    }),
  });
  const client = new GitHubAppAttestationClient({ jwt: 'jwt', fetchImpl, timeoutMs: 20 });
  await assert.rejects(() => client.getApp(), (error) => {
    assert.equal(error instanceof GitHubAppAttestationError, true);
    assert.doesNotMatch(error.message, /secret|jwt/i);
    return true;
  });
});

test('attestation CLI derives owner from the repository and rejects missing configuration before HTTP', async () => {
  const environment = {
    GITHUB_REPOSITORY: 'JinPengGeng/aeris-token',
    AERIS_WRITER_APP_OWNER_DATABASE_ID: String(expected.owner_database_id),
    AERIS_WRITER_APP_ID: String(expected.app_id),
    AERIS_WRITER_APP_SLUG: expected.app_slug,
    AERIS_WRITER_APP_NODE_ID: expected.app_node_id,
    AERIS_WRITER_INSTALLATION_ID: String(expected.installation_id),
  };
  let calls = 0;
  const dependencies = {
    client: {
      async getApp() { calls += 1; return app(); },
      async getInstallation() { calls += 1; return installation(); },
    },
  };
  const result = await runGitHubAppAttestation(environment, dependencies);
  assert.equal(result.app_owner_login, expected.owner_login);
  assert.equal(calls, 2);

  const outputDirectory = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-app-attestation-'));
  const outputPath = path.join(outputDirectory, 'github-output');
  try {
    await runGitHubAppAttestation({ ...environment, GITHUB_OUTPUT: outputPath }, dependencies);
    assert.match(fs.readFileSync(outputPath, 'utf8'), new RegExp(`^app_node_id=${expected.app_node_id}$`, 'm'));
    assert.match(fs.readFileSync(outputPath, 'utf8'), new RegExp(`^app_owner_database_id=${expected.owner_database_id}$`, 'm'));
  } finally {
    fs.rmSync(outputDirectory, { force: true, recursive: true });
  }

  for (const key of ['GITHUB_REPOSITORY', 'AERIS_WRITER_APP_OWNER_DATABASE_ID', 'AERIS_WRITER_APP_ID', 'AERIS_WRITER_APP_SLUG', 'AERIS_WRITER_APP_NODE_ID', 'AERIS_WRITER_INSTALLATION_ID']) {
    calls = 0;
    await assert.rejects(
      () => runGitHubAppAttestation({ ...environment, [key]: '' }, dependencies),
      GitHubAppAttestationError,
    );
    assert.equal(calls, 0);
  }
});

test('read-only installation token proof binds exact repository and REST/GraphQL Bot identity', async () => {
  const tokenExpected = {
    repository: 'JinPengGeng/aeris-token', repository_id: 1316750512,
    app_slug: 'aeris-token-writer', installation_id: expected.installation_id,
  };
  const repositories = {
    total_count: 1,
    repositories: [{ id: 1316750512, full_name: tokenExpected.repository, owner: { login: 'JinPengGeng' } }],
  };
  const bot = { login: 'aeris-token-writer[bot]', type: 'Bot', site_admin: false, id: 319277066, node_id: 'BOT_writer_node' };
  const graphQlBot = { __typename: 'Bot', login: 'aeris-token-writer', databaseId: 319277066, id: 'BOT_writer_node' };
  const calls = [];
  const client = {
    async getInstallationRepositories() { calls.push('repositories'); return repositories; },
    async getBot(login) { calls.push(`bot:${login}`); return bot; },
    async getGraphQlBot(nodeId) { calls.push(`graphql:${nodeId}`); return graphQlBot; },
  };
  const result = await proveGitHubInstallationToken({ client, expected: tokenExpected });
  assert.equal(result.repository, tokenExpected.repository);
  assert.deepEqual(calls.sort(), ['bot:aeris-token-writer[bot]', 'graphql:BOT_writer_node', 'repositories']);
  for (const value of [
    { repositories: { ...repositories, total_count: 2 } },
    { repositories: { ...repositories, repositories: [{ ...repositories.repositories[0], id: 99 }] } },
    { bot: { ...bot, type: 'User' } },
    { graphQlBot: { ...graphQlBot, databaseId: 99 } },
  ]) {
    assert.throws(() => validateGitHubInstallationTokenProof({
      repositories: value.repositories ?? repositories,
      bot: value.bot ?? bot,
      graphQlBot: value.graphQlBot ?? graphQlBot,
      expected: tokenExpected,
    }), GitHubAppAttestationError);
  }
});

test('read-only token proof HTTP client performs only bounded reads plus a GraphQL query', async () => {
  const calls = [];
  const client = new GitHubInstallationTokenProofClient({
    token: 'writer-token',
    fetchImpl: async (url, options) => {
      calls.push({ url, options });
      if (url.endsWith('/installation/repositories?per_page=2&page=1')) {
        return new Response(JSON.stringify({ total_count: 1, repositories: [{ id: 1316750512, full_name: 'JinPengGeng/aeris-token', owner: { login: 'JinPengGeng' } }] }), { status: 200 });
      }
      if (url.endsWith('/users/aeris-token-writer%5Bbot%5D')) {
        return new Response(JSON.stringify({ login: 'aeris-token-writer[bot]', type: 'Bot', site_admin: false, id: 319277066, node_id: 'BOT_writer_node' }), { status: 200 });
      }
      return new Response(JSON.stringify({ data: { node: { __typename: 'Bot', login: 'aeris-token-writer', databaseId: 319277066, id: 'BOT_writer_node' } } }), { status: 200 });
    },
  });
  const result = await proveGitHubInstallationToken({ client, expected: {
    repository: 'JinPengGeng/aeris-token', repository_id: 1316750512,
    app_slug: 'aeris-token-writer', installation_id: expected.installation_id,
  } });
  assert.equal(result.graphql_login, 'aeris-token-writer');
  assert.deepEqual(calls.map(({ url }) => new URL(url).pathname), [
    '/installation/repositories', '/users/aeris-token-writer%5Bbot%5D', '/graphql',
  ]);
  assert.deepEqual(calls.map(({ options }) => options.method), ['GET', 'GET', 'POST']);
  assert.match(calls[2].options.body, /query WriterTokenBot/);
  assert.match(calls[2].options.body, /"variables":\{"id":"BOT_writer_node"\}/);
  assert.doesNotMatch(calls[2].options.body, /mutation/i);
});

test('read-only token proof CLI rejects mismatched token metadata before any request', async () => {
  const environment = {
    GITHUB_REPOSITORY: 'JinPengGeng/aeris-token', GITHUB_REPOSITORY_ID: '1316750512',
    AERIS_WRITER_APP_SLUG: 'aeris-token-writer', AERIS_WRITER_INSTALLATION_ID: String(expected.installation_id),
    AERIS_WRITER_TOKEN: 'writer-token', AERIS_WRITER_TOKEN_APP_SLUG: 'other-writer',
    AERIS_WRITER_TOKEN_INSTALLATION_ID: String(expected.installation_id),
  };
  let calls = 0;
  await assert.rejects(() => runGitHubInstallationTokenProof(environment, {
    client: { async getInstallationRepositories() { calls += 1; } },
  }), GitHubAppAttestationError);
  assert.equal(calls, 0);
});
