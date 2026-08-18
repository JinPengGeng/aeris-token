import assert from 'node:assert/strict';
import test from 'node:test';

import { WriterGitHubApiError, WriterGitHubClient } from '../src/writer-github-client.mjs';
import { evaluateWriterLifecycle, WRITER_OWNERSHIP_MARKER } from '../src/writer-lifecycle.mjs';

const repository = 'example/repository';
const repositoryId = 123;
const writerApp = Object.freeze({ id: 456, slug: 'aeris-writer' });
const oldSha = 'a'.repeat(40);
const newSha = 'b'.repeat(40);

const response = (payload, status = 200) => new Response(payload === null ? null : JSON.stringify(payload), {
  status,
  headers: { 'content-type': 'application/json' },
});

function client(fetchImpl, overrides = {}) {
  return new WriterGitHubClient({
    token: 'writer-test-token',
    repository,
    repositoryId,
    writerApp,
    fetchImpl,
    ...overrides,
  });
}

function rawPull(overrides = {}) {
  return {
    number: 42,
    state: 'open',
    merged: false,
    draft: true,
    title: 'Fix issue 7',
    body: `Details\n\n${WRITER_OWNERSHIP_MARKER}`,
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

test('Writer reads are pinned to GitHub.com, reject redirects, retain tombstones, and normalize App authors', async () => {
  const calls = [];
  const api = client(async (url, init) => {
    calls.push({ url, init });
    if (url.includes('/permission')) return response({ permission: 'write' });
    if (url.includes('/pulls?')) return response([rawPull({ state: 'closed', merged_at: null })]);
    if (url.includes('/pulls/')) return response(rawPull());
    return response({ id: 7 });
  });

  assert.deepEqual(await api.getIssue(7), { id: 7 });
  assert.equal(await api.getCollaboratorPermission('writer-user'), 'write');
  assert.deepEqual((await api.getPull(42)).author, { type: 'App', id: writerApp.id });
  assert.deepEqual(await api.getRef('agent/issue-7'), { id: 7 });
  assert.equal((await api.listPullsForHead('agent/issue-7'))[0].state, 'closed');

  assert.deepEqual(calls.map(({ url, init }) => [url, init.method, init.redirect]), [
    ['https://api.github.com/repos/example/repository/issues/7', 'GET', 'error'],
    ['https://api.github.com/repos/example/repository/collaborators/writer-user/permission', 'GET', 'error'],
    ['https://api.github.com/repos/example/repository/pulls/42', 'GET', 'error'],
    ['https://api.github.com/repos/example/repository/git/ref/heads/agent%2Fissue-7', 'GET', 'error'],
    ['https://api.github.com/repos/example/repository/pulls?state=all&head=example%3Aagent%2Fissue-7&per_page=100&page=1', 'GET', 'error'],
  ]);

  const redirecting = client(async () => response({ location: 'https://attacker.invalid' }, 302));
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
  const api = client(async () => response(fixtures));
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
  const api = client(async () => response(fixtures));
  const pulls = await api.listPullsForHead('agent/issue-7');
  assert.deepEqual(pulls.map(({ merged }) => merged), [null, null, null, null]);
});

test('raw Bot identity accepts a null performed App and overrides any forged lifecycle author field', async () => {
  const appCreated = rawPull({ performed_via_github_app: null });
  const valid = client(async () => response(appCreated));
  assert.deepEqual((await valid.getPull(42)).author, { type: 'App', id: writerApp.id });

  const appFieldOmitted = rawPull();
  delete appFieldOmitted.performed_via_github_app;
  const omitted = client(async () => response(appFieldOmitted));
  assert.deepEqual((await omitted.getPull(42)).author, { type: 'App', id: writerApp.id });

  const forged = rawPull({
    author: { type: 'App', id: writerApp.id },
    user: { login: 'attacker', type: 'User' },
    performed_via_github_app: null,
  });
  const api = client(async () => response(forged));
  assert.equal((await api.getPull(42)).author, null);
});

test('Writer forces its ownership marker on create and preserves it on managed metadata updates', async () => {
  const calls = [];
  const api = client(async (url, init) => {
    calls.push({ url, init });
    if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
    if (init.method === 'POST') return response(rawPull());
    if (init.method === 'PATCH') return response(rawPull({ title: 'Updated title', body: `Updated details\n\n${WRITER_OWNERSHIP_MARKER}` }));
    if (calls.filter(({ init: callInit }) => callInit.method === 'GET').length === 2) return response(rawPull());
    return response(rawPull({
      title: 'Updated title',
      body: `Updated details\n\n${WRITER_OWNERSHIP_MARKER}`,
    }));
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
        body: `Details\n\n${WRITER_OWNERSHIP_MARKER}`,
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
  assert.deepEqual(calls.map(({ init }) => init.method), ['GET', 'POST', 'GET', 'GET', 'PATCH', 'GET']);
});

test('createDraftPull uses a persisted PR snapshot instead of trusting an attacker-controlled POST response', async () => {
  const calls = [];
  const api = client(async (url, init) => {
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
    }));
    return response(rawPull());
  });

  const pull = await api.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' });
  assert.deepEqual(pull.author, { type: 'App', id: writerApp.id });
  assert.deepEqual(calls.map(({ init }) => init.method), ['GET', 'POST', 'GET']);
});

test('createDraftPull fails closed when POST/GET responses cannot establish the persisted managed PR postcondition', async () => {
  const candidates = [
    rawPull({ state: 'closed' }),
    rawPull({ merged: true }),
    rawPull({ draft: false }),
    rawPull({ base: { ref: 'release', repo: { id: repositoryId, full_name: repository } } }),
    rawPull({ base: { ref: 'main', repo: { id: 999, full_name: 'attacker/repository' } } }),
    rawPull({ head: { ref: 'agent/issue-8', sha: oldSha, repo: { id: repositoryId, full_name: repository } } }),
    rawPull({ head: { ref: 'agent/issue-7', sha: newSha, repo: { id: repositoryId, full_name: repository } } }),
    rawPull({ head: { ref: 'agent/issue-7', sha: oldSha, repo: { id: 999, full_name: 'attacker/repository' } } }),
    rawPull({ body: 'marker removed' }),
    rawPull({ user: { login: 'human', type: 'User' } }),
  ];

  for (const persisted of candidates) {
    const methods = [];
    const api = client(async (url, init) => {
      methods.push(init.method);
      if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
      if (init.method === 'POST') return response(rawPull());
      return response(persisted);
    });
    await assert.rejects(() => api.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' }), WriterGitHubApiError);
    assert.deepEqual(methods, ['GET', 'POST', 'GET']);
  }

  const malformedPost = client(async (url, init) => {
    if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
    return response({ number: '42' });
  });
  await assert.rejects(() => malformedPost.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' }), WriterGitHubApiError);

  const failedPost = client(async (url, init) => {
    if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
    return response({ message: 'server error' }, 500);
  });
  await assert.rejects(() => failedPost.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' }), /HTTP 500/);

  const failedGet = client(async (url, init) => {
    if (url.includes('/git/ref/heads/')) return response(rawRef(7, oldSha));
    if (init.method === 'POST') return response(rawPull());
    return response({ message: 'server error' }, 500);
  });
  await assert.rejects(() => failedGet.createDraftPull(7, { title: 'Fix issue 7', body: 'Details' }), /HTTP 500/);
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
    const api = client(async () => {
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
    const api = client(async (_url, init) => {
      methods.push(init.method);
      if (init.method === 'GET' && methods.length === 1) return response(rawPull());
      if (init.method === 'PATCH') return response(rawPull({ title: 'Updated' }));
      return response(after);
    });
    await assert.rejects(() => api.updateManagedDraftPull(7, 42, oldSha, { title: 'Updated' }), WriterGitHubApiError);
    assert.deepEqual(methods, ['GET', 'PATCH', 'GET']);
  }
});

test('createAgentRef is issue-bound and verifies absence, response, and persisted head', async () => {
  const calls = [];
  const api = client(async (url, init) => {
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

test('advanceAgentRef rejects stale heads and uses only a non-force issue-bound update', async () => {
  let staleCalls = 0;
  const stale = client(async () => {
    staleCalls += 1;
    return response(rawRef(7, newSha));
  });
  await assert.rejects(() => stale.advanceAgentRef(7, oldSha, newSha), /requested branch and SHA/);
  assert.equal(staleCalls, 1);

  const calls = [];
  const api = client(async (url, init) => {
    calls.push({ url, init });
    return response(calls.length === 1 ? rawRef(7, oldSha) : rawRef(7, newSha));
  });
  assert.deepEqual(await api.advanceAgentRef(7, oldSha, newSha), rawRef(7, newSha));
  assert.deepEqual(calls.map(({ url, init }) => ({
    method: init.method,
    path: new URL(url).pathname,
    body: init.body === undefined ? undefined : JSON.parse(init.body),
  })), [
    { method: 'GET', path: '/repos/example/repository/git/ref/heads/agent%2Fissue-7', body: undefined },
    {
      method: 'PATCH',
      path: '/repos/example/repository/git/refs/heads/agent%2Fissue-7',
      body: { sha: newSha, force: false },
    },
    { method: 'GET', path: '/repos/example/repository/git/ref/heads/agent%2Fissue-7', body: undefined },
  ]);
});

test('Writer invalid input and ref bounds fail before issuing a network call', async () => {
  let calls = 0;
  const api = client(async () => {
    calls += 1;
    return response({});
  });
  const invalidCalls = [
    () => api.getIssue(0),
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
  const api = client(async () => {
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

test('Writer constructor rejects ambiguous repositories, insecure origin customization, and incomplete identities', () => {
  const invalid = [
    { token: '', repository, repositoryId, writerApp },
    { token: 'contains\nnewline', repository, repositoryId, writerApp },
    { token: 'valid', repository: 'bad/repository/path', repositoryId, writerApp },
    { token: 'valid', repository: '-owner/repository', repositoryId, writerApp },
    { token: 'valid', repository: 'owner--name/repository', repositoryId, writerApp },
    { token: 'valid', repository: 'owner/.', repositoryId, writerApp },
    { token: 'valid', repository, repositoryId: 0, writerApp },
    { token: 'valid', repository, repositoryId, writerApp: { id: writerApp.id } },
    { token: 'valid', repository, repositoryId, writerApp: { id: writerApp.id, slug: 'Bad Slug' } },
    { token: 'valid', repository, repositoryId, writerApp, apiUrl: 'https://attacker.invalid' },
  ];
  for (const options of invalid) {
    assert.throws(() => new WriterGitHubClient({ ...options, fetchImpl: async () => response({}) }), WriterGitHubApiError);
  }
  assert.doesNotThrow(() => new WriterGitHubClient({
    token: 'valid',
    repository,
    repositoryId: 3_000_000_000,
    writerApp: { id: 3_000_000_001, slug: writerApp.slug },
    fetchImpl: async () => response({}),
  }));
});
