import assert from 'node:assert/strict';
import test from 'node:test';

import { WriterGitHubApiError, WriterGitHubClient } from '../src/writer-github-client.mjs';
import { evaluateWriterLifecycle, WRITER_OWNERSHIP_MARKER } from '../src/writer-lifecycle.mjs';

const repository = 'example/repository';
const repositoryId = 123;
const writerApp = Object.freeze({ id: 456, slug: 'aeris-writer' });
const appJwt = 'writer-app.jwt.signature';
const installationToken = 'writer-test-installation-token';
const installationId = 789;
const createAttemptId = '12345678-1234-4123-8123-123456789abc';
const createAttemptMarker = `<!-- aeris-writer-create-attempt:${createAttemptId} -->`;
const oldSha = 'a'.repeat(40);
const newSha = 'b'.repeat(40);

const response = (payload, status = 200) => new Response(payload === null ? null : JSON.stringify(payload), {
  status,
  headers: { 'content-type': 'application/json' },
});

const textResponse = (payload, status = 200) => new Response(payload, {
  status,
  headers: { 'content-type': 'application/json' },
});

function client(fetchImpl, overrides = {}) {
  return new WriterGitHubClient({
    appJwt,
    repository,
    repositoryId,
    writerApp,
    fetchImpl,
    randomUUIDImpl: () => createAttemptId,
    ...overrides,
  });
}

function installation(overrides = {}) {
  return {
    id: installationId,
    app_id: writerApp.id,
    app_slug: writerApp.slug,
    repository_selection: 'selected',
    account: { login: 'example', type: 'Organization' },
    permissions: { contents: 'write', pull_requests: 'write', metadata: 'read' },
    ...overrides,
  };
}

function installationAccess(overrides = {}) {
  return {
    token: installationToken,
    expires_at: new Date(Date.now() + (60 * 60 * 1_000)).toISOString(),
    repository_selection: 'selected',
    permissions: { contents: 'write', pull_requests: 'write', metadata: 'read' },
    ...overrides,
  };
}

function createdBody(value = 'Details') {
  return value.length === 0
    ? `${createAttemptMarker}\n\n${WRITER_OWNERSHIP_MARKER}`
    : `${value}\n\n${createAttemptMarker}\n\n${WRITER_OWNERSHIP_MARKER}`;
}

function installationRepositories(overrides = {}) {
  return {
    repository_selection: 'selected',
    total_count: 1,
    repositories: [{ id: repositoryId, full_name: repository }],
    ...overrides,
  };
}

function withValidInstallation(fetchImpl) {
  return async (url, init) => {
    if (url === 'https://api.github.com/app') return response(writerApp);
    if (url === 'https://api.github.com/repos/example/repository/installation') return response(installation());
    if (url === `https://api.github.com/app/installations/${installationId}/access_tokens`) {
      return response(installationAccess(), 201);
    }
    if (url.includes('/installation/repositories?')) return response(installationRepositories());
    return fetchImpl(url, init);
  };
}

async function verifiedClient(fetchImpl, overrides = {}) {
  const api = client(withValidInstallation(fetchImpl), overrides);
  await api.verifyInstallationIdentity();
  return api;
}

function rawPull(overrides = {}) {
  return {
    number: 42,
    state: 'open',
    merged: false,
    draft: true,
    title: 'Fix issue 7',
    body: createdBody(),
    user: { id: 999, login: 'aeris-writer[bot]', type: 'Bot' },
    performed_via_github_app: { id: writerApp.id, slug: writerApp.slug },
    base: { ref: 'main', repo: { id: repositoryId, full_name: repository } },
    head: { ref: 'agent/issue-7', sha: oldSha, repo: { id: repositoryId, full_name: repository } },
    ...overrides,
  };
}

function rawRef(issueNumber, sha) {
  return { ref: `refs/heads/agent/issue-${issueNumber}`, object: { type: 'commit', sha } };
}

const ambiguousSuccessBodies = [
  {
    name: 'invalid JSON',
    make: (_state, status) => textResponse('not-json', status),
  },
  {
    name: 'truncated JSON',
    make: (_state, status) => textResponse('{"result":', status),
  },
  {
    name: 'stream failure',
    make: (_state, status) => new Response(new ReadableStream({
      start(controller) {
        controller.enqueue(new TextEncoder().encode('{"result":'));
        controller.error(new Error('socket reset'));
      },
    }), { status }),
  },
  {
    name: 'oversize body',
    make: (state, status) => new Response(new ReadableStream({
      start(controller) {
        controller.enqueue(new Uint8Array(1_048_577));
      },
      cancel() {
        state.bodyCanceled = true;
      },
    }), { status }),
  },
  {
    name: 'body timeout',
    make: (state, status) => new Response(new ReadableStream({
      start(controller) {
        controller.enqueue(new TextEncoder().encode('{"result":'));
      },
      cancel() {
        state.bodyCanceled = true;
      },
    }), { status }),
  },
];

test('Writer reads are pinned to GitHub.com, reject redirects, retain tombstones, and normalize App authors', async () => {
  const calls = [];
  const api = await verifiedClient(async (url, init) => {
    calls.push({ url, init });
    if (url.includes('/permission')) return response({ permission: 'write' });
    if (url.includes('/pulls?')) return response([rawPull({ state: 'closed', merged_at: null })]);
    if (url.includes('/pulls/')) return response(rawPull());
    return response({ id: 7 });
  });

  assert.deepEqual(await api.getRepository(), { id: 7 });
  assert.deepEqual(await api.getIssue(7), { id: 7 });
  assert.deepEqual(await api.getIssueComment(91), { id: 7 });
  assert.equal(await api.getCollaboratorPermission('writer-user'), 'write');
  assert.deepEqual((await api.getPull(42)).author, { type: 'App', id: writerApp.id });
  assert.deepEqual(await api.getRef('agent/issue-7'), { id: 7 });
  assert.equal((await api.listPullsForHead('agent/issue-7'))[0].state, 'closed');

  assert.deepEqual(calls.map(({ url, init }) => [url, init.method, init.redirect]), [
    ['https://api.github.com/repos/example/repository', 'GET', 'error'],
    ['https://api.github.com/repos/example/repository/issues/7', 'GET', 'error'],
    ['https://api.github.com/repos/example/repository/issues/comments/91', 'GET', 'error'],
    ['https://api.github.com/repos/example/repository/collaborators/writer-user/permission', 'GET', 'error'],
    ['https://api.github.com/repos/example/repository/pulls/42', 'GET', 'error'],
    ['https://api.github.com/repos/example/repository/git/ref/heads/agent%2Fissue-7', 'GET', 'error'],
    ['https://api.github.com/repos/example/repository/pulls?state=all&head=example%3Aagent%2Fissue-7&per_page=100&page=1', 'GET', 'error'],
  ]);
  assert.equal(calls.every(({ init }) => init.headers.authorization === `Bearer ${installationToken}`), true);

  const redirecting = await verifiedClient(async () => response({ location: 'https://attacker.invalid' }, 302));
  await assert.rejects(() => redirecting.getIssue(7), /HTTP 302/);
});

