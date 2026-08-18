import assert from 'node:assert/strict';
import test from 'node:test';

import { PolicyGitHubClient } from '../src/policy-github-client.mjs';

const sha = (character) => character.repeat(40);
const repository = 'JinPengGeng/aeris-token';
const policyApp = { id: 9001, slug: 'aeris-token-policy' };

function artifact() {
  return {
    schema_version: 1,
    artifact_type: 'policy_evaluation',
    repository_id: 123,
    repository,
    pull_number: 37,
    head_sha: sha('a'),
    base_sha: sha('b'),
    policy_sha: sha('c'),
    snapshot_sha: 'd'.repeat(64),
    evaluated_at: '2026-08-18T12:00:00.000Z',
    result: {
      mode: 'shadow',
      verdict: 'pass',
      enforcement: 'advisory',
      eligible_for_automatic_merge: false,
      reason_codes: [],
      unsuccessful_checks: [],
      human_review_paths: [],
      changed_file_count: 1,
    },
  };
}

function generation() {
  const value = artifact();
  return {
    repository_id: value.repository_id,
    repository: value.repository,
    pull_number: value.pull_number,
    head_sha: value.head_sha,
    base_sha: value.base_sha,
    policy_sha: value.policy_sha,
  };
}

function pull(head = sha('a')) {
  return {
    number: 37,
    state: 'open',
    draft: false,
    mergeable: true,
    labels: [],
    head: { sha: head, ref: 'agent/issue-11', repo: { full_name: repository } },
    base: { sha: sha('b'), ref: 'main', repo: { full_name: repository } },
  };
}

