import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
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
const alwaysRequired = ['shell_security', 'prometheus_contracts'];

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
    prometheus_contracts: { result: overrides.prometheus_contracts ?? 'success' },
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
    'changes', ...alwaysRequired, ...leaves, ...gates, 'publish_dispatch_status',
  ]), 'new jobs must be accounted for in the required graph');
  assert.equal(workflow.jobs.shell_security.if, undefined);
  assert.equal(workflow.jobs.shell_security.needs, undefined);
  assert.equal(workflow.jobs.prometheus_contracts.if, undefined);
  assert.equal(workflow.jobs.prometheus_contracts.needs, undefined);
  assert.equal(workflow.jobs.check.name, 'Rust CI / check');
  assert.equal(workflow.jobs.publish_dispatch_status.needs, 'check');
  const shellFixtureStep = workflow.jobs.shell_security.steps.find((step) => typeof step.run === 'string');
  assert.ok(shellFixtureStep, 'shell fixture run step must exist');
  assert.match(
    shellFixtureStep.run,
    /PYTHONUTF8=1 python3 docs\/api\/generate_format_field_coverage\.py --check/u,
    'shell fixtures must enforce the format-field matrix drift check',
  );
  assert.match(
    shellFixtureStep.run,
    /python3 tests\/readme_governance_reference_test\.py/u,
    'shell fixtures must enforce README and CODEOWNERS reference checks',
  );
  assert.match(
    shellFixtureStep.run,
    /bash tests\/aether_gateway_build_script_invalidation_test\.sh/u,
    'shell fixtures must enforce linked-worktree build-script freshness',
  );
  const gatewayIntegrationStep = workflow.jobs.test_gateway.steps.find((step) =>
    step.name === 'Test gateway integration security contract');
  assert.ok(gatewayIntegrationStep, 'gateway security integration target must be executed');
  assert.match(shellFixtureStep.run, /bash tests\/postgres_live_test_gate_test\.sh/u,
    'shell fixtures must reject live DB runs with zero executed tests');
  assert.equal(
    gatewayIntegrationStep.run,
    'cargo nextest run -p aether-gateway --test admin_unsigned_identity_headers',
    'gateway security integration target must use the pinned nextest command',
  );
  assert.ok(workflow.jobs.shell_security.steps.some((step) =>
    step.uses?.startsWith('dtolnay/rust-toolchain@') && step.with?.toolchain === '1.95.0'),
  'the real Cargo fixture must have the pinned Rust toolchain');
  assert.ok(
    workflow.on.push.paths.includes('.github/CODEOWNERS'),
    'CODEOWNERS-only default-branch pushes must run the reference check',
  );
  for (const jobId of leaves) {
    assert.equal(workflow.jobs[jobId].needs, 'changes', jobId);
  }
  for (const jobId of gates) {
    const job = workflow.jobs[jobId];
    assert.equal(job.if, '${{ always() }}', jobId);
    const expected = jobId === 'check'
      ? ['changes', ...directLeaves, ...Object.keys(groups), ...alwaysRequired]
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

test('shell fixtures, Prometheus and every internal aggregate must succeed for both selected and skipped Rust work', () => {
  for (const filterOutput of ['true', 'false']) {
    for (const jobId of [...alwaysRequired, ...Object.keys(groups)]) {
      for (const result of ['skipped', 'failure', 'cancelled', '']) {
        const observed = runWorkflow({ filterOutput, overrides: { [jobId]: result } });
        assert.equal(observed.check.result, 'failure', `${filterOutput}: ${jobId}: ${result}`);
      }
    }
  }
});

const managedReadinessStepName = 'Verify standalone support and managed PostgreSQL readiness contracts';
const liveReadinessTargets = [
  'postgres::tests::live_managed_postgres_restarts_cleanly_with_open_connections',
  'postgres::tests::live_failed_postgres_stop_can_be_retried_without_losing_ownership',
];

function runManagedReadinessFixture(scenario) {
  const step = workflow.jobs.test_gateway.steps.find((candidate) => candidate.name === managedReadinessStepName);
  assert.ok(step, 'existing gateway job must execute managed service readiness contracts');
  assert.equal(step.shell, 'bash');
  const temporaryBase = process.env.AGENT_TMP_DIR || process.env.RUNNER_TEMP || path.join(os.homedir(), '.agents', 'tmp');
  fs.mkdirSync(temporaryBase, { recursive: true });
  const directory = fs.mkdtempSync(path.join(temporaryBase, 'aether-readiness-ci-fixture-'));
  const commandLog = path.join(directory, 'commands.log');
  const evidence = path.join(directory, 'evidence');
  const prelude = `
    cargo() {
      printf '%s\\n' "$*" >> "$COMMAND_LOG"
      local expected=1
      if [[ "$*" == *'aether-test-support'* ]]; then expected=7; fi
      if [[ "$SCENARIO" == support-zero && "$expected" == 7 ]]; then expected=0; fi
      if [[ "$expected" == 1 ]]; then
        case "$SCENARIO" in
          live-zero) expected=0 ;;
          live-two) expected=2 ;;
          live-ignored) printf 'test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out\\n'; return 0 ;;
          live-no-summary) return 0 ;;
          live-failed) printf 'test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\\n'; return 0 ;;
        esac
      fi
      printf 'test result: ok. %s passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\\n' "$expected"
      if [[ "$SCENARIO" == live-duplicate && "$expected" == 1 ]]; then
        printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\\n'
      fi
      if [[ "$SCENARIO" == cargo-fails ]]; then return 17; fi
      return 0
    }
    if [[ "$SCENARIO" == tee-fails ]]; then
      tee() { cat >/dev/null; return 23; }
    fi
  `;
  try {
    const result = spawnSync('bash', ['--noprofile', '--norc', '-e', '-o', 'pipefail', '-c', prelude + step.run], {
      env: {
        PATH: process.env.PATH,
        SCENARIO: scenario,
        COMMAND_LOG: commandLog,
        AETHER_TESTKIT_READINESS_EVIDENCE_DIR: evidence,
      },
      encoding: 'utf8',
    });
    assert.ifError(result.error);
    assert.equal(result.signal, null);
    const commands = fs.readFileSync(commandLog, 'utf8').trim().split('\n');
    const logs = fs.readdirSync(evidence);
    return { ...result, commands, logs };
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

test('managed readiness contracts reuse the equipped gateway job and always retain evidence', () => {
  const steps = workflow.jobs.test_gateway.steps;
  const index = steps.findIndex((step) => step.name === managedReadinessStepName);
  assert.ok(index >= 0);
  assert.ok(steps.slice(0, index).some((step) => step.run?.includes('pg_config --bindir')),
    'managed PostgreSQL binaries must be on PATH before the live tests');
  assert.ok(steps.slice(0, index).some((step) => step.run?.includes('apt-get install -y redis-server')),
    'reuse the existing managed-service dependencies');
  const upload = steps.find((step) => step.with?.name === 'testkit-readiness');
  assert.ok(upload);
  assert.equal(upload.if, 'always()');
  assert.equal(upload.with.path, 'target/testkit-readiness/');
  assert.equal(steps[index].env.AETHER_TESTKIT_READINESS_EVIDENCE_DIR, 'target/testkit-readiness');

  const result = runManagedReadinessFixture('success');
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.deepEqual(result.commands, [
    'test --locked -p aether-test-support --lib -- --nocapture --test-threads=1 --color never',
    ...liveReadinessTargets.map((target) =>
      `test --locked -p aether-testkit --features postgres --lib ${target} -- --exact --ignored --nocapture --test-threads=1 --color never`),
  ], 'support must compile alone and both ignored live tests must execute by exact name');
  assert.deepEqual(new Set(result.logs), new Set([
    'support-standalone.log',
    ...liveReadinessTargets.map((target) => `${target.split('::').at(-1)}.log`),
  ]));
});

test('managed readiness gate rejects empty, ignored, duplicate or failed runs and preserves Cargo/tee failure', () => {
  for (const scenario of [
    'support-zero', 'live-zero', 'live-two', 'live-ignored', 'live-no-summary', 'live-failed', 'live-duplicate',
  ]) {
    const result = runManagedReadinessFixture(scenario);
    assert.notEqual(result.status, 0, `${scenario}: ${result.stdout}${result.stderr}`);
    assert.ok(result.logs.includes('support-standalone.log'), `${scenario}: retain evidence on failure`);
  }
  assert.equal(runManagedReadinessFixture('cargo-fails').status, 17);
  assert.equal(runManagedReadinessFixture('tee-fails').status, 23);
});
