import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import yaml from 'js-yaml';

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, '..', '..', '..');

function read(relativePath) {
  const source = fs.readFileSync(path.join(repoRoot, relativePath), 'utf8');
  return { source, document: yaml.load(source) };
}

function serialize(value) {
  return JSON.stringify(value);
}

function collectUses(value, output = []) {
  if (Array.isArray(value)) {
    for (const item of value) collectUses(item, output);
  } else if (value && typeof value === 'object') {
    for (const [key, child] of Object.entries(value)) {
      if (key === 'uses' && typeof child === 'string') output.push(child);
      collectUses(child, output);
    }
  }
  return output;
}

async function runDiscovery(eventName, payload, request = async () => {
  throw new Error('unexpected GitHub API request');
}) {
  const { document } = read('.github/workflows/automation-policy-gate.yml');
  const script = document.jobs.discover.steps.find((step) => step.id === 'targets').with.script;
  let targets = null;
  const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
  const execute = new AsyncFunction('github', 'context', 'core', script);
  await execute(
    { request },
    { eventName, payload, repo: { owner: 'JinPengGeng', repo: 'aeris-token' } },
    {
      setOutput(name, value) {
        assert.equal(name, 'targets');
        targets = JSON.parse(value);
      },
    },
  );
  return targets;
}

async function runEarlyFence(request) {
  const { document } = read('.github/workflows/automation-policy-gate.yml');
  const script = document.jobs.publish.steps.find((step) => step.id === 'early_fence').with.script;
  const prior = {
    AERIS_POLICY_APP_ID: process.env.AERIS_POLICY_APP_ID,
    AERIS_POLICY_APP_SLUG: process.env.AERIS_POLICY_APP_SLUG,
    AERIS_PULL_REQUEST_NUMBER: process.env.AERIS_PULL_REQUEST_NUMBER,
    AERIS_REPOSITORY_ID: process.env.AERIS_REPOSITORY_ID,
  };
  Object.assign(process.env, {
    AERIS_POLICY_APP_ID: '9001',
    AERIS_POLICY_APP_SLUG: 'aeris-token-policy',
    AERIS_PULL_REQUEST_NUMBER: '37',
    AERIS_REPOSITORY_ID: '123',
  });
  const outputs = {};
  try {
    const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
    await new AsyncFunction('github', 'context', 'core', script)(
      { request },
      {
        repo: { owner: 'JinPengGeng', repo: 'aeris-token' },
        payload: { repository: { id: 123, full_name: 'JinPengGeng/aeris-token' } },
        serverUrl: 'https://github.com',
        runId: 99,
      },
      { setOutput(name, value) { outputs[name] = value; } },
    );
  } finally {
    for (const [name, value] of Object.entries(prior)) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
  }
  return outputs;
}

test('Policy Signal has no token permissions, checkout, or Secret surface', () => {
  const { source, document } = read('.github/workflows/policy-signal.yml');
  assert.deepEqual(document.permissions, {});
  assert.deepEqual(Object.keys(document.jobs), ['signal']);
  assert.equal(collectUses(document.jobs).length, 0);
  assert.doesNotMatch(source, /secrets\.|github\.token|GITHUB_TOKEN|pull_request_target/);
  assert.doesNotMatch(source, /checkout@/);
});

test('Policy Gate reacts only through trusted reconciliation events', () => {
  const { source, document } = read('.github/workflows/automation-policy-gate.yml');
  assert.deepEqual(document.on.workflow_run.workflows, ['Rust CI', 'Frontend CI', 'Policy Signal']);
  assert.deepEqual(document.on.workflow_run.types, ['completed']);
  assert.ok(document.on.push.branches.includes('main'));
  assert.ok(Array.isArray(document.on.schedule));
  assert.ok(document.on.workflow_dispatch);
  assert.doesNotMatch(source, /pull_request_target/);
  assert.doesNotMatch(source, /AERIS_AI_API_KEY|AERIS_AI_MODEL/);
});

test('workflow_run discovery emits every unique associated PR and ignores stale event heads', async () => {
  const targets = await runDiscovery('workflow_run', {
    repository: { id: 123, full_name: 'JinPengGeng/aeris-token' },
    workflow_run: {
      id: 456,
      status: 'completed',
      repository: { id: 123, full_name: 'JinPengGeng/aeris-token' },
      head_sha: 'f'.repeat(40),
      pull_requests: [{ number: 19 }, { number: 7 }, { number: 19 }],
    },
  });
  assert.deepEqual(targets, [{ pull_request_number: 7 }, { pull_request_number: 19 }]);
  assert.deepEqual(await runDiscovery('workflow_run', {
    repository: { id: 123, full_name: 'JinPengGeng/aeris-token' },
    workflow_run: { id: 456, status: 'completed', repository: { id: 123, full_name: 'JinPengGeng/aeris-token' }, head_sha: 'e'.repeat(40), pull_requests: [] },
  }), []);
});

