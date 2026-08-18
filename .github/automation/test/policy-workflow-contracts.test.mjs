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

test('Policy Gate jobs preserve read/evaluate and App publish separation', () => {
  const { document } = read('.github/workflows/automation-policy-gate.yml');
  const { discover, evaluate, publish } = document.jobs;
  for (const job of [discover, evaluate]) {
    for (const access of Object.values(job.permissions ?? {})) assert.notEqual(access, 'write');
    assert.doesNotMatch(serialize(job), /secrets\./);
  }
  assert.equal(evaluate.permissions.contents, 'read');
  assert.equal(evaluate.permissions['pull-requests'], 'read');
  assert.equal(evaluate.permissions.checks, 'read');
  const evaluateRunner = evaluate.steps.find((step) => step.run === 'node .github/automation/src/run-policy-gate.mjs evaluate');
  assert.equal(evaluateRunner.env.AERIS_EXPECTED_HEAD_SHA, '');
  assert.doesNotMatch(serialize(evaluateRunner.env), /workflow_run\.head_sha/);
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

test('Policy Gate isolates evaluation from publish serialization by repository and PR without cancellation', () => {
  const { document } = read('.github/workflows/automation-policy-gate.yml');
  const expectedGroups = {
    evaluate: 'policy-evaluate-',
    publish: 'policy-gate-',
  };
  for (const [name, prefix] of Object.entries(expectedGroups)) {
    const concurrency = document.jobs[name].concurrency;
    assert.match(String(concurrency.group), new RegExp(`^${prefix}`));
    assert.match(String(concurrency.group), /repository\.id/);
    assert.match(String(concurrency.group), /matrix\.pull_request_number/);
    assert.equal(concurrency['cancel-in-progress'], false);
  }
  assert.notEqual(document.jobs.evaluate.concurrency.group, document.jobs.publish.concurrency.group);
  assert.match(serialize(document.jobs.discover), /more than 100 open pull requests/);
  assert.equal(document.jobs.evaluate.strategy['max-parallel'], 4);
  assert.equal(document.jobs.publish.strategy['max-parallel'], 4);
});