test('state=all list fixtures normalize merged_at for lifecycle tombstones', async () => {
  const fixtures = [
    rawPull({ state: 'closed', merged: undefined, merged_at: '2026-08-18T01:02:03Z' }),
    rawPull({
      number: 43,
      state: 'closed',
      merged: undefined,
      merged_at: null,
      base: { ref: 'release', repo: { id: repositoryId, full_name: repository } },
    }),
    rawPull({ number: 44, state: 'open', merged: undefined, merged_at: null }),
  ];
  const api = await verifiedClient(async () => response(fixtures));
  const pulls = await api.listPullsForHead('agent/issue-7');
  assert.deepEqual(pulls.map(({ merged }) => merged), [true, false, false]);
  assert.equal(evaluateWriterLifecycle({
    command: '/agent implement',
    issueNumber: 7,
    repositoryId,
    writerApp,
    branch: { ref: 'agent/issue-7', exists: false, headSha: null },
    pullRequests: [pulls[0]],
  }).reason, 'issue_tombstoned');
  assert.equal(evaluateWriterLifecycle({
    command: '/agent implement',
    issueNumber: 7,
    repositoryId,
    writerApp,
    branch: { ref: 'agent/issue-7', exists: false, headSha: null },
    pullRequests: [pulls[1]],
  }).reason, 'managed_pr_base_not_main');
});

test('merged_at normalization fails closed for invalid or open-merged list fixtures', async () => {
  const fixtures = [
    rawPull({ merged: undefined, merged_at: '2026-08-18T01:02:03Z' }),
    rawPull({ state: 'closed', merged: undefined, merged_at: 'not-a-timestamp' }),
    rawPull({ state: 'closed', merged: undefined, merged_at: '2026-02-31T01:02:03Z' }),
    rawPull({ state: 'closed', merged: undefined }),
  ];
  const api = await verifiedClient(async () => response(fixtures));
  const pulls = await api.listPullsForHead('agent/issue-7');
  assert.deepEqual(pulls.map(({ merged }) => merged), [null, null, null, null]);
});

test('raw Bot identity accepts a null performed App and overrides any forged lifecycle author field', async () => {
  const appCreated = rawPull({ performed_via_github_app: null });
  const valid = await verifiedClient(async () => response(appCreated));
  assert.deepEqual((await valid.getPull(42)).author, { type: 'App', id: writerApp.id });

  const appFieldOmitted = rawPull();
  delete appFieldOmitted.performed_via_github_app;
  const omitted = await verifiedClient(async () => response(appFieldOmitted));
  assert.deepEqual((await omitted.getPull(42)).author, { type: 'App', id: writerApp.id });

  const forged = rawPull({
    author: { type: 'App', id: writerApp.id },
    user: { login: 'attacker', type: 'User' },
    performed_via_github_app: null,
  });
  const api = await verifiedClient(async () => response(forged));
  assert.equal((await api.getPull(42)).author, null);
});

test('Writer verifies the installation App and exact selected repository before enabling writes', async () => {
  const identityCalls = [];
  const valid = client(async (url, init) => {
    identityCalls.push({ url, init });
    if (url === 'https://api.github.com/app') return response(writerApp);
    if (url === 'https://api.github.com/repos/example/repository/installation') return response(installation());
    if (url === `https://api.github.com/app/installations/${installationId}/access_tokens`) {
      return response(installationAccess(), 201);
    }
    if (url.includes('/installation/repositories?')) return response(installationRepositories());
    return response({ message: 'unexpected' }, 500);
  });
  assert.deepEqual(await valid.verifyInstallationIdentity(), {
    appId: writerApp.id,
    appSlug: writerApp.slug,
    installationId,
    repositoryId,
  });
  assert.deepEqual(identityCalls.map(({ url, init }) => ({
    method: init.method,
    path: new URL(url).pathname,
    authorization: init.headers.authorization,
    body: init.body === undefined ? undefined : JSON.parse(init.body),
  })), [
    { method: 'GET', path: '/app', authorization: `Bearer ${appJwt}`, body: undefined },
    {
      method: 'GET',
      path: '/repos/example/repository/installation',
      authorization: `Bearer ${appJwt}`,
      body: undefined,
    },
    {
      method: 'POST',
      path: `/app/installations/${installationId}/access_tokens`,
      authorization: `Bearer ${appJwt}`,
      body: { permissions: { contents: 'write', pull_requests: 'write' } },
    },
    {
      method: 'GET',
      path: '/installation/repositories',
      authorization: `Bearer ${installationToken}`,
      body: undefined,
    },
  ]);

  const invalidFixtures = [
    { app: { ...writerApp, id: 999 } },
    { app: { ...writerApp, slug: 'different-writer' } },
    { app: writerApp, installation: installation({ app_id: 999 }) },
    { app: writerApp, installation: installation({ app_slug: 'different-writer' }) },
    { app: writerApp, installation: installation({ repository_selection: 'all' }) },
    { app: writerApp, installation: installation({ account: { login: 'attacker' } }) },
    { app: writerApp, installation: installation({ permissions: { contents: 'write', pull_requests: 'write', issues: 'read' } }) },
    { app: writerApp, access: installationAccess({ token: appJwt }) },
    { app: writerApp, access: installationAccess({ expires_at: '2020-01-01T00:00:00Z' }) },
    {
      app: writerApp,
      access: installationAccess({ expires_at: new Date(Date.now() + (71 * 60 * 1_000)).toISOString() }),
    },
    { app: writerApp, access: installationAccess({ repository_selection: 'all' }) },
    { app: writerApp, access: installationAccess({ permissions: { contents: 'write', pull_requests: 'write', checks: 'write' } }) },
    { app: writerApp, access: installationAccess({ repositories: [] }) },
    { app: writerApp, access: installationAccess({ repositories: [{ id: 999, full_name: repository }] }) },
    { app: writerApp, repositories: installationRepositories({ repository_selection: 'all' }) },
    { app: writerApp, repositories: installationRepositories({ total_count: 2 }) },
    { app: writerApp, repositories: installationRepositories({ repositories: [] }) },
    {
      app: writerApp,
      repositories: installationRepositories({
        repositories: [{ id: 999, full_name: repository }],
      }),
    },
    {
      app: writerApp,
      repositories: installationRepositories({
        repositories: [{ id: repositoryId, full_name: 'attacker/repository' }],
      }),
    },
  ];

  for (const fixture of invalidFixtures) {
    const calls = [];
    const api = client(async (url, init) => {
      calls.push({ url, init });
      if (url === 'https://api.github.com/app') return response(fixture.app);
      if (url === 'https://api.github.com/repos/example/repository/installation') {
        return response(fixture.installation ?? installation());
      }
      if (url === `https://api.github.com/app/installations/${installationId}/access_tokens`) {
        return response(fixture.access ?? installationAccess(), 201);
      }
      if (url.includes('/installation/repositories?')) {
        return response(fixture.repositories ?? installationRepositories());
      }
      return response({ message: 'unexpected' }, 500);
    });
    await assert.rejects(() => api.verifyInstallationIdentity(), /Writer/);
    await assert.rejects(() => api.createAgentRef(7, oldSha), /identity is not verified/);
    assert.equal(calls.some(({ url }) => url.includes('/git/refs')), false);
  }

  let unverifiedCalls = 0;
  const unverified = client(async () => {
    unverifiedCalls += 1;
    return response({});
  });
  assert.throws(() => unverified.getIssue(7), /token has not been minted and verified/);
  await assert.rejects(() => unverified.createAgentRef(7, oldSha), /identity is not verified/);
  assert.equal(unverifiedCalls, 0);
});

