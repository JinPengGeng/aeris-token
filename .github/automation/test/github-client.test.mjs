import assert from 'node:assert/strict';
import test from 'node:test';

import { GitHubApiError, GitHubClient } from '../src/github-client.mjs';
import { findManagedComment, MANAGED_MARKER } from '../src/managed-comment.mjs';

const response = (payload, status = 200) =>
  new Response(payload === null ? null : JSON.stringify(payload), {
    status,
    headers: { 'content-type': 'application/json' },
  });

function client(fetchImpl) {
  return new GitHubClient({
    token: 'test-token',
    repository: 'example/repo',
    fetchImpl,
  });
}

test('listIssueComments follows pagination until a partial page', async () => {
  const calls = [];
  const pageOne = Array.from({ length: 100 }, (_, index) => ({ id: index + 1 }));
  const api = client(async (url) => {
    calls.push(url);
    return response(url.endsWith('page=1') ? pageOne : [{ id: 101 }]);
  });

  const comments = await api.listIssueComments(7);
  assert.equal(comments.length, 101);
  assert.equal(calls.length, 2);
  assert.match(calls[0], /per_page=100&page=1$/);
  assert.match(calls[1], /per_page=100&page=2$/);
});

test('listIssueComments fails closed when the lookup reaches its page limit', async () => {
  let calls = 0;
  const fullPage = Array.from({ length: 100 }, (_, index) => ({ id: index + 1 }));
  const api = client(async () => {
    calls += 1;
    return response(fullPage);
  });

  await assert.rejects(
    () => api.listIssueComments(7),
    (error) => error instanceof GitHubApiError && /too many comments/.test(error.message),
  );
  assert.equal(calls, 10);
});

test('listPullTimelineEvents follows pagination through the issues timeline endpoint', async () => {
  const calls = [];
  const pageOne = Array.from({ length: 100 }, (_, index) => ({ id: index + 1 }));
  const api = client(async (url) => {
    calls.push(url);
    return response(url.endsWith('page=1') ? pageOne : [{ id: 101 }]);
  });

  const events = await api.listPullTimelineEvents(42);
  assert.equal(events.length, 101);
  assert.equal(calls.length, 2);
  assert.match(calls[0], /\/repos\/example\/repo\/issues\/42\/timeline\?per_page=100&page=1$/);
  assert.match(calls[1], /\/repos\/example\/repo\/issues\/42\/timeline\?per_page=100&page=2$/);
});

test('listPullTimelineEvents fails closed when the timeline reaches its page limit', async () => {
  let calls = 0;
  const fullPage = Array.from({ length: 100 }, (_, index) => ({ id: index + 1 }));
  const api = client(async () => {
    calls += 1;
    return response(fullPage);
  });

  await assert.rejects(
    () => api.listPullTimelineEvents(42),
    (error) => error instanceof GitHubApiError && /too many timeline events/.test(error.message),
  );
  assert.equal(calls, 10);
});

test('listPullTimelineEvents rejects invalid pull numbers before calling GitHub', async () => {
  let calls = 0;
  const api = client(async () => {
    calls += 1;
    return response([]);
  });

  for (const invalidPullNumber of [0, -1, 1.5, Number.NaN, '42']) {
    await assert.rejects(
      () => api.listPullTimelineEvents(invalidPullNumber),
      (error) => error instanceof GitHubApiError && /Pull number is invalid/.test(error.message),
    );
  }
  assert.equal(calls, 0);
});

test('Writer governance API methods use fixed repository and writer Environment endpoints', async () => {
  const calls = [];
  const api = client(async (url) => {
    calls.push(url);
    if (url.includes('/collaborators?')) return response([]);
    if (url.includes('/rulesets?')) return response([]);
    if (url.endsWith('/rulesets/101')) return response({ id: 101 });
    if (url.endsWith('/actions/permissions/workflow')) return response({ default_workflow_permissions: 'read' });
    if (url.endsWith('/actions/permissions')) return response({ enabled: true });
    if (url.endsWith('/environments/writer')) return response({ name: 'writer' });
    if (url.includes('/deployment-branch-policies?')) {
      return response({ total_count: 1, branch_policies: [{ id: 1, name: 'main', type: 'branch' }] });
    }
    return response(null, 404);
  });

  assert.deepEqual(await api.listDirectCollaborators(), { items: [], truncated: false });
  assert.deepEqual(await api.listRepositoryRulesetsIncludingParents(), { items: [], truncated: false });
  assert.deepEqual(await api.getRepositoryRuleset(101), { id: 101 });
  assert.deepEqual(await api.getActionsPermissions(), { enabled: true });
  assert.deepEqual(await api.getDefaultWorkflowPermissions(), { default_workflow_permissions: 'read' });
  assert.deepEqual(await api.getWriterEnvironment(), { name: 'writer' });
  assert.deepEqual(await api.listWriterDeploymentBranchPolicies(), {
    items: [{ id: 1, name: 'main', type: 'branch' }], truncated: false,
  });

  assert.ok(calls.some((url) => /collaborators\?affiliation=direct&per_page=100&page=1$/.test(url)));
  assert.ok(calls.some((url) => /rulesets\?includes_parents=true&per_page=100&page=1$/.test(url)));
  assert.ok(calls.some((url) => /rulesets\/101$/.test(url)));
  assert.ok(calls.some((url) => /actions\/permissions\/workflow$/.test(url)));
  assert.ok(calls.some((url) => /environments\/writer$/.test(url)));
  assert.ok(calls.some((url) => /environments\/writer\/deployment-branch-policies\?per_page=100&page=1$/.test(url)));
});

