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

function client(fetchImpl, app = policyApp, options = {}) {
  return new PolicyGitHubClient({ token: 'installation-token', repository, repositoryId: 123, policyApp: app, fetchImpl, ...options });
}

test('GitHub requests hard-abort when fetch does not settle before the deadline', async () => {
  let signal;
  const value = client((_url, options) => {
    signal = options.signal;
    return new Promise(() => {});
  }, policyApp, { requestTimeoutMs: 100 });
  await assert.rejects(() => value.getRepository(), /timed out/);
  assert.equal(signal.aborted, true);
});

test('streaming GitHub responses are cancelled immediately above two MiB', async () => {
  let cancelled = false;
  const megabyte = new Uint8Array(1024 * 1024);
  const stream = new ReadableStream({
    start(controller) {
      controller.enqueue(megabyte);
      controller.enqueue(megabyte);
      controller.enqueue(new Uint8Array([1]));
    },
    cancel() {
      cancelled = true;
    },
  });
  const value = client(async () => new Response(stream, { status: 200 }));
  await assert.rejects(() => value.getRepository(), /exceeds the configured limit/);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(cancelled, true);
});

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
  const state = new Map();
  const value = client(async (url, options) => {
    requests.push({ url, options });
    if (url.endsWith('/pulls/37')) {
      pullReads += 1;
      return response(pull());
    }
    if (url.includes(`/commits/${sha('a')}/check-runs`)) return response({ check_runs: [...state.values()] });
    if (url.endsWith('/check-runs') && options.method === 'POST') {
      const body = JSON.parse(options.body);
      assert.equal(body.external_id, external);
      assert.equal(body.status, 'in_progress');
      assert.equal(Object.hasOwn(body, 'conclusion'), false);
      const created = { id: 77, ...body, conclusion: null, app: policyApp };
      state.set(77, created);
      return response(created);
    }
    throw new Error(`unexpected request ${url}`);
  });
  const result = await value.beginPolicyCheck(generation(), 'Automation Policy / gate', 'https://github.com/run/1');
  assert.equal(result.id, 77);
  assert.equal(result.app.id, policyApp.id);
  assert.equal(pullReads, 2);
  assert.equal(requests.filter((request) => request.options.method === 'POST' && request.url.endsWith('/check-runs')).length, 1);
});