test('Writer ignores optional token metadata but validates any optional repository list', async () => {
  const api = client(async (url) => {
    if (url === 'https://api.github.com/app') return response(writerApp);
    if (url === 'https://api.github.com/repos/example/repository/installation') return response(installation());
    if (url === `https://api.github.com/app/installations/${installationId}/access_tokens`) {
      return response(installationAccess({
        repositories: [{ id: repositoryId, full_name: repository }],
        has_multiple_single_files: false,
        single_file: null,
      }), 201);
    }
    if (url.includes('/installation/repositories?')) {
      const value = installationRepositories();
      delete value.repository_selection;
      return response(value);
    }
    return response({ message: 'unexpected' }, 500);
  });

  assert.equal((await api.verifyInstallationIdentity()).repositoryId, repositoryId);
});

test('Writer never enables an installation token from an unreadable 201 mint response', async () => {
  const expectations = new Map([
    ['invalid JSON', /invalid JSON/],
    ['truncated JSON', /invalid JSON/],
    ['stream failure', /response body could not be read/],
    ['oversize body', /response exceeds the configured limit/],
    ['body timeout', /response body timed out/],
  ]);

  for (const fixture of ambiguousSuccessBodies) {
    const state = { bodyCanceled: false };
    const calls = [];
    const api = client(async (url, init) => {
      calls.push({ url, init });
      if (url === 'https://api.github.com/app') return response(writerApp);
      if (url === 'https://api.github.com/repos/example/repository/installation') return response(installation());
      if (url === `https://api.github.com/app/installations/${installationId}/access_tokens`) {
        return fixture.make(state, 201);
      }
      return response({ message: 'unexpected' }, 500);
    }, { totalTimeoutMs: 100, headersTimeoutMs: 20, bodyTimeoutMs: 15 });

    await assert.rejects(() => api.verifyInstallationIdentity(), expectations.get(fixture.name), fixture.name);
    assert.throws(() => api.getIssue(7), /token has not been minted and verified/);
    await assert.rejects(() => api.createAgentRef(7, oldSha), /identity is not verified/);
    assert.equal(calls.some(({ url }) => url.includes('/installation/repositories')), false);
    assert.equal(calls.some(({ url }) => url.includes('/git/refs')), false);
    if (fixture.name === 'oversize body' || fixture.name === 'body timeout') {
      assert.equal(state.bodyCanceled, true, `${fixture.name} must cancel its response stream`);
    }
  }
});

test('Writer streams and cancels API responses above the one MiB limit', async () => {
  let canceled = false;
  let signal;
  const api = await verifiedClient(async (_url, init) => {
    signal = init.signal;
    return new Response(new ReadableStream({
      start(controller) {
        controller.enqueue(new Uint8Array(1_048_577));
      },
      cancel() {
        canceled = true;
      },
    }));
  });

  await assert.rejects(() => api.getIssue(7), /response exceeds the configured limit/);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(signal.aborted, true);
  assert.equal(canceled, true);
});

test('Writer enforces hard headers, whole-body, and total API deadlines even when fetch ignores abort', async () => {
  let headersSignal;
  const headers = await verifiedClient((_url, init) => {
    headersSignal = init.signal;
    return new Promise(() => {});
  }, { totalTimeoutMs: 120, headersTimeoutMs: 30, bodyTimeoutMs: 60 });
  await assert.rejects(() => headers.getIssue(7), /response headers timed out/);
  assert.equal(headersSignal.aborted, true);

  let bodyCanceled = false;
  let bodySignal;
  let interval;
  const body = await verifiedClient(async (_url, init) => {
    bodySignal = init.signal;
    return new Response(new ReadableStream({
      start(controller) {
        interval = setInterval(() => controller.enqueue(new TextEncoder().encode(' ')), 5);
      },
      cancel() {
        clearInterval(interval);
        bodyCanceled = true;
      },
    }));
  }, { totalTimeoutMs: 120, headersTimeoutMs: 30, bodyTimeoutMs: 40 });
  await assert.rejects(() => body.getIssue(7), /response body timed out/);
  assert.equal(bodySignal.aborted, true);
  assert.equal(bodyCanceled, true);

  let totalSignal;
  const total = await verifiedClient((_url, init) => {
    totalSignal = init.signal;
    return new Promise((resolve) => {
      setTimeout(() => resolve(new Response(new ReadableStream({
        start(controller) {
          setTimeout(() => {
            controller.enqueue(new TextEncoder().encode('{"id":7}'));
            controller.close();
          }, 50);
        },
      }))), 50);
    });
  }, { totalTimeoutMs: 80, headersTimeoutMs: 60, bodyTimeoutMs: 60 });
  await assert.rejects(() => total.getIssue(7), /API request timed out/);
  assert.equal(totalSignal.aborted, true);
});