test('workflow_run discovery fails closed for malformed or excessive associated PR sets', async () => {
  const bound = (pullRequests) => ({
    repository: { id: 123, full_name: 'JinPengGeng/aeris-token' },
    workflow_run: { id: 456, status: 'completed', repository: { id: 123, full_name: 'JinPengGeng/aeris-token' }, pull_requests: pullRequests },
  });
  await assert.rejects(
    () => runDiscovery('workflow_run', bound([{ number: 7 }, { number: 0 }])),
    /pull request 1 is invalid/,
  );
  await assert.rejects(
    () => runDiscovery('workflow_run', bound(null)),
    /pull request set is invalid/,
  );
  await assert.rejects(
    () => runDiscovery('workflow_run', bound(Array.from({ length: 101 }, (_, index) => ({ number: index + 1 })))),
    /exceeds 100/,
  );
});

test('Policy Gate jobs preserve diagnostic observation and authoritative App publish separation', () => {
  const { document } = read('.github/workflows/automation-policy-gate.yml');
  const { discover, observe, publish } = document.jobs;
  for (const job of [discover, observe]) {
    for (const access of Object.values(job.permissions ?? {})) assert.notEqual(access, 'write');
    assert.doesNotMatch(serialize(job), /secrets\./);
  }
  assert.equal(observe.permissions.contents, 'read');
  assert.equal(observe.permissions['pull-requests'], 'read');
  assert.equal(observe.permissions.checks, 'read');
  assert.equal(observe['continue-on-error'], undefined);
  const observationRunner = observe.steps.find((step) => step.run === 'node .github/automation/src/run-policy-gate.mjs evaluate');
  assert.equal(observationRunner.env.AERIS_EXPECTED_HEAD_SHA, '');
  assert.doesNotMatch(serialize(observationRunner.env), /workflow_run\.head_sha/);
  assert.match(observationRunner.env.AERIS_OUTPUT_PATH, /observation\.json$/);
  assert.equal(observationRunner.env.AERIS_EXPECTED_POLICY_SHA, '${{ steps.policy_sha.outputs.sha }}');
  assert.match(serialize(observe), /non-authoritative|diagnostic/i);
  assert.equal(publish.environment, 'policy');
  assert.deepEqual(publish.permissions, { contents: 'read' });
  assert.deepEqual(publish.needs, ['discover']);
  assert.equal(publish.steps.some((step) => String(step.uses ?? '').startsWith('actions/download-artifact@')), false);

  const mint = publish.steps.find((step) => step.id === 'policy_token');
  assert.ok(mint);
  assert.equal(mint.uses, 'actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1');
  assert.equal(mint.with['permission-contents'], 'read');
  assert.equal(mint.with['permission-pull-requests'], 'read');
  assert.equal(mint.with['permission-checks'], 'write');
  const secretSteps = publish.steps.filter((step) => /secrets\./.test(serialize(step)));
  assert.deepEqual(secretSteps.map((step) => step.id), ['policy_token']);
  const runner = publish.steps.find((step) => step.run === 'node .github/automation/src/run-policy-gate.mjs publish');
  assert.ok(runner);
  assert.equal(runner.env.AERIS_POLICY_TOKEN, '${{ steps.policy_token.outputs.token }}');
  assert.equal(runner.env.AERIS_PULL_REQUEST_NUMBER, '${{ matrix.pull_request_number }}');
  assert.equal(runner.env.AERIS_EXPECTED_POLICY_SHA, '${{ steps.policy_sha.outputs.sha }}');
  assert.equal(runner.env.AERIS_EXPECTED_HEAD_SHA, '${{ steps.early_fence.outputs.head_sha }}');
  assert.equal(runner.env.AERIS_EXPECTED_FENCE_CHECK_ID, '${{ steps.early_fence.outputs.check_run_id }}');
});

test('publish fences any reusable Policy success before checkout, npm, or runtime begin', () => {
  const { document } = read('.github/workflows/automation-policy-gate.yml');
  const steps = document.jobs.publish.steps;
  assert.equal(steps[0].id, 'policy_token');
  assert.equal(steps[1].id, 'early_fence');
  assert.equal(steps[1].uses, 'actions/github-script@f28e40c7f34bde8b3046d885e986cb6290c5673b');
  assert.match(steps[1].with.script, /status: 'in_progress'/);
  assert.match(steps[1].with.script, /PATCH \/repos\/\{owner\}\/\{repo\}\/check-runs\/\{check_run_id\}/);
  assert.match(steps[1].with.script, /POST \/repos\/\{owner\}\/\{repo\}\/check-runs/);
  assert.match(steps[1].with.script, /persisted\.data\?\.status !== 'in_progress'/);
  const checkout = steps.findIndex((step) => String(step.uses ?? '').startsWith('actions/checkout@'));
  const install = steps.findIndex((step) => step.run === 'npm ci --ignore-scripts');
  const runtime = steps.findIndex((step) => step.run === 'node .github/automation/src/run-policy-gate.mjs publish');
  assert.ok(checkout > 1 && install > checkout && runtime > install);
});

