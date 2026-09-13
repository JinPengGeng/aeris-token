import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import yaml from 'js-yaml';

const workflowPath = fileURLToPath(new URL('../../workflows/rust-ci.yml', import.meta.url));
const expectedActions = new Map([
  ['actions/checkout', 'fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09'],
  ['actions/upload-artifact', '330a01c490aca151604b8cf639adc76d48f6c5d4'],
  ['dtolnay/rust-toolchain', '4360b52568e2003a75bf9bc1d59f33a8e3fc893c'],
  ['Swatinem/rust-cache', '49a0bdc70d2e1b713ca9e2869b211fcce03d3c1c'],
  ['mozilla-actions/sccache-action', '7d986dd989559c6ecdb630a3fd2557667be217ad'],
  ['rui314/setup-mold', '7e4f20ad28a2e8ca6fd0892ccf72e2abb706b9c3'],
  ['taiki-e/install-action', 'd5f9268ff7620505a81ada10ddf18cdd72240185'],
  ['dorny/paths-filter', 'de90cc6fb38fc0963ad72b210f1f284cd68cea36'],
]);

test('Rust CI pins every third-party action to its approved immutable commit', async () => {
  const workflow = await readFile(workflowPath, 'utf8');
  const references = (text) => [...text.matchAll(/^\s*-?\s*uses:\s*([^\s#]+)(?:\s+#.*)?$/gm)]
    .map((match) => match[1]);
  const localWorkflows = references(workflow).filter((ref) => ref.startsWith('./'));
  assert.deepEqual(localWorkflows, ['./.github/workflows/prometheus-ci.yml']);
  const prometheusWorkflow = await readFile(
    new URL('../../workflows/prometheus-ci.yml', import.meta.url), 'utf8',
  );
  const actionRefs = [...references(workflow).filter((ref) => !ref.startsWith('./')),
    ...references(prometheusWorkflow)];

  assert.ok(actionRefs.length > 0, 'Rust CI should invoke third-party actions');
  for (const ref of actionRefs) {
    const match = /^(?<action>[^@]+)@(?<sha>[0-9a-f]{40})$/.exec(ref);
    assert.ok(match, `action reference must use a full 40-character SHA: ${ref}`);
    assert.equal(expectedActions.get(match.groups.action), match.groups.sha, `unexpected action SHA: ${ref}`);
  }
  assert.deepEqual(new Set(actionRefs.map((ref) => ref.split('@')[0])), new Set(expectedActions.keys()));
});

test('Prometheus failure or skipped execution fails the required Rust aggregate', async () => {
  const workflow = yaml.load(await readFile(workflowPath, 'utf8'));
  const aggregate = workflow.jobs.check;
  assert.equal(workflow.jobs.prometheus_contracts.uses, './.github/workflows/prometheus-ci.yml');
  assert.ok(aggregate.needs.includes('prometheus_contracts'));
  const script = aggregate.steps.find((step) => step.name === 'Verify required jobs').run;
  for (const result of ['success', 'failure', 'cancelled', 'skipped', '']) {
    const rendered = script.replace(/\$\{\{\s*needs\.(\w+)\.result\s*\}\}/g,
      (_, job) => job === 'prometheus_contracts' ? result : 'success');
    const outcome = spawnSync('bash', ['-e', '-c', rendered], { encoding: 'utf8' });
    assert.equal(outcome.status, result === 'success' ? 0 : 1,
      `Prometheus result ${JSON.stringify(result)}: ${outcome.stdout} ${outcome.stderr}`);
  }
  const reusable = yaml.load(await readFile(
    new URL('../../workflows/prometheus-ci.yml', import.meta.url), 'utf8',
  ));
  assert.ok(Object.hasOwn(reusable.on, 'workflow_call'));
});