test('Writer forces its ownership marker on create and preserves it on managed metadata updates', async () => {
  const calls = [];
  let metadataUpdated = false;
  const api = await verifiedClient(async (url, init) => {
    calls.push({ url, init });
    if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
    if (init.method === 'POST') return response(rawPull(), 201);
    if (init.method === 'PATCH') {
      metadataUpdated = true;
      return response(rawPull({ title: 'Updated title', body: `Updated details\n\n${WRITER_OWNERSHIP_MARKER}` }));
    }
    return response(metadataUpdated
      ? rawPull({ title: 'Updated title', body: `Updated details\n\n${WRITER_OWNERSHIP_MARKER}` })
      : rawPull());
  });

  await api.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' });
  await api.updateManagedDraftPull(7, 42, oldSha, { title: 'Updated title', body: 'Updated details' });

  const writes = calls.filter(({ init }) => init.method === 'POST' || init.method === 'PATCH').map(({ url, init }) => ({
    method: init.method,
    path: new URL(url).pathname,
    body: JSON.parse(init.body),
  }));
  assert.deepEqual(writes, [
    {
      method: 'POST',
      path: '/repos/example/repository/pulls',
      body: {
        title: 'Fix issue 7',
        body: createdBody(),
        base: 'main',
        head: 'agent/issue-7',
        draft: true,
      },
    },
    {
      method: 'PATCH',
      path: '/repos/example/repository/pulls/42',
      body: { title: 'Updated title', body: `Updated details\n\n${WRITER_OWNERSHIP_MARKER}` },
    },
  ]);
  assert.deepEqual(calls.map(({ init }) => init.method), [
    'GET', 'GET', 'POST', 'GET', 'GET', 'GET', 'GET', 'PATCH', 'GET',
  ]);
});

test('Writer rejects caller-supplied ownership markers in any body position before API access', async () => {
  let calls = 0;
  const api = await verifiedClient(async () => {
    calls += 1;
    return response(null, 500);
  });
  for (const body of [
    WRITER_OWNERSHIP_MARKER,
    createAttemptMarker,
    `Quoted ${createAttemptMarker} text`,
    `Quoted ${WRITER_OWNERSHIP_MARKER} text`,
    `> ${WRITER_OWNERSHIP_MARKER}`,
    `Details\n\n${WRITER_OWNERSHIP_MARKER}`,
    `${WRITER_OWNERSHIP_MARKER}\n\n${WRITER_OWNERSHIP_MARKER}`,
  ]) {
    await assert.rejects(
      () => api.createDraftPull(7, { title: 'Fix issue 7', body }),
      /reserved Writer (?:ownership|create-attempt) marker/,
    );
  }
  assert.equal(calls, 0);
});

test('Writer body byte limit includes its unique create-attempt and canonical ownership markers', async () => {
  const separator = '\n\n';
  const markerBytes = Buffer.byteLength(WRITER_OWNERSHIP_MARKER, 'utf8');
  const attemptBytes = Buffer.byteLength(createAttemptMarker, 'utf8');
  const maximumCallerBytes = 65_536 - (2 * Buffer.byteLength(separator, 'utf8')) - attemptBytes - markerBytes;
  const acceptedBody = 'a'.repeat(maximumCallerBytes);
  let acceptedRequest = null;
  const accepted = await verifiedClient(async (url, init) => {
    if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
    if (init.method === 'POST') {
      acceptedRequest = JSON.parse(init.body);
      return response(rawPull({ body: createdBody(acceptedBody) }), 201);
    }
    return response(rawPull({ body: createdBody(acceptedBody) }));
  });
  await accepted.createDraftPull(7, { title: 'Fix issue 7', body: acceptedBody });
  assert.equal(Buffer.byteLength(acceptedRequest.body, 'utf8'), 65_536);
  assert.equal(acceptedRequest.body, createdBody(acceptedBody));

  let rejectedCalls = 0;
  const rejected = client(async () => {
    rejectedCalls += 1;
    return response(null, 500);
  });
  await assert.rejects(
    () => rejected.createDraftPull(7, { title: 'Fix issue 7', body: 'a'.repeat(maximumCallerBytes + 1) }),
    /Pull request body exceeds the configured limit/,
  );
  assert.equal(rejectedCalls, 0);
});

test('createDraftPull uses a persisted PR snapshot instead of trusting an attacker-controlled POST response', async () => {
  const calls = [];
  const api = await verifiedClient(async (url, init) => {
    calls.push({ url, init });
    if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
    if (init.method === 'POST') return response(rawPull({
      state: 'closed',
      merged: true,
      draft: false,
      body: 'forged marker removal',
      user: { login: 'human', type: 'User' },
      base: { ref: 'release', repo: { id: 999, full_name: 'attacker/repository' } },
      head: { ref: 'agent/issue-999', sha: newSha, repo: { id: 999, full_name: 'attacker/repository' } },
    }), 201);
    return response(rawPull());
  });

  const pull = await api.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' });
  assert.deepEqual(pull.author, { type: 'App', id: writerApp.id });
  assert.deepEqual(calls.map(({ init }) => init.method), ['GET', 'GET', 'POST', 'GET', 'GET']);
});

test('createDraftPull reconciles a corrupt returned PR number to its unique attempt marker without closing', async () => {
  const calls = [];
  let closed = false;
  const api = await verifiedClient(async (url, init) => {
    calls.push({ url, init });
    if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
    if (init.method === 'POST') return response({ number: 99 }, 201);
    if (url.endsWith('/pulls/99')) return response(rawPull({ number: 99, body: 'unrelated PR' }));
    if (url.includes('/pulls?')) return response([rawPull()]);
    if (init.method === 'PATCH') {
      closed = true;
      return response(rawPull({ state: 'closed' }));
    }
    return response(rawPull({ state: closed ? 'closed' : 'open' }));
  });

  const pull = await api.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' });
  assert.equal(pull.number, 42);
  assert.equal(closed, false);
  assert.equal(calls.some(({ init }) => init.method === 'PATCH'), false);
  assert.equal(calls.filter(({ init }) => init.method === 'POST').length, 1);
});

