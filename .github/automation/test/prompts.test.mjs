import assert from 'node:assert/strict';
import test from 'node:test';

import { buildMessages, responseFormatForAgent } from '../src/prompts.mjs';

const expectedRequiredKeys = {
  triage: [
    'schema_version',
    'agent',
    'summary',
    'risk',
    'proposed_labels',
    'missing_information',
    'recommended_action',
    'next_agent',
  ],
  planner: [
    'schema_version',
    'agent',
    'summary',
    'acceptance_criteria',
    'implementation_steps',
    'validation_plan',
    'risks',
    'next_agent',
  ],
  reviewer: ['schema_version', 'agent', 'summary', 'verdict', 'findings', 'test_recommendations', 'next_agent'],
};

const providerConstraintKeywords = new Set([
  'minLength',
  'maxLength',
  'pattern',
  'format',
  'minimum',
  'maximum',
  'exclusiveMinimum',
  'exclusiveMaximum',
  'multipleOf',
  'minItems',
  'maxItems',
]);

function assertProviderSafeSchema(value) {
  if (!value || typeof value !== 'object') return;
  for (const [key, nested] of Object.entries(value)) {
    assert.equal(providerConstraintKeywords.has(key), false, `provider schema contains ${key}`);
    assertProviderSafeSchema(nested);
  }
}

test('response formats encode each agent provider contract', () => {
  for (const [agent, required] of Object.entries(expectedRequiredKeys)) {
    const responseFormat = responseFormatForAgent(agent);
    const schema = responseFormat.json_schema.schema;
    assert.equal(responseFormat.type, 'json_schema');
    assert.equal(responseFormat.json_schema.name, `aeris_${agent}_output`);
    assert.equal(responseFormat.json_schema.strict, true);
    assert.equal(schema.additionalProperties, false);
    assert.deepEqual(schema.required, required);
    assert.deepEqual(schema.properties.schema_version, { type: 'integer', const: 1 });
    assert.deepEqual(schema.properties.agent, { type: 'string', const: agent });
    assert.match(schema.properties.summary.description, /at most 1200 characters/);
    assertProviderSafeSchema(schema);
  }

  const triage = responseFormatForAgent('triage').json_schema.schema;
  assert.deepEqual(triage.properties.risk.enum, ['low', 'medium', 'high']);
  assert.deepEqual(triage.properties.next_agent.enum, ['planner', null]);
  assert.match(triage.properties.proposed_labels.description, /At most 8 items/);
  assert.match(triage.properties.proposed_labels.items.description, /at most 80 characters/);
  assert.match(triage.properties.missing_information.description, /At most 12 items/);
  assert.match(triage.properties.missing_information.items.description, /at most 500 characters/);

  const planner = responseFormatForAgent('planner').json_schema.schema;
  assert.deepEqual(planner.properties.next_agent.enum, ['reviewer', null]);
  for (const field of ['acceptance_criteria', 'implementation_steps', 'validation_plan', 'risks']) {
    assert.match(planner.properties[field].description, /At most 12 items/);
    assert.match(planner.properties[field].items.description, /at most 500 characters/);
  }

  const reviewer = responseFormatForAgent('reviewer').json_schema.schema;
  assert.deepEqual(reviewer.properties.next_agent.enum, [null]);
  assert.match(reviewer.properties.findings.description, /At most 20 items/);
  assert.equal(reviewer.properties.findings.items.additionalProperties, false);
  assert.match(reviewer.properties.findings.items.properties.title.description, /at most 200 characters/);
  assert.match(reviewer.properties.findings.items.properties.details.description, /at most 1000 characters/);
  assert.match(reviewer.properties.findings.items.properties.path.description, /at most 300 characters/);
  assert.match(reviewer.properties.findings.items.properties.line.description, /positive integer/);
});

test('buildMessages embeds the exact schema sent through response_format', () => {
  for (const agent of Object.keys(expectedRequiredKeys)) {
    const schema = responseFormatForAgent(agent).json_schema.schema;
    const [system] = buildMessages(agent, { issue: 'untrusted' });
    assert.match(system.content, new RegExp(`Required JSON Schema: ${JSON.stringify(schema).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`));
  }
});

test('response format returns independent schemas and rejects unknown agents', () => {
  const first = responseFormatForAgent('planner');
  first.json_schema.schema.properties.summary.description = 'mutated';
  assert.match(
    responseFormatForAgent('planner').json_schema.schema.properties.summary.description,
    /at most 1200 characters/,
  );
  assert.throws(() => responseFormatForAgent('writer'), /unsupported output agent/);
});