test('Writer governance list methods expose bounded pagination truncation', async () => {
  const fullPage = Array.from({ length: 100 }, (_, index) => ({ id: index + 1 }));
  const api = client(async () => response(fullPage));
  assert.deepEqual(await api.listDirectCollaborators(), {
    items: Array.from({ length: 1_000 }, (_, index) => ({ id: (index % 100) + 1 })),
    truncated: true,
  });
  assert.deepEqual(await api.listRepositoryRulesetsIncludingParents(), {
    items: Array.from({ length: 1_000 }, (_, index) => ({ id: (index % 100) + 1 })),
    truncated: true,
  });
});

test('Writer deployment branch policy pagination is complete, bounded, and total-stable', async () => {
  const first = Array.from({ length: 100 }, (_, index) => ({ id: index + 1 }));
  let calls = 0;
  const api = client(async () => {
    calls += 1;
    return calls === 1
      ? response({ total_count: 101, branch_policies: first })
      : response({ total_count: 101, branch_policies: [{ id: 101 }] });
  });
  const complete = await api.listWriterDeploymentBranchPolicies();
  assert.equal(complete.items.length, 101);
  assert.equal(complete.truncated, false);

  calls = 0;
  const drifting = client(async () => {
    calls += 1;
    return response({ total_count: calls === 1 ? 101 : 102, branch_policies: calls === 1 ? first : [] });
  });
  await assert.rejects(
    () => drifting.listWriterDeploymentBranchPolicies(),
    (error) => error instanceof GitHubApiError && /total drifted/.test(error.message),
  );

  const full = client(async () => response({ total_count: 1_000, branch_policies: first }));
  assert.equal((await full.listWriterDeploymentBranchPolicies()).truncated, true);
});

test('unsupported Writer governance REST endpoints fail closed', async () => {
  const api = client(async () => response({ message: 'Not Found' }, 404));
  await assert.rejects(
    () => api.getWriterEnvironment(),
    (error) => error instanceof GitHubApiError && error.status === 404,
  );
});

test('managed comment writes use only the Issue Comments API', async () => {
  const calls = [];
  const api = client(async (url, init) => {
    calls.push({ url, init });
    return response({ id: 11, body: JSON.parse(init.body).body });
  });

  await api.createIssueComment(7, 'created');
  await api.updateIssueComment(11, 'updated');

  assert.equal(calls[0].init.method, 'POST');
  assert.match(calls[0].url, /\/repos\/example\/repo\/issues\/7\/comments$/);
  assert.deepEqual(JSON.parse(calls[0].init.body), { body: 'created' });
  assert.equal(calls[1].init.method, 'PATCH');
  assert.match(calls[1].url, /\/repos\/example\/repo\/issues\/comments\/11$/);
  assert.deepEqual(JSON.parse(calls[1].init.body), { body: 'updated' });
});

test('findManagedComment rejects multiple bot-owned managed comments', () => {
  const managed = (id) => ({
    id,
    body: `${MANAGED_MARKER}\nmanaged`,
    user: { login: 'github-actions[bot]' },
  });

  assert.throws(
    () => findManagedComment([managed(1), managed(2)]),
    /multiple bot-managed comments exist/,
  );
});

test('findManagedComment ignores forged markers from non-bot users', () => {
  assert.equal(
    findManagedComment([
      { id: 1, body: `${MANAGED_MARKER}\nforged`, user: { login: 'outside-user' } },
    ]),
    null,
  );
});

test('required-check API methods query the exact encoded commit ref', async () => {
  const calls = [];
  const api = client(async (url) => {
    calls.push(url);
    if (url.includes('/check-runs?')) return response({ total_count: 1, check_runs: [{ id: 1 }] });
    if (url.includes('/statuses?')) return response([{ id: 2 }]);
    return response(null, 404);
  });

  assert.deepEqual(await api.listCheckRunsForRef('head/ref'), [{ id: 1 }]);
  assert.deepEqual(await api.listCommitStatuses('head/ref'), [{ id: 2 }]);
  assert.match(calls[0], /commits\/head%2Fref\/check-runs\?filter=all&per_page=100&page=1$/);
  assert.match(calls[1], /commits\/head%2Fref\/statuses\?per_page=100&page=1$/);
});

test('check-run lookup rejects a malformed REST envelope', async () => {
  const api = client(async () => response([{ id: 1 }]));
  await assert.rejects(
    () => api.listCheckRunsForRef('head'),
    (error) => error instanceof GitHubApiError && /check-runs response is invalid/.test(error.message),
  );
});
