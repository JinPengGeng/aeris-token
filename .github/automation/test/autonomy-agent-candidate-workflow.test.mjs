import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const yaml = require('js-yaml');
const testDirectory = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(testDirectory, '..', '..', '..');
const workflowPath = path.join(repoRoot, '.github', 'workflows', 'agent-candidate.yml');
const promptPath = path.join(repoRoot, '.github', 'codex', 'candidate-prompt.md');
const schemaPath = path.join(repoRoot, '.github', 'codex', 'schemas', 'result.schema.json');

function workflow() {
  return yaml.load(fs.readFileSync(workflowPath, 'utf8'));
}

function allUses(value, results = []) {
  if (Array.isArray(value)) value.forEach((item) => allUses(item, results));
  else if (value && typeof value === 'object') {
    for (const [key, item] of Object.entries(value)) {
      if (key === 'uses') results.push(item);
      allUses(item, results);
    }
  }
  return results;
}

test('candidate workflow is manual, disabled by default, and binds the trusted base', () => {
  const document = workflow();
  assert.deepEqual(Object.keys(document.on), ['workflow_dispatch']);
  assert.match(String(document.jobs.preflight.if), /AERIS_CANDIDATE_AGENTS_ENABLED/);
  assert.match(String(document.jobs.preflight.if), /AERIS_AGENTS_ENABLED/);
  assert.equal(document.jobs.preflight.permissions.contents, 'read');
  assert.equal(document.jobs.preflight.permissions.issues, 'read');
  assert.equal(document.jobs.candidate.needs[0], 'preflight');
  assert.equal(document.jobs.candidate.environment, 'agent');
  assert.equal(document.jobs.candidate.permissions.contents, 'read');
  assert.equal(document.jobs.candidate.steps[0].with.ref, '${{ needs.preflight.outputs.base_sha }}');
});

test('candidate workflow exposes the model secret only to the agent Environment and pins actions', () => {
  const document = workflow();
  const candidate = document.jobs.candidate;
  const serializedPreflight = JSON.stringify(document.jobs.preflight);
  assert.doesNotMatch(serializedPreflight, /secrets\./i);
  const secretSteps = candidate.steps.filter((step) => /secrets\.AERIS_AI_API_KEY/.test(JSON.stringify(step)));
  assert.equal(secretSteps.length, 1);
  assert.equal(secretSteps[0].uses, 'openai/codex-action@52fe01ec70a42f454c9d2ebd47598f9fd6893d56');
  assert.equal(secretSteps[0].with['codex-version'], '0.148.0');
  assert.equal(secretSteps[0].with['permission-profile'], ':workspace');
  assert.equal(secretSteps[0].with['safety-strategy'], 'drop-sudo');
  assert.equal(secretSteps[0].with['responses-api-endpoint'], "${{ format('{0}/responses', vars.AERIS_AI_BASE_URL) }}");
  assert.equal(secretSteps[0].with['output-schema-file'], '.github/codex/schemas/result.schema.json');
  assert.equal(secretSteps[0].with.model, '${{ vars.AERIS_AI_MODEL }}');
  assert.equal(JSON.stringify(candidate), JSON.stringify(candidate).replace(/AERIS_WRITER|release/i, ''));
  for (const action of allUses(document.jobs)) assert.match(action, /^[^@\s]+@[0-9a-f]{40}$/);
});

test('candidate workflow emits only a short-lived patch and manifest after deterministic validation', () => {
  const candidate = workflow().jobs.candidate;
  const stage = candidate.steps.find((step) => /Stage trusted candidate runtime/.test(step.name));
  const extract = candidate.steps.find((step) => /Extract and validate/.test(step.name));
  const codexIndex = candidate.steps.findIndex((step) => step.uses?.startsWith('openai/codex-action@'));
  const stageIndex = candidate.steps.indexOf(stage);
  const extractIndex = candidate.steps.indexOf(extract);
  assert.ok(stageIndex >= 0 && stageIndex < codexIndex);
  assert.ok(extractIndex > codexIndex);
  assert.match(stage.run, /autonomy-agent-candidate-runner\.mjs/);
  assert.match(stage.run, /autonomy-candidate\.mjs/);
  assert.match(stage.run, /autonomy-extract\.mjs/);
  assert.match(extract.run, /\/usr\/bin\/env -i/);
  assert.match(extract.run, /\$\{runtime\}\/autonomy-agent-candidate-runner\.mjs/);
  assert.doesNotMatch(extract.run, /node \.github\/automation/);
  const upload = candidate.steps.find((step) => /Upload candidate artifact/.test(step.name));
  assert.equal(upload.with.name, 'agent-candidate-issue-${{ needs.preflight.outputs.issue_number }}-run-${{ github.run_id }}-${{ github.run_attempt }}');
  assert.equal(upload.with['retention-days'], 1);
  assert.match(upload.with.path, /candidate\.patch/);
  assert.match(upload.with.path, /candidate-manifest\.json/);
});

test('candidate prompt confines the Agent to unstaged, non-governed changes', () => {
  const prompt = fs.readFileSync(promptPath, 'utf8');
  assert.match(prompt, /AERIS_TASK_ID/);
  assert.match(prompt, /AERIS_BASE_SHA/);
  assert.match(prompt, /Do not modify `\.github\/\*\*`/);
  assert.match(prompt, /Do not commit, push, stage/);
  assert.match(prompt, /Do not run tests, builds, package installation, generated code/);
  assert.match(prompt, /matches the\s+provided output schema/);
  const schema = JSON.parse(fs.readFileSync(schemaPath, 'utf8'));
  assert.equal(schema.additionalProperties, false);
  assert.deepEqual(schema.required, ['summary', 'files_changed']);
});
