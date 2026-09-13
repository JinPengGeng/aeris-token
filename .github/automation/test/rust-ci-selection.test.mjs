import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

import yaml from 'js-yaml';

import { loadChangeFilters, matchedChangeFilterGroups } from '../src/change-filters.mjs';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const workflow = yaml.load(fs.readFileSync(path.join(repoRoot, '.github/workflows/rust-ci.yml'), 'utf8'));
const filters = loadChangeFilters(repoRoot);
const groups = {
  clippy: ['clippy_gateway', 'clippy_data', 'clippy_rest'],
  test: ['test_gateway', 'test_data', 'check_data_features', 'test_rest', 'test_data_adapters', 'check_integration_scenarios'],
  data_db_smoke: ['data_db_smoke_postgres'],
};
const directLeaves = ['fmt', 'data_db_ignored_postgres'];
const leaves = [...directLeaves, ...Object.values(groups).flat()];
const gates = [...Object.keys(groups), 'check'];

// Execute the workflow's restricted boolean expressions with the same string
// truthiness and case-insensitive string equality as Actions (notably, the
// string 'false' is truthy). This exercises the actual job conditions/output
// expressions rather than a copied decision. It is not an Actions scheduler.
function expressionValue(expression, context) {
  const match = /^\$\{\{\s*([\s\S]*?)\s*\}\}$/.exec(expression);
  assert.ok(match, `expected an Actions expression: ${expression}`);
  const comparable = "(?:[a-z_][a-z_0-9.]*|'[^']*')";
  const source = match[1].replace(new RegExp(`(${comparable})\\s*(!=|==)\\s*(${comparable})`, 'gi'),
    (_, left, operator, right) => `${operator === '!=' ? '!' : ''}actionsEqual(${left}, ${right})`);
  return vm.runInNewContext(source, {
    ...context,
    actionsEqual(left, right) {
      assert.equal(typeof left, 'string', 'extend the evaluator before using non-string equality');
      assert.equal(typeof right, 'string', 'extend the evaluator before using non-string equality');
      return left.toLowerCase() === right.toLowerCase();
    },
  }, { timeout: 1000 });
}

function contextFor(eventName, needs, filterOutput, inputs = {}) {
  return {
    github: { event_name: eventName },
    inputs,
    needs,
    steps: { filter: { outputs: { rust: filterOutput, data: '' } } },
    always: () => true,
  };
}