test('policy check begin creates a new check instead of reopening completed history', async () => {
  const external = `aeris-policy:v1:123:37:${sha('a')}:${sha('c')}`;
  const pendingOutput = {
    title: 'Policy evaluation in progress',
    summary: `Policy inputs are being revalidated.\nHead SHA: ${sha('a')}\nPolicy SHA: ${sha('c')}`,
  };
  const mutations = [];
  let pullReads = 0;
  const state = new Map([[77, { id: 77, name: 'Automation Policy / gate', head_sha: sha('a'), external_id: external,
    status: 'completed', conclusion: 'success', app: policyApp }]]);
  const value = client(async (url, options) => {
    if (url.endsWith('/pulls/37')) {
      pullReads += 1;
      return response(pull());
    }
    if (url.includes(`/commits/${sha('a')}/check-runs`)) {
      return response({ check_runs: [...state.values()] });
    }
    if (url.endsWith('/check-runs') && options.method === 'POST') {
      mutations.push(options.method);
      const created = { id: 88, app: policyApp, ...JSON.parse(options.body), conclusion: null };
      state.set(88, created);
      return response(created);
    }
    if (options.method === 'PATCH') throw new Error('completed check must not be reopened');
    throw new Error(`unexpected request ${url}`);
  });
  const result = await value.beginPolicyCheck(generation(), 'Automation Policy / gate');
  assert.equal(result.id, 88);
  assert.deepEqual(mutations, ['POST']);
  assert.equal(pullReads, 2);
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

test('policy check recovery creates a higher successor without trusting stale pull state', async () => {
  const external = `aeris-policy:v1:123:37:${sha('a')}:${sha('c')}`;
  const pendingOutput = {
    title: 'Policy evaluation in progress',
    summary: `Policy inputs are being revalidated.\nHead SHA: ${sha('a')}\nPolicy SHA: ${sha('c')}`,
  };
  const state = new Map([[77, { id: 77, name: 'Automation Policy / gate', head_sha: sha('a'), external_id: external,
    status: 'in_progress', conclusion: null, app: policyApp, output: pendingOutput }]]);
  const methods = [];
  const value = client(async (url, options) => {
    assert.equal(url.includes('/pulls/'), false);
    if (url.endsWith('/check-runs/77')) return response(state.get(77));
    if (url.includes(`/commits/${sha('a')}/check-runs`)) return response({ check_runs: [...state.values()] });
    if (url.endsWith('/check-runs') && options.method === 'POST') {
      methods.push('POST');
      const created = { id: 88, app: policyApp, ...JSON.parse(options.body), conclusion: null };
      state.set(88, created);
      return response(created);
    }
    if (options.method === 'PATCH') throw new Error('recovery must not PATCH');
    throw new Error(`unexpected request ${url}`);
  });
  const result = await value.restorePolicyCheckInProgress(77, generation(), 'Automation Policy / gate');
  assert.equal(result.id, 88);
  assert.deepEqual(methods, ['POST']);
});

test('policy check recovery creates a successor when the source completes before fresh-list publication', async () => {
  const external = `aeris-policy:v1:123:37:${sha('a')}:${sha('c')}`;
  const pendingOutput = {
    title: 'Policy evaluation in progress',
    summary: `Policy inputs are being revalidated.\nHead SHA: ${sha('a')}\nPolicy SHA: ${sha('c')}`,
  };
  const state = new Map([[77, { id: 77, name: 'Automation Policy / gate', head_sha: sha('a'), external_id: external,
    status: 'in_progress', conclusion: null, app: policyApp, output: pendingOutput }]]);
  let listReads = 0;
  const value = client(async (url, options) => {
    if (url.endsWith('/check-runs/77')) return response(state.get(77));
    if (url.includes(`/commits/${sha('a')}/check-runs`)) {
      listReads += 1;
      if (listReads === 1) state.set(77, { ...state.get(77), status: 'completed', conclusion: 'success' });
      return response({ check_runs: [...state.values()] });
    }
    if (url.endsWith('/check-runs') && options.method === 'POST') {
      const created = { id: 88, app: policyApp, ...JSON.parse(options.body), conclusion: null };
      state.set(88, created);
      return response(created);
    }
    if (options.method === 'PATCH') throw new Error('completed check must not be reopened');
    throw new Error(`unexpected request ${url}`);
  });
  const result = await value.restorePolicyCheckInProgress(77, generation(), 'Automation Policy / gate');
  assert.equal(result.id, 88);
  assert.equal(state.get(77).status, 'completed');
});

test('policy check recovery verifies the completed App-owned generation before replacing it', async () => {
  let mutated = false;
  const value = client(async (url, options) => {
    if (options.method === 'PATCH' || options.method === 'POST') {
      mutated = true;
      throw new Error('recovery must not mutate an unbound check');
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
  assert.equal(mutated, false);
});

test('policy check recovery replaces a completed generation without reopening it', async () => {
  const external = `aeris-policy:v1:123:37:${sha('a')}:${sha('c')}`;
  const pendingOutput = {
    title: 'Policy evaluation in progress',
    summary: `Policy inputs are being revalidated.\nHead SHA: ${sha('a')}\nPolicy SHA: ${sha('c')}`,
  };
  const methods = [];
  const state = new Map([[77, { id: 77, name: 'Automation Policy / gate', head_sha: sha('a'), external_id: external,
    status: 'completed', conclusion: 'success', app: policyApp }]]);
  const value = client(async (url, options) => {
    if (url.endsWith('/check-runs/77')) return response(state.get(77));
    if (url.includes(`/commits/${sha('a')}/check-runs`)) return response({ check_runs: [...state.values()] });
    if (url.endsWith('/check-runs') && options.method === 'POST') {
      methods.push(options.method);
      const body = JSON.parse(options.body);
      assert.equal(body.status, 'in_progress');
      assert.equal(body.head_sha, sha('a'));
      const created = { id: 88, app: policyApp, ...body, conclusion: null };
      state.set(88, created);
      return response(created);
    }
    throw new Error(`unexpected request ${url}`);
  });
  const result = await value.restorePolicyCheckInProgress(77, generation(), 'Automation Policy / gate');
  assert.equal(result.id, 88);
  assert.deepEqual(methods, ['POST']);
});

test('policy check successor adopts an applied POST whose response is lost', async () => {
  const external = `aeris-policy:v1:123:37:${sha('a')}:${sha('c')}`;
  const state = new Map();
  let posts = 0;
  const value = client(async (url, options) => {
    if (url.endsWith('/pulls/37')) return response(pull());
    if (url.includes(`/commits/${sha('a')}/check-runs`)) return response({ check_runs: [...state.values()] });
    if (url.endsWith('/check-runs') && options.method === 'POST') {
      posts += 1;
      const created = { id: 88, app: policyApp, ...JSON.parse(options.body), conclusion: null };
      state.set(created.id, created);
      throw new Error('connection reset after POST was applied');
    }
    throw new Error(`unexpected request ${url}`);
  });

  const result = await value.beginPolicyCheck(generation(), 'Automation Policy / gate');
  assert.equal(result.id, 88);
  assert.equal(posts, 1);
});

test('policy check successor retries an ambiguous POST that was not applied', async () => {
  const state = new Map();
  let posts = 0;
  const value = client(async (url, options) => {
    if (url.endsWith('/pulls/37')) return response(pull());
    if (url.includes(`/commits/${sha('a')}/check-runs`)) return response({ check_runs: [...state.values()] });
    if (url.endsWith('/check-runs') && options.method === 'POST') {
      posts += 1;
      if (posts === 1) throw new Error('connection reset before POST was applied');
      const created = { id: 88, app: policyApp, ...JSON.parse(options.body), conclusion: null };
      state.set(created.id, created);
      return response(created);
    }
    throw new Error(`unexpected request ${url}`);
  });

  const result = await value.beginPolicyCheck(generation(), 'Automation Policy / gate');
  assert.equal(result.id, 88);
  assert.equal(posts, 2);
});

test('policy check successor retries when the first created check completes before adoption', async () => {
  const state = new Map();
  let posts = 0;
  const value = client(async (url, options) => {
    if (url.endsWith('/pulls/37')) return response(pull());
    if (url.includes(`/commits/${sha('a')}/check-runs`)) return response({ check_runs: [...state.values()] });
    if (url.endsWith('/check-runs') && options.method === 'POST') {
      posts += 1;
      const body = JSON.parse(options.body);
      const created = posts === 1
        ? { id: 88, app: policyApp, ...body, status: 'completed', conclusion: 'neutral' }
        : { id: 99, app: policyApp, ...body, conclusion: null };
      state.set(created.id, created);
      return response(created);
    }
    throw new Error(`unexpected request ${url}`);
  });

  const result = await value.beginPolicyCheck(generation(), 'Automation Policy / gate');
  assert.equal(result.id, 99);
  assert.equal(posts, 2);
  assert.equal(state.get(88).status, 'completed');
});

test('policy check begin rejects a higher-ID fresh-list identity conflict before POST', async () => {
  const external = `aeris-policy:v1:123:37:${sha('a')}:${sha('c')}`;
  const pendingOutput = {
    title: 'Policy evaluation in progress',
    summary: `Policy inputs are being revalidated.\nHead SHA: ${sha('a')}\nPolicy SHA: ${sha('c')}`,
  };
  let lists = 0;
  let mutated = false;
  const value = client(async (url, options) => {
    if (url.endsWith('/pulls/37')) return response(pull());
    if (url.includes(`/commits/${sha('a')}/check-runs`)) {
      lists += 1;
      const checks = [{ id: 77, name: 'Automation Policy / gate', head_sha: sha('a'), external_id: external,
        status: 'in_progress', conclusion: null, app: policyApp, output: pendingOutput }];
      if (lists > 1) checks.push({ ...checks[0], id: 88, app: { id: 2, slug: 'other-app' } });
      return response({ check_runs: checks });
    }
    if (options.method === 'POST' || options.method === 'PATCH') mutated = true;
    throw new Error(`unexpected request ${url}`);
  });

  await assert.rejects(
    () => value.beginPolicyCheck(generation(), 'Automation Policy / gate', null, 77),
    /unexpected name, head, or App/,
  );
  assert.equal(mutated, false);
});

test('policy check begin rejects a higher-ID managed check injected after the expected fence', async () => {
  const external = `aeris-policy:v1:123:37:${sha('a')}:${sha('c')}`;
  const pendingOutput = {
    title: 'Policy evaluation in progress',
    summary: `Policy inputs are being revalidated.\nHead SHA: ${sha('a')}\nPolicy SHA: ${sha('c')}`,
  };
  let lists = 0;
  let mutated = false;
  const value = client(async (url, options) => {
    if (url.endsWith('/pulls/37')) return response(pull());
    if (url.includes(`/commits/${sha('a')}/check-runs`)) {
      lists += 1;
      const checks = [{ id: 77, name: 'Automation Policy / gate', head_sha: sha('a'), external_id: external,
        status: 'in_progress', conclusion: null, app: policyApp, output: pendingOutput }];
      if (lists > 1) checks.push({ ...checks[0], id: 88 });
      return response({ check_runs: checks });
    }
    if (options.method === 'POST' || options.method === 'PATCH') mutated = true;
    throw new Error(`unexpected request ${url}`);
  });

  await assert.rejects(
    () => value.beginPolicyCheck(generation(), 'Automation Policy / gate', null, 77),
    /not the dominant in-progress check on the fresh check list/,
  );
  assert.equal(mutated, false);
});

test('policy check completion timeout is followed by a higher pending successor', async () => {
  const external = `aeris-policy:v1:123:37:${sha('a')}:${sha('c')}`;
  const pendingOutput = {
    title: 'Policy evaluation in progress',
    summary: `Policy inputs are being revalidated.\nHead SHA: ${sha('a')}\nPolicy SHA: ${sha('c')}`,
  };
  const state = new Map([[77, { id: 77, name: 'Automation Policy / gate', head_sha: sha('a'), external_id: external,
    status: 'in_progress', conclusion: null, app: policyApp, output: pendingOutput }]]);
  const value = client(async (url, options) => {
    if (url.endsWith('/pulls/37')) return response(pull());
    if (url.endsWith('/check-runs/77') && options.method === 'PATCH') {
      state.set(77, { ...state.get(77), status: 'completed', conclusion: 'neutral' });
      return new Promise(() => {});
    }
    if (url.endsWith('/check-runs/77')) return response(state.get(77));
    if (url.includes(`/commits/${sha('a')}/check-runs`)) return response({ check_runs: [...state.values()] });
    if (url.endsWith('/check-runs') && options.method === 'POST') {
      const created = { id: 88, app: policyApp, ...JSON.parse(options.body), conclusion: null };
      state.set(created.id, created);
      return response(created);
    }
    throw new Error(`unexpected request ${url}`);
  }, policyApp, { requestTimeoutMs: 100 });

  await assert.rejects(
    () => value.completePolicyCheck(77, artifact(), 'Automation Policy / gate'),
    /timed out/,
  );
  const successor = await value.restorePolicyCheckInProgress(
    77,
    generation(),
    'Automation Policy / gate',
    null,
    true,
  );
  assert.equal(successor.id, 88);
  assert.equal(state.get(77).status, 'completed');
  assert.equal(state.get(88).status, 'in_progress');
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
      return response({ check_runs: [{ id: 77, name: 'Automation Policy / gate', head_sha: sha('a'), external_id: external,
        status: 'in_progress', conclusion: null, app: policyApp, output: pendingOutput }] });
    }
    if (url.endsWith('/check-runs/77') && options.method === 'PATCH') {
      patched = true;
      const body = JSON.parse(options.body);
      assert.equal(Object.hasOwn(body, 'head_sha'), false);
      assert.equal(Object.hasOwn(body, 'status'), false);
      assert.equal(Object.hasOwn(body, 'external_id'), false);
      return response({ id: 77, head_sha: sha('a'), app: policyApp, ...body });
    }
    if (url.endsWith('/check-runs/77')) {
      return response({ id: 77, name: 'Automation Policy / gate', head_sha: sha('a'), external_id: external,
        status: 'in_progress', conclusion: null, app: policyApp, output: pendingOutput });
    }
    throw new Error(`unexpected request ${url}`);
  });
  await reused.beginPolicyCheck(generation(), 'Automation Policy / gate', null, 77);
  assert.equal(patched, false);
  await assert.rejects(
    () => reused.beginPolicyCheck(generation(), 'Automation Policy / gate', null, 76),
    /Expected early Policy fence is not the dominant in-progress check/,
  );

  let duplicateMutated = false;
  const duplicateCurrent = client(async (url, options) => {
    if (url.endsWith('/pulls/37')) return response(pull());
    if (url.includes(`/commits/${sha('a')}/check-runs`)) {
      return response({ check_runs: [88, 77].map((id) => ({
        id,
        name: 'Automation Policy / gate',
        head_sha: sha('a'),
        external_id: external,
        status: 'in_progress',
        conclusion: null,
        app: policyApp,
        output: pendingOutput,
      })) });
    }
    if (url.endsWith('/check-runs') && options.method === 'POST') {
      duplicateMutated = true;
      return response({ id: 99, app: policyApp, ...JSON.parse(options.body) });
    }
    if (options.method === 'PATCH') duplicateMutated = true;
    throw new Error(`unexpected request ${url}`);
  });
  const duplicateResult = await duplicateCurrent.beginPolicyCheck(generation(), 'Automation Policy / gate', null, 88);
  assert.equal(duplicateResult.id, 88);
  assert.equal(duplicateMutated, false);

  const stale = client(async (url) => {
    if (url.endsWith('/pulls/37')) return response(pull(sha('d')));
    throw new Error(`unexpected request ${url}`);
  });
  await assert.rejects(() => stale.beginPolicyCheck(generation(), 'Automation Policy / gate'), /stale/);

  const impersonated = client(async (url) => {
    if (url.endsWith('/pulls/37')) return response(pull());
    if (url.includes('/check-runs')) return response({ check_runs: [{ id: 8, name: 'Automation Policy / gate', head_sha: sha('a'), external_id: external, app: { id: 2, slug: 'other' } }] });
    throw new Error(`unexpected request ${url}`);
  });
  await assert.rejects(() => impersonated.beginPolicyCheck(generation(), 'Automation Policy / gate'), /unexpected name, head, or App/);

  let conflictingNameMutated = false;
  const conflictingName = client(async (url, options) => {
    if (url.endsWith('/pulls/37')) return response(pull());
    if (url.includes(`/commits/${sha('a')}/check-runs`)) return response({ check_runs: [{
      id: 8, name: 'Different check', head_sha: sha('a'), external_id: external,
      status: 'in_progress', conclusion: null, app: policyApp,
    }] });
    if (options.method === 'POST' || options.method === 'PATCH') conflictingNameMutated = true;
    throw new Error(`unexpected request ${url}`);
  });
  await assert.rejects(
    () => conflictingName.beginPolicyCheck(generation(), 'Automation Policy / gate'),
    /unexpected name, head, or App/,
  );
  assert.equal(conflictingNameMutated, false);

  const wrongPersisted = client(async (url, options) => {
    if (url.endsWith('/pulls/37')) return response(pull());
    if (url.includes(`/commits/${sha('a')}/check-runs`)) return response({ check_runs: [{ id: 9,
      name: 'Automation Policy / gate', head_sha: sha('a'), external_id: external,
      status: 'in_progress', conclusion: null, app: { id: 2, slug: 'other' }, output: pendingOutput }] });
    if (url.endsWith('/check-runs') && options.method === 'POST') return response({ id: 9 });
    throw new Error(`unexpected request ${url}`);
  });
  await assert.rejects(() => wrongPersisted.beginPolicyCheck(generation(), 'Automation Policy / gate'), /unexpected name, head, or App/);
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
