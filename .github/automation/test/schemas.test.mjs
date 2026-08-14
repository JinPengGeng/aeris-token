import assert from 'node:assert/strict';
import test from 'node:test';

import { parseModelJson, validateAgentOutput } from '../src/schemas.mjs';

test('triage output accepts known non-control labels', () => {
  const output = validateAgentOutput(
    'triage',
    {
      schema_version: 1,
      agent: 'triage',
      summary: 'A bounded summary',
      risk: 'medium',
      proposed_labels: ['type:bug'],
      missing_information: ['Reproduction steps'],
      recommended_action: 'Confirm the failing request.',
      next_agent: 'planner',
    },
    ['type:bug', 'agent-ready'],
  );
  assert.deepEqual(output.proposed_labels, ['type:bug']);
});

test('triage output cannot propose authorization labels', () => {
  assert.throws(() =>
    validateAgentOutput(
      'triage',
      {
        schema_version: 1,
        agent: 'triage',
        summary: 'Summary',
        risk: 'low',
        proposed_labels: ['agent-ready'],
        missing_information: [],
        recommended_action: 'Review.',
        next_agent: null,
      },
      ['agent-ready'],
    ),
  );
});

test('model JSON parser rejects markdown fences', () => {
  assert.throws(() => parseModelJson('```json\n{"ok":true}\n```'));
  assert.deepEqual(parseModelJson('{"ok":true}'), { ok: true });
});

test('reviewer output cannot hand off to writer', () => {
  assert.throws(() =>
    validateAgentOutput('reviewer', {
      schema_version: 1,
      agent: 'reviewer',
      summary: 'Summary',
      verdict: 'needs_human_decision',
      findings: [],
      test_recommendations: [],
      next_agent: 'writer',
    }),
  );
});

test('reviewer finding is strictly bounded and structured', () => {
  const output = validateAgentOutput('reviewer', {
    schema_version: 1,
    agent: 'reviewer',
    summary: 'One issue found.',
    verdict: 'changes_requested',
    findings: [
      {
        severity: 'high',
        title: 'Missing authorization check',
        details: 'The write path accepts an untrusted actor.',
        path: 'src/handler.ts',
        line: 42,
      },
    ],
    test_recommendations: ['Add an unauthorized actor test.'],
    next_agent: 'security',
  });
  assert.equal(output.findings[0].line, 42);
  assert.equal(output.next_agent, 'security');
});