test('createDraftPull fails closed when POST/GET responses cannot establish the persisted managed PR postcondition', async () => {
  const unverifiedCandidates = [
    rawPull({ state: 'closed' }),
    rawPull({ merged: true }),
    rawPull({ draft: false }),
    rawPull({ base: { ref: 'release', repo: { id: repositoryId, full_name: repository } } }),
    rawPull({ base: { ref: 'main', repo: { id: 999, full_name: 'attacker/repository' } } }),
    rawPull({ head: { ref: 'agent/issue-8', sha: oldSha, repo: { id: repositoryId, full_name: repository } } }),
    rawPull({ head: { ref: 'agent/issue-7', sha: oldSha, repo: { id: 999, full_name: 'attacker/repository' } } }),
    rawPull({ body: 'marker removed' }),
    rawPull({ user: { login: 'human', type: 'User' } }),
  ];

  for (const persisted of unverifiedCandidates) {
    const methods = [];
    const api = await verifiedClient(async (url, init) => {
      methods.push(init.method);
      if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
      if (init.method === 'POST') return response(rawPull(), 201);
      return response(persisted);
    });
    await assert.rejects(
      () => api.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' }),
      WriterGitHubApiError,
    );
    assert.deepEqual(methods.slice(0, 5), ['GET', 'GET', 'POST', 'GET', 'GET']);
    assert.equal(methods.filter((method) => method === 'POST').length, 1);
    assert.equal(methods.includes('PATCH'), false);
  }

  const malformedPost = await verifiedClient(async (url, init) => {
    if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
    return response({ number: '42' }, 201);
  });
  await assert.rejects(() => malformedPost.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' }), WriterGitHubApiError);

  const unexpectedStatusMethods = [];
  const unexpectedStatus = await verifiedClient(async (url, init) => {
    unexpectedStatusMethods.push(init.method);
    if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
    return response({ message: 'validation failed' }, 422);
  });
  await assert.rejects(
    () => unexpectedStatus.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' }),
    /HTTP 422/,
  );
  assert.deepEqual(unexpectedStatusMethods, ['GET', 'GET', 'POST']);

  const failedGet = await verifiedClient(async (url, init) => {
    if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
    if (init.method === 'POST') return response(rawPull(), 201);
    return response({ message: 'server error' }, 500);
  });
  await assert.rejects(
    () => failedGet.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' }),
    /platform residue may remain/,
  );
});

test('createDraftPull read-only reconciles one nonce-attributable PR after ambiguous transport and 201 bodies', async () => {
  const fixtures = [
    {
      name: 'connection failure',
      expected: /connection failed/,
      post: () => { throw new TypeError('connection failed'); },
    },
    { name: 'headers timeout', expected: /response headers timed out/, post: () => new Promise(() => {}) },
    { name: 'unexpected success status', expected: /expected 201/, post: () => response({ number: 42 }, 200) },
    { name: 'server error', expected: /HTTP 500/, post: () => response({ message: 'server error' }, 500) },
    { name: 'invalid JSON', expected: /invalid JSON/, post: () => textResponse('not-json', 201) },
    { name: 'truncated JSON', expected: /invalid JSON/, post: () => textResponse('{"number":42', 201) },
    {
      name: 'stream failure',
      expected: /response body could not be read/,
      post: () => new Response(new ReadableStream({
        start(controller) {
          controller.enqueue(new TextEncoder().encode('{"number":'));
          controller.error(new Error('socket reset'));
        },
      }), { status: 201 }),
    },
    {
      name: 'oversize body',
      expected: /response exceeds the configured limit/,
      post: (state) => new Response(new ReadableStream({
        start(controller) {
          controller.enqueue(new Uint8Array(1_048_577));
        },
        cancel() {
          state.bodyCanceled = true;
        },
      }), { status: 201 }),
    },
    {
      name: 'body timeout',
      expected: /response body timed out/,
      post: (state) => new Response(new ReadableStream({
        start(controller) {
          controller.enqueue(new TextEncoder().encode('{"number":'));
        },
        cancel() {
          state.bodyCanceled = true;
        },
      }), { status: 201 }),
    },
  ];

  for (const fixture of fixtures) {
    const state = { closed: false, bodyCanceled: false };
    const calls = [];
    const api = await verifiedClient(async (url, init) => {
      calls.push({ url, init });
      if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
      if (init.method === 'POST') return fixture.post(state, init);
      if (url.includes('/pulls?')) return response([rawPull()]);
      if (init.method === 'PATCH') {
        assert.deepEqual(JSON.parse(init.body), { state: 'closed' });
        state.closed = true;
        return response(rawPull({ state: 'closed' }));
      }
      if (url.endsWith('/pulls/42')) return response(rawPull({ state: state.closed ? 'closed' : 'open' }));
      return response({ message: 'unexpected' }, 500);
    }, { totalTimeoutMs: 100, headersTimeoutMs: 20, bodyTimeoutMs: 15 });

    const pull = await api.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' });
    assert.equal(pull.number, 42, fixture.name);
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.equal(state.closed, false, `${fixture.name} must leave the attributable PR open`);
    if (fixture.name === 'oversize body' || fixture.name === 'body timeout') {
      assert.equal(state.bodyCanceled, true, `${fixture.name} must cancel its response stream`);
    }
    assert.equal(calls.filter(({ init }) => init.method === 'POST').length, 1, 'an ambiguous create must never retry POST');
    assert.deepEqual(calls.map(({ init }) => init.method), ['GET', 'GET', 'POST', 'GET', 'GET', 'GET']);
    const recovery = new URL(calls[3].url);
    assert.equal(recovery.pathname, '/repos/example/repository/pulls');
    assert.equal(recovery.searchParams.get('state'), 'all');
    assert.equal(recovery.searchParams.get('base'), 'main');
    assert.equal(recovery.searchParams.get('head'), 'example:agent/issue-7');
  }
});

test('createDraftPull refuses ambiguous recovery without exactly one fully verified attempt marker', async () => {
  const fixtures = [
    { name: 'missing', pulls: [], expected: /could not uniquely attribute/ },
    {
      name: 'duplicate',
      pulls: [rawPull(), rawPull({ number: 43 })],
      expected: /could not uniquely attribute/,
    },
    {
      name: 'wrong identity',
      pulls: [rawPull({ user: { login: 'human', type: 'User' } })],
      expected: /refused to reconcile/,
    },
    {
      name: 'mutated body',
      pulls: [rawPull({ body: `${createAttemptMarker}\n\nforged` })],
      expected: /refused to reconcile/,
    },
    {
      name: 'wrong exact head',
      pulls: [rawPull({
        head: { ref: 'agent/issue-7', sha: newSha, repo: { id: repositoryId, full_name: repository } },
      })],
      expected: /refused to reconcile/,
    },
  ];

  for (const fixture of fixtures) {
    const methods = [];
    const api = await verifiedClient(async (url, init) => {
      methods.push(init.method);
      if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
      if (init.method === 'POST') return textResponse('{"number":', 201);
      if (url.includes('/pulls?')) return response(fixture.pulls);
      return response({ message: 'unexpected' }, 500);
    });
    await assert.rejects(
      () => api.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' }),
      fixture.expected,
      fixture.name,
    );
    assert.equal(methods.filter((method) => method === 'POST').length, 1);
    assert.equal(methods.includes('PATCH'), false, `${fixture.name} must not close any PR`);
  }
});

test('createDraftPull enumerates after a valid POST number and rejects duplicate attempt markers', async () => {
  const calls = [];
  const api = await verifiedClient(async (url, init) => {
    calls.push({ url, init });
    if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
    if (init.method === 'POST') return response({ number: 42 }, 201);
    if (url.endsWith('/pulls/42')) return response(rawPull({ body: 'persisted body was replaced' }));
    if (url.includes('/pulls?')) return response([rawPull(), rawPull({ number: 43 })]);
    return response({ message: 'unexpected' }, 500);
  });

  await assert.rejects(
    () => api.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' }),
    /could not uniquely attribute/,
  );
  assert.equal(calls.filter(({ init }) => init.method === 'PATCH').length, 0);
  const enumeration = calls.find(({ url }) => url.includes('/pulls?'));
  assert.ok(enumeration, 'a valid POST number must not skip bounded head enumeration');
  assert.equal(new URL(enumeration.url).searchParams.get('state'), 'all');
});

test('createDraftPull refuses a closed attributable candidate whose head has drifted', async () => {
  let patches = 0;
  const api = await verifiedClient(async (url, init) => {
    if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
    if (init.method === 'POST') return textResponse('not-json', 201);
    if (url.includes('/pulls?')) return response([rawPull({
      state: 'closed',
      head: { ref: 'agent/issue-7', sha: newSha, repo: { id: repositoryId, full_name: repository } },
    })]);
    if (init.method === 'PATCH') patches += 1;
    return response({ message: 'unexpected' }, 500);
  });

  await assert.rejects(() => api.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' }), /refused to reconcile/);
  assert.equal(patches, 0);
});

test('createDraftPull refuses candidate drift during read-only reconciliation without a cleanup mutation', async () => {
  let patches = 0;
  const api = await verifiedClient(async (url, init) => {
    if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
    if (init.method === 'POST') return textResponse('not-json', 201);
    if (url.includes('/pulls?')) return response([rawPull()]);
    if (init.method === 'PATCH') patches += 1;
    if (url.endsWith('/pulls/42')) return response(rawPull({
      head: { ref: 'agent/issue-7', sha: newSha, repo: { id: repositoryId, full_name: repository } },
    }));
    return response({ message: 'unexpected' }, 500);
  });

  await assert.rejects(
    () => api.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' }),
    /changed after create/,
  );
  assert.equal(patches, 0);
});

test('createDraftPull reconciles an unreadable create response without attempting close', async () => {
  let closed = false;
  const methods = [];
  const api = await verifiedClient(async (url, init) => {
    methods.push(init.method);
    if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
    if (init.method === 'POST') return textResponse('{"number":', 201);
    if (url.includes('/pulls?')) return response([rawPull()]);
    if (init.method === 'PATCH') {
      closed = true;
      return textResponse('not-json', 200);
    }
    return response(rawPull({ state: closed ? 'closed' : 'open' }));
  });

  const pull = await api.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' });
  assert.equal(pull.number, 42);
  assert.equal(closed, false);
  assert.deepEqual(methods, ['GET', 'GET', 'POST', 'GET', 'GET', 'GET']);
});

test('createDraftPull fences pre-POST ref drift without creating a PR', async () => {
  const calls = [];
  let refReads = 0;
  const api = await verifiedClient(async (url, init) => {
    calls.push({ url, init });
    if (url.includes('/git/ref/heads/')) {
      refReads += 1;
      return response(rawRef(7, refReads === 1 ? oldSha : newSha));
    }
    return response(rawPull());
  });

  await assert.rejects(
    () => api.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' }),
    /requested branch and SHA/,
  );
  assert.deepEqual(calls.map(({ init }) => init.method), ['GET', 'GET']);
});

test('createDraftPull leaves fail-closed residue after post-POST ref drift and never closes', async () => {
  const calls = [];
  let refReads = 0;
  let closed = false;
  const api = await verifiedClient(async (url, init) => {
    calls.push({ url, init });
    if (url.includes('/git/ref/heads/')) {
      refReads += 1;
      return response(rawRef(7, refReads < 3 ? oldSha : newSha));
    }
    if (init.method === 'POST') return response({ number: 42 }, 201);
    if (url.includes('/pulls?')) return response([rawPull({
      head: { ref: 'agent/issue-7', sha: oldSha, repo: { id: repositoryId, full_name: repository } },
    })]);
    if (init.method === 'PATCH') {
      assert.deepEqual(JSON.parse(init.body), { state: 'closed' });
      closed = true;
      return response(rawPull({ state: 'closed', head: { ref: 'agent/issue-7', sha: newSha, repo: { id: repositoryId, full_name: repository } } }));
    }
    return response(rawPull({
      state: closed ? 'closed' : 'open',
      head: { ref: 'agent/issue-7', sha: closed ? newSha : oldSha, repo: { id: repositoryId, full_name: repository } },
    }), 201);
  });

  await assert.rejects(
    () => api.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' }),
    /platform residue may remain/,
  );
  assert.equal(closed, false);
  assert.deepEqual(calls.map(({ init }) => init.method), [
    'GET', 'GET', 'POST', 'GET', 'GET', 'GET', 'GET', 'GET',
  ]);
  assert.equal(calls.some(({ init }) => init.method === 'PATCH'), false);
});

test('createDraftPull refuses to reconcile a Draft PR whose exact head drifted during POST', async () => {
  const methods = [];
  let closed = false;
  const api = await verifiedClient(async (url, init) => {
    methods.push(init.method);
    if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
    if (init.method === 'POST') return response({ number: 42 }, 201);
    if (url.includes('/pulls?')) return response([rawPull({
      head: { ref: 'agent/issue-7', sha: newSha, repo: { id: repositoryId, full_name: repository } },
    })]);
    if (init.method === 'PATCH') {
      closed = true;
      return response(rawPull({
        state: 'closed',
        head: { ref: 'agent/issue-7', sha: newSha, repo: { id: repositoryId, full_name: repository } },
      }));
    }
    return response(rawPull({
      state: closed ? 'closed' : 'open',
      head: { ref: 'agent/issue-7', sha: newSha, repo: { id: repositoryId, full_name: repository } },
    }));
  });

  await assert.rejects(
    () => api.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' }),
    /refused to reconcile/,
  );
  assert.equal(closed, false);
  assert.deepEqual(methods, ['GET', 'GET', 'POST', 'GET', 'GET']);
});

test('createDraftPull reports platform residue without ever attempting cleanup close', async () => {
  for (const failure of ['patch', 'readback']) {
    let refReads = 0;
    let patchCalls = 0;
    const api = await verifiedClient(async (url, init) => {
      if (url.includes('/git/ref/heads/')) {
        refReads += 1;
        return response(rawRef(7, refReads < 3 ? oldSha : newSha));
      }
      if (init.method === 'POST') return response({ number: 42 }, 201);
      if (url.includes('/pulls?')) return response([rawPull({ state: 'open' })]);
      if (init.method === 'PATCH') {
        patchCalls += 1;
        if (failure === 'patch') return response({ message: 'server error' }, 500);
        return response(rawPull({ state: 'closed' }));
      }
      return response(rawPull({ state: 'open' }));
    });

    await assert.rejects(
      () => api.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' }),
      /platform residue may remain/,
    );
    assert.equal(patchCalls, 0);
  }
});

test('managed Draft PR update fails closed before PATCH for cross-PR, state, head, repository, and ownership violations', async () => {
  const candidates = [
    rawPull({ number: 41 }),
    rawPull({ state: 'closed' }),
    rawPull({ merged: true }),
    rawPull({ draft: false }),
    rawPull({ base: { ref: 'release', repo: { id: repositoryId, full_name: repository } } }),
    rawPull({ head: { ref: 'agent/issue-8', sha: oldSha, repo: { id: repositoryId, full_name: repository } } }),
    rawPull({ head: { ref: 'agent/issue-7', sha: oldSha, repo: { id: 999, full_name: 'attacker/repository' } } }),
    rawPull({ head: { ref: 'agent/issue-7', sha: newSha, repo: { id: repositoryId, full_name: repository } } }),
    rawPull({ body: 'marker removed' }),
    rawPull({ user: { login: 'attacker[bot]', type: 'Bot' } }),
    rawPull({ performed_via_github_app: { id: 999, slug: writerApp.slug } }),
    rawPull({ performed_via_github_app: { id: writerApp.id, slug: 'different-writer' } }),
  ];

  for (const candidate of candidates) {
    let calls = 0;
    const api = await verifiedClient(async () => {
      calls += 1;
      return response(candidate);
    });
    await assert.rejects(
      () => api.updateManagedDraftPull(7, 42, oldSha, { title: 'No write' }),
      WriterGitHubApiError,
    );
    assert.equal(calls, 1, 'an invalid pre-write snapshot must never reach PATCH');
  }
});

test('managed Draft PR update rereads and rejects marker removal or stale head after PATCH', async () => {
  for (const after of [
    rawPull({ title: 'Updated', body: 'marker removed' }),
    rawPull({ title: 'Updated', head: { ref: 'agent/issue-7', sha: newSha, repo: { id: repositoryId, full_name: repository } } }),
  ]) {
    const methods = [];
    const api = await verifiedClient(async (_url, init) => {
      methods.push(init.method);
      if (init.method === 'GET' && methods.length <= 2) return response(rawPull());
      if (init.method === 'PATCH') return response(rawPull({ title: 'Updated' }));
      return response(after);
    });
    await assert.rejects(() => api.updateManagedDraftPull(7, 42, oldSha, { title: 'Updated' }), WriterGitHubApiError);
    assert.deepEqual(methods, ['GET', 'GET', 'PATCH', 'GET']);
  }
});

test('managed Draft PR update refuses a second pre-PATCH fencing read that drifts', async () => {
  const methods = [];
  const api = await verifiedClient(async (_url, init) => {
    methods.push(init.method);
    return response(methods.length === 1
      ? rawPull()
      : rawPull({ head: { ref: 'agent/issue-7', sha: newSha, repo: { id: repositoryId, full_name: repository } } }));
  });

  await assert.rejects(() => api.updateManagedDraftPull(7, 42, oldSha, { title: 'Updated' }), /head SHA changed/);
  assert.deepEqual(methods, ['GET', 'GET']);
});

test('managed Draft PR update reconciles ambiguous successful response bodies by exact readback', async () => {
  for (const fixture of ambiguousSuccessBodies) {
    const state = { updated: false, bodyCanceled: false };
    const methods = [];
    const api = await verifiedClient(async (_url, init) => {
      methods.push(init.method);
      if (init.method === 'PATCH') {
        state.updated = true;
        return fixture.make(state, 200);
      }
      return response(rawPull(state.updated ? { title: 'Updated' } : {}));
    }, { totalTimeoutMs: 100, headersTimeoutMs: 20, bodyTimeoutMs: 15 });

    const updated = await api.updateManagedDraftPull(7, 42, oldSha, { title: 'Updated' });
    assert.equal(updated.title, 'Updated', fixture.name);
    assert.deepEqual(methods, ['GET', 'GET', 'PATCH', 'GET'], fixture.name);
    if (fixture.name === 'oversize body' || fixture.name === 'body timeout') {
      assert.equal(state.bodyCanceled, true, fixture.name);
    }
  }
});

test('createAgentRef is issue-bound and verifies absence, response, and persisted head', async () => {
  const calls = [];
  const api = await verifiedClient(async (url, init) => {
    calls.push({ url, init });
    if (calls.length === 1) return response({ message: 'Not Found' }, 404);
    return response(rawRef(7, oldSha));
  });

  assert.deepEqual(await api.createAgentRef(7, oldSha), rawRef(7, oldSha));
  assert.deepEqual(calls.map(({ url, init }) => ({
    method: init.method,
    path: new URL(url).pathname,
    body: init.body === undefined ? undefined : JSON.parse(init.body),
  })), [
    { method: 'GET', path: '/repos/example/repository/git/ref/heads/agent%2Fissue-7', body: undefined },
    { method: 'POST', path: '/repos/example/repository/git/refs', body: { ref: 'refs/heads/agent/issue-7', sha: oldSha } },
    { method: 'GET', path: '/repos/example/repository/git/ref/heads/agent%2Fissue-7', body: undefined },
  ]);
});

test('createAgentRef reconciles ambiguous successful response bodies by exact ref readback', async () => {
  for (const fixture of ambiguousSuccessBodies) {
    const state = { created: false, bodyCanceled: false };
    const methods = [];
    const api = await verifiedClient(async (_url, init) => {
      methods.push(init.method);
      if (init.method === 'POST') {
        state.created = true;
        return fixture.make(state, 201);
      }
      if (!state.created) return response({ message: 'Not Found' }, 404);
      return response(rawRef(7, oldSha));
    }, { totalTimeoutMs: 100, headersTimeoutMs: 20, bodyTimeoutMs: 15 });

    assert.deepEqual(await api.createAgentRef(7, oldSha), rawRef(7, oldSha), fixture.name);
    assert.deepEqual(methods, ['GET', 'POST', 'GET'], fixture.name);
    if (fixture.name === 'oversize body' || fixture.name === 'body timeout') {
      assert.equal(state.bodyCanceled, true, fixture.name);
    }
  }
});

test('advanceAgentRef rejects stale heads and uses only a non-force issue-bound update', async () => {
  let staleCalls = 0;
  const stale = await verifiedClient(async () => {
    staleCalls += 1;
    return response(rawRef(7, newSha));
  });
  await assert.rejects(() => stale.advanceAgentRef(7, oldSha, newSha), /requested branch and SHA/);
  assert.equal(staleCalls, 1);

  const calls = [];
  const api = await verifiedClient(async (url, init) => {
    calls.push({ url, init });
    return response(calls.length <= 2 ? rawRef(7, oldSha) : rawRef(7, newSha));
  });
  assert.deepEqual(await api.advanceAgentRef(7, oldSha, newSha), rawRef(7, newSha));
  assert.deepEqual(calls.map(({ url, init }) => ({
    method: init.method,
    path: new URL(url).pathname,
    body: init.body === undefined ? undefined : JSON.parse(init.body),
  })), [
    { method: 'GET', path: '/repos/example/repository/git/ref/heads/agent%2Fissue-7', body: undefined },
    { method: 'GET', path: '/repos/example/repository/git/ref/heads/agent%2Fissue-7', body: undefined },
    {
      method: 'PATCH',
      path: '/repos/example/repository/git/refs/heads/agent%2Fissue-7',
      body: { sha: newSha, force: false },
    },
    { method: 'GET', path: '/repos/example/repository/git/ref/heads/agent%2Fissue-7', body: undefined },
  ]);
});

test('advanceAgentRef refuses a second pre-PATCH fencing read that drifts', async () => {
  const methods = [];
  const api = await verifiedClient(async (_url, init) => {
    methods.push(init.method);
    return response(rawRef(7, methods.length === 1 ? oldSha : newSha));
  });

  await assert.rejects(() => api.advanceAgentRef(7, oldSha, newSha), /requested branch and SHA/);
  assert.deepEqual(methods, ['GET', 'GET']);
});

test('advanceAgentRef reconciles ambiguous successful response bodies by exact ref readback', async () => {
  for (const fixture of ambiguousSuccessBodies) {
    const state = { advanced: false, bodyCanceled: false };
    const methods = [];
    const api = await verifiedClient(async (_url, init) => {
      methods.push(init.method);
      if (init.method === 'PATCH') {
        state.advanced = true;
        return fixture.make(state, 200);
      }
      return response(rawRef(7, state.advanced ? newSha : oldSha));
    }, { totalTimeoutMs: 100, headersTimeoutMs: 20, bodyTimeoutMs: 15 });

    assert.deepEqual(await api.advanceAgentRef(7, oldSha, newSha), rawRef(7, newSha), fixture.name);
    assert.deepEqual(methods, ['GET', 'GET', 'PATCH', 'GET'], fixture.name);
    if (fixture.name === 'oversize body' || fixture.name === 'body timeout') {
      assert.equal(state.bodyCanceled, true, fixture.name);
    }
  }
});

test('Writer invalid input and ref bounds fail before issuing a network call', async () => {
  let calls = 0;
  const api = client(async () => {
    calls += 1;
    return response({});
  });
  const invalidCalls = [
    () => api.getIssue(0),
    () => api.getIssueComment(0),
    () => api.getCollaboratorPermission('../admin'),
    () => api.getPull(1.5),
    () => api.getRef('refs/heads/main'),
    () => api.listPullsForHead('agent/issue-0'),
    () => api.createDraftPull(7, { title: 'x', body: 'x', draft: false }),
    () => api.updateManagedDraftPull(7, 42, 'BAD', { title: 'x' }),
    () => api.updateManagedDraftPull(7, 42, oldSha, { state: 'closed' }),
    () => api.createAgentRef(0, oldSha),
    () => api.createAgentRef(7, 'a'.repeat(39)),
    () => api.advanceAgentRef(8, oldSha, oldSha),
  ];
  for (const invoke of invalidCalls) await assert.rejects(async () => invoke(), WriterGitHubApiError);
  assert.equal(calls, 0);
});

test('Writer bounds pagination and exposes no dangerous or generic operation methods', async () => {
  let calls = 0;
  const api = await verifiedClient(async () => {
    calls += 1;
    return response(Array.from({ length: 100 }, () => rawPull()));
  });

  await assert.rejects(() => api.listPullsForHead('agent/issue-7'), /page limit/);
  assert.equal(calls, 3);

  for (const method of [
    'request', 'list', 'createIssueComment', 'updateIssueComment', 'createReview',
    'approve', 'merge', 'enableAutoMerge', 'createCheckRun', 'closePull', 'markReady',
    'deleteRef', 'updateRef', 'forceUpdateRef', 'updateDraftPullMetadata',
  ]) assert.equal(typeof api[method], 'undefined', `${method} must not be public`);
});

test('Writer constructor rejects ambiguous repositories, insecure origin customization, and incomplete identities', async () => {
  const invalid = [
    { appJwt: '', repository, repositoryId, writerApp },
    { appJwt: 'not-a-jwt', repository, repositoryId, writerApp },
    { appJwt, token: 'caller-supplied-installation-token', repository, repositoryId, writerApp },
    { appJwt, repository: 'bad/repository/path', repositoryId, writerApp },
    { appJwt, repository: '-owner/repository', repositoryId, writerApp },
    { appJwt, repository: 'owner--name/repository', repositoryId, writerApp },
    { appJwt, repository: 'owner/.', repositoryId, writerApp },
    { appJwt, repository, repositoryId: 0, writerApp },
    { appJwt, repository, repositoryId, writerApp: { id: writerApp.id } },
    { appJwt, repository, repositoryId, writerApp: { id: writerApp.id, slug: 'Bad Slug' } },
    { appJwt, repository, repositoryId, writerApp, randomUUIDImpl: 'not-a-function' },
    { appJwt, repository, repositoryId, writerApp, apiUrl: 'https://attacker.invalid' },
  ];
  for (const options of invalid) {
    assert.throws(() => new WriterGitHubClient({ ...options, fetchImpl: async () => response({}) }), WriterGitHubApiError);
  }
  assert.doesNotThrow(() => new WriterGitHubClient({
    appJwt,
    repository,
    repositoryId: 3_000_000_000,
    writerApp: { id: 3_000_000_001, slug: writerApp.slug },
    fetchImpl: async () => response({}),
  }));

  for (const randomUUIDImpl of [() => 'predictable', () => { throw new Error('entropy failed'); }]) {
    const api = client(async () => response({}), { randomUUIDImpl });
    await assert.rejects(
      () => api.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' }),
      /create attempt ID|generate a create attempt ID/,
    );
  }
});
