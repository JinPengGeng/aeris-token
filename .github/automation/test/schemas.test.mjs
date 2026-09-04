import assert from 'node:assert/strict';
import test from 'node:test';

import { parseModelJson, validateAgentOutput } from '../src/schemas.mjs';

function assertDiagnostic(action, diagnostic) {
  assert.throws(
    action,
    (error) =>
      error.code === 'invalid_model_output' && error.diagnostic === diagnostic,
  );
}

function plannerOutput(overrides = {}) {
  return {
    schema_version: 1,
    agent: 'planner',
    summary: 'A bounded implementation plan.',
    acceptance_criteria: ['The behavior is deterministic.'],
    implementation_steps: ['Implement the bounded change.'],
    validation_plan: ['Run the focused tests.'],
    risks: ['Provider compatibility must be verified.'],
    next_agent: null,
    ...overrides,
  };
}

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
  assertDiagnostic(() => parseModelJson('```json\n{"ok":true}\n```'), 'json_envelope');
  assert.deepEqual(parseModelJson('{"ok":true}'), { ok: true });
});

test('model JSON parser distinguishes envelope and syntax failures without output text', () => {
  assertDiagnostic(() => parseModelJson('prefix {"ok":true}'), 'json_envelope');
  assertDiagnostic(() => parseModelJson('{"secret-shaped-value":}'), 'json_syntax');
});

test('planner validation reports stable shape, enum, and bounds diagnostics', () => {
  assertDiagnostic(
    () => validateAgentOutput('planner', plannerOutput({ extra: 'not allowed' })),
    'planner_output_keys',
  );
  assertDiagnostic(
    () => validateAgentOutput('planner', plannerOutput({ next_agent: 'writer' })),
    'planner_next_agent_enum',
  );
  assertDiagnostic(
    () =>
      validateAgentOutput(
        'planner',
        plannerOutput({ acceptance_criteria: Array.from({ length: 13 }, () => 'criterion') }),
      ),
    'acceptance_criteria_bounds',
  );
  assertDiagnostic(
    () =>
      validateAgentOutput(
        'planner',
        plannerOutput({ acceptance_criteria: ['x'.repeat(501)] }),
      ),
    'acceptance_criteria_item_bounds',
  );
});

test('reviewer output cannot hand off to a retired or unknown agent', () => {
  for (const nextAgent of ['writer', 'security']) {
    assertDiagnostic(
      () =>
        validateAgentOutput('reviewer', {
          schema_version: 1,
          agent: 'reviewer',
          summary: 'Summary',
          verdict: 'needs_human_decision',
          findings: [],
          test_recommendations: [],
          next_agent: nextAgent,
        }),
      'reviewer_next_agent_enum',
    );
  }
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
    next_agent: null,
  });
  assert.equal(output.findings[0].line, 42);
  assert.equal(output.next_agent, null);
});