function runGate(jobId, needs) {
  const job = workflow.jobs[jobId];
  assert.equal(job.steps.length, 1, `${jobId}: inspect new gate steps before extending the harness`);
  const step = job.steps[0];
  assert.equal(step.shell, 'bash');
  assert.doesNotMatch(step.run, /\$\{\{/u, 'Actions results must enter the shell through environment variables');
  const context = contextFor('pull_request', needs, '');
  const env = Object.fromEntries(Object.entries(step.env).map(([key, value]) => [
    key,
    value.replace(/\$\{\{[\s\S]*?\}\}/g, (expression) => String(expressionValue(expression, context) ?? '')),
  ]));
  const result = spawnSync('bash', ['--noprofile', '--norc', '-e', '-o', 'pipefail', '-c', step.run], {
    env: { PATH: process.env.PATH, ...env },
    encoding: 'utf8',
  });
  assert.ifError(result.error);
  assert.equal(result.signal, null);
  return { result: result.status === 0 ? 'success' : 'failure', output: result.stdout + result.stderr };
}

function runWorkflow({
  eventName = 'pull_request',
  files = ['docs/guide.md'],
  changesResult = 'success',
  filterOutput = String(matchedChangeFilterGroups(filters, files).includes('rust')),
  inputs = {},
  overrides = {},
} = {}) {
  const output = expressionValue(workflow.jobs.changes.outputs.rust, contextFor(eventName, {}, filterOutput, inputs));
  const needs = {
    changes: { result: changesResult, outputs: { rust: output } },
    shell_security: { result: overrides.shell_security ?? 'success' },
  };
  for (const jobId of leaves) {
    const selected = expressionValue(workflow.jobs[jobId].if, contextFor(eventName, needs, filterOutput));
    needs[jobId] = { result: overrides[jobId] ?? (selected ? 'success' : 'skipped') };
  }
  for (const jobId of gates) {
    const observed = runGate(jobId, needs);
    needs[jobId] = { result: overrides[jobId] ?? observed.result, output: observed.output };
  }
  return needs;
}

test('every Rust and database job consumes changes, while all aggregate gates and shell fixtures remain required', () => {
  assert.deepEqual(new Set(Object.keys(workflow.jobs)), new Set([
    'changes', 'shell_security', ...leaves, ...gates, 'publish_dispatch_status',
  ]), 'new jobs must be accounted for in the required graph');
  assert.equal(workflow.jobs.shell_security.if, undefined);
  assert.equal(workflow.jobs.shell_security.needs, undefined);
  assert.equal(workflow.jobs.check.name, 'Rust CI / check');
  assert.equal(workflow.jobs.publish_dispatch_status.needs, 'check');
  for (const jobId of leaves) {
    assert.equal(workflow.jobs[jobId].needs, 'changes', jobId);
  }
  for (const jobId of gates) {
    const job = workflow.jobs[jobId];
    assert.equal(job.if, '${{ always() }}', jobId);
    const expected = jobId === 'check'
      ? ['changes', ...directLeaves, ...Object.keys(groups), 'shell_security']
      : ['changes', ...groups[jobId]];
    assert.deepEqual(new Set(job.needs), new Set(expected), jobId);
    const referenced = [...Object.values(job.steps[0].env).join(' ').matchAll(/needs\.([a-z_]+)\.result/g)]
      .map((match) => match[1]);
    assert.deepEqual(new Set(referenced), new Set(expected), `${jobId}: every dependency must be verified`);
  }
});

test('ordinary docs and frontend package PRs skip all Rust/DB work and pass the actual aggregate scripts', () => {
  for (const files of [['docs/guide.md'], ['README.md'], ['frontend/package.json'], ['LICENSE']]) {
    const observed = runWorkflow({ files });
    for (const jobId of leaves) assert.equal(observed[jobId].result, 'skipped', `${files}: ${jobId}`);
    for (const jobId of gates) assert.equal(observed[jobId].result, 'success', `${files}: ${jobId}: ${observed[jobId].output}`);
    assert.equal(observed.shell_security.result, 'success');
  }
});

test('Rust, DB, CI harnesses, and external contract inputs run the whole existing Rust/DB graph', () => {
  const files = [
    'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml',
    'crates/aether-gateway/src/lib.rs', 'crates/aether-data/adapters/postgres/src/usage/cleanup.rs',
    'apps/aether-gateway/src/main.rs', 'tools/ci/run_postgres_live_tests.sh',
    'Dockerfile.app.local', 'deploy.sh', 'frontend/vite.config.ts', 'frontend/src/api/example.ts',
    'tools/ci/run_readiness_drill.sh', 'tests/compose_database_config_test.py',
    'docs/api/fixtures/public-api-compatibility.json', 'docs/api/provider-interface-definitions.md',
    'docs/api/format-field-coverage-matrix.md', 'docs/api/format-conversion-audit.md',
    'docs/operations/referral-rebate-numeric-audit.sql', 'docs/operations/fixtures/referral-specials.sql',
    '.github/change-filters.yml', '.github/workflows/rust-ci.yml',
  ];
  for (const file of files) {
    const observed = runWorkflow({ files: [file] });
    for (const jobId of [...leaves, ...gates]) assert.equal(observed[jobId].result, 'success', `${file}: ${jobId}: ${observed[jobId].output}`);
    assert.ok(workflow.on.push.paths.some((pattern) => pattern === file || (pattern.endsWith('/**') && file.startsWith(pattern.slice(0, -2)))),
      `main push must also run for ${file}`);
  }
});

test('push and manual dispatch run fully even without filter outputs', () => {
  for (const eventName of ['push', 'workflow_dispatch']) {
    for (const filterOutput of ['', 'false', 'true']) {
      const observed = runWorkflow({ eventName, filterOutput });
      assert.equal(observed.changes.outputs.rust, 'true');
      for (const jobId of [...leaves, ...gates]) assert.equal(observed[jobId].result, 'success', `${eventName}: ${jobId}`);
    }
  }
});

test('reusable workflows retain the caller event and default to full validation even for docs PRs', () => {
  const fullInput = workflow.on.workflow_call.inputs.force_full;
  assert.equal(fullInput.type, 'boolean');
  assert.equal(fullInput.default, true);
  for (const eventName of ['pull_request', 'push', 'workflow_dispatch']) {
    for (const filterOutput of ['', 'false', 'true']) {
      const inputs = { force_full: fullInput.default };
      const observed = runWorkflow({ eventName, filterOutput, inputs });
      assert.equal(observed.changes.outputs.rust, 'true');
      for (const jobId of [...leaves, ...gates]) assert.equal(observed[jobId].result, 'success', `${eventName}: ${jobId}`);
      for (const step of workflow.jobs.changes.steps) {
        assert.equal(expressionValue(step.if, contextFor(eventName, {}, filterOutput, inputs)), false,
          'full validation must not rely on a PR path-filter step');
      }
    }
  }
  const directPr = contextFor('pull_request', {}, 'false');
  for (const step of workflow.jobs.changes.steps) assert.equal(expressionValue(step.if, directPr), true);
});

test('failed, cancelled or skipped detection cannot turn skipped work into a successful gate', () => {
  for (const changesResult of ['failure', 'cancelled', 'skipped', '']) {
    for (const filterOutput of ['true', 'false']) {
      const observed = runWorkflow({ changesResult, filterOutput });
      for (const jobId of leaves) assert.equal(observed[jobId].result, 'skipped', `${changesResult}: ${jobId}`);
      for (const jobId of gates) assert.equal(observed[jobId].result, 'failure', `${changesResult}: ${jobId}`);
    }
  }
});

test('empty or unrecognized successful PR detection fails closed instead of defaulting to a passing scope', () => {
  for (const filterOutput of ['', 'unknown', 'TRUE', ' false ']) {
    const observed = runWorkflow({ filterOutput });
    // Actions string comparison accepts uppercase TRUE; the actual shell gate
    // still rejects it instead of accepting a noncanonical detector result.
    const expectedLeaf = filterOutput === 'TRUE' ? 'success' : 'skipped';
    for (const jobId of leaves) assert.equal(observed[jobId].result, expectedLeaf, `${filterOutput}: ${jobId}`);
    for (const jobId of gates) assert.equal(observed[jobId].result, 'failure', `${filterOutput}: ${jobId}`);
  }
});

test('every selected leaf must succeed; unexpected skips, failures and cancellation block the required check', () => {
  for (const jobId of leaves) {
    for (const result of ['skipped', 'failure', 'cancelled', '']) {
      const observed = runWorkflow({ filterOutput: 'true', overrides: { [jobId]: result } });
      assert.equal(observed.check.result, 'failure', `${jobId}: ${result}`);
    }
  }
});

test('docs-only skips must be planned; unexpected execution or failure blocks the required check', () => {
  for (const jobId of leaves) {
    for (const result of ['success', 'failure', 'cancelled', '']) {
      const observed = runWorkflow({ filterOutput: 'false', overrides: { [jobId]: result } });
      assert.equal(observed.check.result, 'failure', `${jobId}: ${result}`);
    }
  }
});

test('shell fixtures and every internal aggregate must succeed for both selected and skipped Rust work', () => {
  for (const filterOutput of ['true', 'false']) {
    for (const jobId of ['shell_security', ...Object.keys(groups)]) {
      for (const result of ['skipped', 'failure', 'cancelled', '']) {
        const observed = runWorkflow({ filterOutput, overrides: { [jobId]: result } });
        assert.equal(observed.check.result, 'failure', `${filterOutput}: ${jobId}: ${result}`);
      }
    }
  }
});