test('early fence repurposes the dominant old same-App success as the current generation', async () => {
  const head = 'a'.repeat(40);
  const base = 'b'.repeat(40);
  const policy = 'c'.repeat(40);
  const calls = [];
  let persisted;
  const outputs = await runEarlyFence(async (route, parameters) => {
    calls.push({ route, parameters });
    if (route === 'GET /repos/{owner}/{repo}') return { data: { id: 123, full_name: 'JinPengGeng/aeris-token', default_branch: 'main' } };
    if (route === 'GET /repos/{owner}/{repo}/git/ref/{ref}') return { data: { ref: 'refs/heads/main', object: { sha: policy } } };
    if (route === 'GET /repos/{owner}/{repo}/pulls/{pull_number}') return { data: {
      number: 37, state: 'open', head: { sha: head, repo: { full_name: 'JinPengGeng/aeris-token' } },
      base: { sha: base, ref: 'main', repo: { full_name: 'JinPengGeng/aeris-token' } },
    } };
    if (route === 'GET /repos/{owner}/{repo}/commits/{ref}/check-runs') return { data: { check_runs: [{
      id: 7, name: 'Automation Policy / gate', head_sha: head, external_id: 'aeris-policy:v1:123:37:old',
      status: 'completed', conclusion: 'success', app: { id: 9001, slug: 'aeris-token-policy' },
    }] } };
    if (route === 'PATCH /repos/{owner}/{repo}/check-runs/{check_run_id}') {
      persisted = { id: parameters.check_run_id, head_sha: head, conclusion: null, app: { id: 9001, slug: 'aeris-token-policy' }, ...parameters };
      return { data: persisted };
    }
    if (route === 'GET /repos/{owner}/{repo}/check-runs/{check_run_id}') return { data: persisted };
    throw new Error(`unexpected request ${route}`);
  });
  const mutations = calls.filter(({ route }) => route.startsWith('PATCH ') || route.startsWith('POST '));
  assert.deepEqual(mutations.map(({ route }) => route.split(' ')[0]), ['PATCH']);
  assert.ok(mutations.every(({ parameters }) => parameters.status === 'in_progress'));
  assert.deepEqual(outputs, { head_sha: head, policy_sha: policy, check_run_id: '7' });
});

test('cleanup failure leaves the highest-ID current fence dominant over an older success', async () => {
  const head = 'a'.repeat(40);
  const base = 'b'.repeat(40);
  const policy = 'c'.repeat(40);
  const state = new Map([
    [9, { id: 9, name: 'Automation Policy / gate', head_sha: head, external_id: 'aeris-policy:v1:123:37:old-9', status: 'completed', conclusion: 'success', app: { id: 9001, slug: 'aeris-token-policy' } }],
    [7, { id: 7, name: 'Automation Policy / gate', head_sha: head, external_id: 'aeris-policy:v1:123:37:old-7', status: 'completed', conclusion: 'success', app: { id: 9001, slug: 'aeris-token-policy' } }],
  ]);
  await assert.rejects(() => runEarlyFence(async (route, parameters) => {
    if (route === 'GET /repos/{owner}/{repo}') return { data: { id: 123, full_name: 'JinPengGeng/aeris-token', default_branch: 'main' } };
    if (route === 'GET /repos/{owner}/{repo}/git/ref/{ref}') return { data: { ref: 'refs/heads/main', object: { sha: policy } } };
    if (route === 'GET /repos/{owner}/{repo}/pulls/{pull_number}') return { data: {
      number: 37, state: 'open', head: { sha: head, repo: { full_name: 'JinPengGeng/aeris-token' } },
      base: { sha: base, ref: 'main', repo: { full_name: 'JinPengGeng/aeris-token' } },
    } };
    if (route === 'GET /repos/{owner}/{repo}/commits/{ref}/check-runs') return { data: { check_runs: [...state.values()] } };
    if (route === 'PATCH /repos/{owner}/{repo}/check-runs/{check_run_id}') {
      if (parameters.check_run_id === 7) throw new Error('simulated cleanup failure');
      const prior = state.get(parameters.check_run_id);
      const next = { ...prior, ...parameters, conclusion: null };
      state.set(parameters.check_run_id, next);
      return { data: next };
    }
    if (route === 'GET /repos/{owner}/{repo}/check-runs/{check_run_id}') return { data: state.get(parameters.check_run_id) };
    throw new Error(`unexpected request ${route}`);
  }), /simulated cleanup failure/);
  assert.equal(state.get(9).status, 'in_progress');
  assert.equal(state.get(9).external_id, `aeris-policy:v1:123:37:${head}:${policy}`);
  assert.equal(state.get(7).conclusion, 'success');
  assert.ok(state.get(9).id > state.get(7).id);
});

