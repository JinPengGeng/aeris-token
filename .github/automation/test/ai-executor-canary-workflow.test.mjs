import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';

test('executor canary keeps contract tests read-only and gates an optional protected live probe', () => {
  const workflow = fs.readFileSync(
    path.resolve(import.meta.dirname, '../../workflows/ai-executor-canary.yml'),
    'utf8',
  ).replace(/\r\n/g, '\n');
  assert.match(workflow, /^permissions:\n  contents: read$/m);
  assert.match(workflow, /node --test test\/ai-executor-contract\.test\.mjs test\/openai-responses-executor\.test\.mjs test\/ai-executor-canary\.test\.mjs/);
  assert.match(workflow, /if: inputs\.mode == 'live'/);
  assert.match(workflow, /environment: agent/);
  assert.match(workflow, /AERIS_AI_EXECUTOR_ID: \$\{\{ inputs\.executor \}\}/);
  assert.match(workflow, /run: node src\/ai-executor-canary\.mjs/);
  assert.doesNotMatch(workflow, /run:[^\n]*\$\{\{\s*inputs\.executor/);
  assert.doesNotMatch(workflow, /pull-requests:\s*write|contents:\s*write|issues:\s*write/);
});
