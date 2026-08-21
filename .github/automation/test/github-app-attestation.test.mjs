import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import test from 'node:test';

import {
  GitHubAppAttestationClient,
  GitHubAppAttestationError,
  attestGitHubApp,
  createGitHubAppJwt,
  runGitHubAppAttestation,
  validateGitHubAppAttestation,
} from '../src/github-app-attestation.mjs';

const expected = Object.freeze({
  owner_login: 'JinPengGeng',
  app_id: 4667256,
  app_slug: 'aeris-token-writer',
  installation_id: 155342531,
});

function app(overrides = {}) {
  return {
    id: expected.app_id,
    slug: expected.app_slug,
    owner: { login: expected.owner_login, type: 'User' },
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
    app_owner_login: expected.owner_login,
    app_owner_type: 'User',
    installation_id: expected.installation_id,
    installation_account_login: expected.owner_login,
    installation_account_type: 'User',
    repository_selection: 'selected',
  });
  assert.deepEqual(calls, ['app', `installation:${expected.installation_id}`]);
  assert.equal(Object.isFrozen(result), true);
});

test('App JWT attestation fails closed on wrong or missing live identity fields', () => {
  const cases = [
    [app({ id: expected.app_id + 1 }), installation()],
    [app({ id: undefined }), installation()],
    [app({ slug: 'other-writer' }), installation()],
    [app({ owner: { login: 'other-owner', type: 'User' } }), installation()],
    [app({ owner: null }), installation()],
    [app(), installation({ id: expected.installation_id + 1 })],
    [app(), installation({ app_id: expected.app_id + 1 })],
    [app(), installation({ app_slug: 'other-writer' })],
    [app(), installation({ account: { login: 'other-owner', type: 'User' } })],
    [app(), installation({ account: null })],
    [app(), installation({ repository_selection: 'all' })],
    [app(), installation({ suspended_at: '2026-08-21T00:00:00Z' })],
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

test('attestation CLI derives owner from the repository and rejects missing configuration before HTTP', async () => {
  const environment = {
    GITHUB_REPOSITORY: 'JinPengGeng/aeris-token',
    AERIS_WRITER_APP_ID: String(expected.app_id),
    AERIS_WRITER_APP_SLUG: expected.app_slug,
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

  for (const key of ['GITHUB_REPOSITORY', 'AERIS_WRITER_APP_ID', 'AERIS_WRITER_APP_SLUG', 'AERIS_WRITER_INSTALLATION_ID']) {
    calls = 0;
    await assert.rejects(
      () => runGitHubAppAttestation({ ...environment, [key]: '' }, dependencies),
      GitHubAppAttestationError,
    );
    assert.equal(calls, 0);
  }
});