test('early fence does not normalize duplicate exact-generation checks into acceptable state', async () => {
  const head = 'a'.repeat(40);
  const base = 'b'.repeat(40);
  const policy = 'c'.repeat(40);
  const external = `aeris-policy:v1:123:37:${head}:${policy}`;
  const state = new Map([88, 77].map((id) => [id, {
    id,
    name: 'Automation Policy / gate',
    head_sha: head,
    external_id: external,
    status: 'in_progress',
    conclusion: null,
    app: { id: 9001, slug: 'aeris-token-policy' },
  }]));
  const outputs = await runEarlyFence(async (route, parameters) => {
    if (route === 'GET /repos/{owner}/{repo}') return { data: { id: 123, full_name: 'JinPengGeng/aeris-token', default_branch: 'main' } };
    if (route === 'GET /repos/{owner}/{repo}/git/ref/{ref}') return { data: { ref: 'refs/heads/main', object: { sha: policy } } };
    if (route === 'GET /repos/{owner}/{repo}/pulls/{pull_number}') return { data: {
      number: 37, state: 'open', head: { sha: head, repo: { full_name: 'JinPengGeng/aeris-token' } },
      base: { sha: base, ref: 'main', repo: { full_name: 'JinPengGeng/aeris-token' } },
    } };
    if (route === 'GET /repos/{owner}/{repo}/commits/{ref}/check-runs') return { data: { check_runs: [...state.values()] } };
    if (route === 'PATCH /repos/{owner}/{repo}/check-runs/{check_run_id}') {
      const next = { ...state.get(parameters.check_run_id), ...parameters, conclusion: null };
      state.set(parameters.check_run_id, next);
      return { data: next };
    }
    if (route === 'GET /repos/{owner}/{repo}/check-runs/{check_run_id}') return { data: state.get(parameters.check_run_id) };
    throw new Error(`unexpected request ${route}`);
  });
  assert.deepEqual(outputs, { head_sha: head, policy_sha: policy, check_run_id: '88' });
  assert.equal(state.get(88).external_id, external);
  assert.equal(state.get(77).external_id, external);
  assert.equal(state.get(88).status, 'in_progress');
  assert.equal(state.get(77).status, 'in_progress');
  assert.equal([...state.values()].filter((check) => check.external_id === external).length, 2);
});

test('Policy Gate jobs have bounded hard workflow deadlines', () => {
  const { document } = read('.github/workflows/automation-policy-gate.yml');
  assert.equal(document.jobs.discover['timeout-minutes'], 5);
  assert.equal(document.jobs.observe['timeout-minutes'], 10);
  assert.equal(document.jobs.publish['timeout-minutes'], 15);
});

test('Policy Gate checks out only the trusted default branch and pins every Action', () => {
  const { document } = read('.github/workflows/automation-policy-gate.yml');
  for (const job of Object.values(document.jobs)) {
    for (const step of job.steps ?? []) {
      if (typeof step.uses !== 'string') continue;
      assert.match(step.uses, /^[^@\s]+@[0-9a-f]{40}$/);
      if (step.uses.startsWith('actions/checkout@')) {
        assert.equal(step.with.ref, '${{ github.event.repository.default_branch }}');
        assert.equal(step.with['persist-credentials'], false);
        assert.doesNotMatch(serialize(step.with), /pull_request|workflow_run/);
      }
    }
  }
});

test('Policy Gate isolates observation from publish serialization by repository and PR without cancellation', () => {
  const { document } = read('.github/workflows/automation-policy-gate.yml');
  const expectedGroups = {
    observe: 'policy-observe-',
    publish: 'policy-gate-',
  };
  for (const [name, prefix] of Object.entries(expectedGroups)) {
    const concurrency = document.jobs[name].concurrency;
    assert.match(String(concurrency.group), new RegExp(`^${prefix}`));
    assert.match(String(concurrency.group), /repository\.id/);
    assert.match(String(concurrency.group), /matrix\.pull_request_number/);
    assert.equal(concurrency['cancel-in-progress'], false);
  }
  assert.notEqual(document.jobs.observe.concurrency.group, document.jobs.publish.concurrency.group);
  assert.match(serialize(document.jobs.discover), /more than 100 open pull requests/);
  assert.equal(document.jobs.observe.strategy['max-parallel'], 4);
  assert.equal(document.jobs.publish.strategy['max-parallel'], 4);
});
