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
    if (url.includes('/check-runs?')) return response({ check_runs: [{ id: 1 }] });
    if (url.includes('/statuses?')) return response([{ id: 2 }]);
    return response(null, 404);
  });

  assert.deepEqual(await api.listCheckRunsForRef('head/ref'), [{ id: 1 }]);
  assert.deepEqual(await api.listCommitStatuses('head/ref'), [{ id: 2 }]);
  assert.match(calls[0], /commits\/head%2Fref\/check-runs\?filter=all&per_page=100&page=1$/);
  assert.match(calls[1], /commits\/head%2Fref\/statuses\?per_page=100&page=1$/);
});