function response(value, status = 200) {
  return new Response(value === null ? '' : JSON.stringify(value), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function client(fetchImpl, app = policyApp) {
  return new PolicyGitHubClient({ token: 'installation-token', repository, repositoryId: 123, policyApp: app, fetchImpl });
}

test('read client binds comparison and review-thread responses to the requested repository and PR', async () => {
  const calls = [];
  const value = client(async (url, options) => {
    calls.push({ url, options });
    if (url.includes('/compare/')) {
      return response({ status: 'ahead', base_commit: { sha: sha('b') }, commits: [{ sha: sha('a') }] });
    }
    if (url.endsWith('/graphql')) {
      return response({ data: { repository: { databaseId: 123, pullRequest: {
        number: 37,
        headRefOid: sha('a'),
        baseRefOid: sha('b'),
        reviewThreads: { nodes: [{ isResolved: false }, { isResolved: true }], pageInfo: { hasNextPage: false, endCursor: null } },
      } } } });
    }
    throw new Error(`unexpected request ${url}`);
  });
  assert.deepEqual(await value.compare(sha('b'), sha('a')), { base_sha: sha('b'), head_sha: sha('a'), status: 'ahead' });
  assert.deepEqual(await value.listReviewThreads(37), {
    unresolved: 1,
    truncated: false,
    head_sha: sha('a'),
    base_sha: sha('b'),
  });
  assert.ok(calls.every((call) => call.options.redirect === 'error'));
  assert.ok(calls.every((call) => call.options.headers.authorization === 'Bearer installation-token'));
});

test('comparison rejects a response bound to different commits', async () => {
  const value = client(async () => response({ status: 'ahead', base_commit: { sha: sha('c') }, commits: [{ sha: sha('a') }] }));
  await assert.rejects(() => value.compare(sha('b'), sha('a')), /not bound/);
});

test('pull lookup rejects a REST response for a different pull request number', async () => {
  const value = client(async () => response({ ...pull(), number: 38 }));
  await assert.rejects(() => value.getPull(37), /not bound to the requested pull request number/);
});

test('review-thread lookup fails closed on GraphQL errors and repository drift', async () => {
  const errorClient = client(async () => response({ errors: [{ message: 'denied' }] }));
  await assert.rejects(() => errorClient.listReviewThreads(37), /returned errors/);

  const driftClient = client(async () => response({ data: { repository: { databaseId: 999, pullRequest: null } } }));
  await assert.rejects(() => driftClient.listReviewThreads(37), /invalid/);
});

test('policy check begins in-progress, App-owned, and exact-head fenced', async () => {
  const requests = [];
  const external = `aeris-policy:v1:123:37:${sha('a')}:${sha('c')}`;
  const output = {
    title: 'Policy evaluation in progress',
    summary: `Policy inputs are being revalidated.\nHead SHA: ${sha('a')}\nPolicy SHA: ${sha('c')}`,
  };
  let pullReads = 0;
  const value = client(async (url, options) => {
    requests.push({ url, options });
    if (url.endsWith('/pulls/37')) {
      pullReads += 1;
      return response(pull());
    }
    if (url.includes(`/commits/${sha('a')}/check-runs`)) return response({ check_runs: [] });
    if (url.endsWith('/check-runs') && options.method === 'POST') {
      const body = JSON.parse(options.body);
      assert.equal(body.external_id, external);
      assert.equal(body.status, 'in_progress');
      assert.equal(Object.hasOwn(body, 'conclusion'), false);
      return response({ id: 77, ...body, app: policyApp });
    }
    if (url.endsWith('/check-runs/77')) {
      return response({ id: 77, name: 'Automation Policy / gate', head_sha: sha('a'), external_id: external,
        status: 'in_progress', conclusion: null, app: policyApp, output });
    }
    throw new Error(`unexpected request ${url}`);
  });
  const result = await value.beginPolicyCheck(generation(), 'Automation Policy / gate', 'https://github.com/run/1');
  assert.equal(result.id, 77);
  assert.equal(result.app.id, policyApp.id);
  assert.equal(pullReads, 2);
  assert.equal(requests.filter((request) => request.options.method === 'POST' && request.url.endsWith('/check-runs')).length, 1);
});

test('policy check completion requires the exact in-progress App-owned generation', async () => {
  const external = `aeris-policy:v1:123:37:${sha('a')}:${sha('c')}`;
  const pendingOutput = {
    title: 'Policy evaluation in progress',
    summary: `Policy inputs are being revalidated.\nHead SHA: ${sha('a')}\nPolicy SHA: ${sha('c')}`,
  };
  const completedOutput = {
    title: 'Policy shadow: pass',
    summary: `Mode: shadow\nVerdict: pass\nEnforcement: advisory\nAutomatic merge eligible: no\nHead SHA: ${sha('a')}\nPolicy SHA: ${sha('c')}\nSnapshot SHA: ${'d'.repeat(64)}\nChanged files: 1`,
  };
  let patched = false;
  let checkReads = 0;
  const value = client(async (url, options) => {
    if (url.endsWith('/pulls/37')) return response(pull());
    if (url.endsWith('/check-runs/77') && options.method === 'PATCH') {
      patched = true;
      const body = JSON.parse(options.body);
      assert.equal(body.status, 'completed');
      assert.equal(body.conclusion, 'neutral');
      return response({ id: 77, head_sha: sha('a'), app: policyApp, ...body });
    }
    if (url.endsWith('/check-runs/77')) {
      checkReads += 1;
      return response({
        id: 77,
        name: 'Automation Policy / gate',
        head_sha: sha('a'),
        external_id: external,
        status: checkReads === 1 ? 'in_progress' : 'completed',
        conclusion: checkReads === 1 ? null : 'neutral',
        app: policyApp,
        output: checkReads === 1 ? pendingOutput : completedOutput,
      });
    }
    throw new Error(`unexpected request ${url}`);
  });
  await value.completePolicyCheck(77, artifact(), 'Automation Policy / gate');
  assert.equal(patched, true);
});

test('policy check recovery restores in-progress without trusting stale pull state', async () => {
  const external = `aeris-policy:v1:123:37:${sha('a')}:${sha('c')}`;
  const pendingOutput = {
    title: 'Policy evaluation in progress',
    summary: `Policy inputs are being revalidated.\nHead SHA: ${sha('a')}\nPolicy SHA: ${sha('c')}`,
  };
  let patched = false;
  const value = client(async (url, options) => {
    assert.equal(url.includes('/pulls/'), false);
    if (url.endsWith('/check-runs/77') && options.method === 'PATCH') {
      patched = true;
      const body = JSON.parse(options.body);
      assert.equal(body.status, 'in_progress');
      return response({ id: 77, head_sha: sha('a'), app: policyApp, ...body });
    }
    if (url.endsWith('/check-runs/77')) {
      return response({ id: 77, name: 'Automation Policy / gate', head_sha: sha('a'), external_id: external,
        status: 'in_progress', conclusion: null, app: policyApp, output: pendingOutput });
    }
    throw new Error(`unexpected request ${url}`);
  });
  const result = await value.restorePolicyCheckInProgress(77, generation(), 'Automation Policy / gate');
  assert.equal(result.status, 'in_progress');
  assert.equal(patched, true);
});

test('policy check recovery verifies the completed App-owned generation before patching it', async () => {
  let patched = false;
  const value = client(async (url, options) => {
    if (url.endsWith('/check-runs/77') && options.method === 'PATCH') {
      patched = true;
      throw new Error('recovery must not patch an unbound check');
    }
    if (url.endsWith('/check-runs/77')) {
      return response({
        id: 77,
        name: 'Automation Policy / gate',
        head_sha: sha('a'),
        external_id: 'aeris-policy:v1:123:37:wrong',
        status: 'completed',
        conclusion: 'neutral',
        app: policyApp,
      });
    }
    throw new Error(`unexpected request ${url}`);
  });
  await assert.rejects(
    () => value.restorePolicyCheckInProgress(77, generation(), 'Automation Policy / gate'),
    /expected recoverable App-owned generation/,
  );
  assert.equal(patched, false);
});

test('policy check begin reuses the exact generation and rejects stale or impersonated state', async () => {
  const external = `aeris-policy:v1:123:37:${sha('a')}:${sha('c')}`;
  const pendingOutput = {
    title: 'Policy evaluation in progress',
    summary: `Policy inputs are being revalidated.\nHead SHA: ${sha('a')}\nPolicy SHA: ${sha('c')}`,
  };
  let patched = false;
  const reused = client(async (url, options) => {
    if (url.endsWith('/pulls/37')) return response(pull());
    if (url.includes(`/commits/${sha('a')}/check-runs`)) {
      return response({ check_runs: [{ id: 77, name: 'Automation Policy / gate', head_sha: sha('a'), external_id: external, app: policyApp }] });
    }
    if (url.endsWith('/check-runs/77') && options.method === 'PATCH') {
      patched = true;
      const body = JSON.parse(options.body);
      assert.equal(Object.hasOwn(body, 'head_sha'), false);
      return response({ id: 77, head_sha: sha('a'), app: policyApp, ...body });
    }
    if (url.endsWith('/check-runs/77')) {
      return response({ id: 77, name: 'Automation Policy / gate', head_sha: sha('a'), external_id: external,
        status: 'in_progress', conclusion: null, app: policyApp, output: pendingOutput });
    }
    throw new Error(`unexpected request ${url}`);
  });
  await reused.beginPolicyCheck(generation(), 'Automation Policy / gate');
  assert.equal(patched, true);

  const stale = client(async (url) => {
    if (url.endsWith('/pulls/37')) return response(pull(sha('d')));
    throw new Error(`unexpected request ${url}`);
  });
  await assert.rejects(() => stale.beginPolicyCheck(generation(), 'Automation Policy / gate'), /stale/);

  const impersonated = client(async (url) => {
    if (url.endsWith('/pulls/37')) return response(pull());
    if (url.includes('/check-runs')) return response({ check_runs: [{ id: 8, name: 'Automation Policy / gate', external_id: external, app: { id: 2, slug: 'other' } }] });
    throw new Error(`unexpected request ${url}`);
  });
  await assert.rejects(() => impersonated.beginPolicyCheck(generation(), 'Automation Policy / gate'), /non-Policy App/);

  const wrongPersisted = client(async (url, options) => {
    if (url.endsWith('/pulls/37')) return response(pull());
    if (url.includes(`/commits/${sha('a')}/check-runs`)) return response({ check_runs: [] });
    if (url.endsWith('/check-runs') && options.method === 'POST') return response({ id: 9 });
    if (url.endsWith('/check-runs/9')) return response({ id: 9, name: 'Automation Policy / gate', head_sha: sha('a'), external_id: external,
      status: 'in_progress', conclusion: null, app: { id: 2, slug: 'other' }, output: pendingOutput });
    throw new Error(`unexpected request ${url}`);
  });
  await assert.rejects(() => wrongPersisted.beginPolicyCheck(generation(), 'Automation Policy / gate'), /not owned/);
});

test('client exposes no generic request or merge surface', () => {
  const value = client(async () => response(null, 204));
  const methods = Object.getOwnPropertyNames(Object.getPrototypeOf(value)).filter((name) => name !== 'constructor').sort();
  assert.deepEqual(methods, [
    'beginPolicyCheck',
    'compare',
    'completePolicyCheck',
    'getBranchHead',
    'getPull',
    'getRepository',
    'listCheckRunsForRef',
    'listPullFiles',
    'listReviewThreads',
    'restorePolicyCheckInProgress',
  ]);
});
