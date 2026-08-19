import assert from 'node:assert/strict';
import test from 'node:test';

import {
  canonicalJson,
  lifecycleFromGraphqlPull,
  parseReviewAttestationSummary,
  renderReviewAttestation,
  reviewAttestationExternalId,
  validateReviewAttestation,
} from '../src/review-attestation-contract.mjs';

const sha = (character) => character.repeat(40);
const hash = (character) => character.repeat(64);
const lifecycle = { kind: 'initial', pull_node_id: 'PR_kwDOAerisToken37', pull_created_at: '2026-08-18T00:00:00Z', reopened_event_id: null, reopened_at: null };
const generation = { repository_id: 123, repository: 'JinPengGeng/aeris-token', pull_number: 37, head_sha: sha('a'), base_sha: sha('b'), policy_sha: sha('c'), lifecycle_epoch: lifecycle };

function receipt(overrides = {}) {
  return validateReviewAttestation({
    schema_version: 1, artifact_type: 'review_attestation', role: 'reviewer', ...generation,
    input_sha: hash('d'), prompt_sha: hash('e'), profile_sha: hash('f'),
    coverage: { complete: true, file_count: 1, patch_bytes: 20, manifest_sha: hash('2'), raw_diff_sha: hash('3') },
    requested_model: { alias: 'reviewer', id: 'gpt-5.6-sol' },
    provider_model: { response_id: 'chatcmpl-37', model: 'gpt-5.6-sol', system_fingerprint: null },
    result_sha: hash('1'), verdict: 'pass', finding_count: 0, run_group_id: '42', run_id: '42.1', completed_at: '2026-08-20T00:00:00Z',
    ...overrides,
  });
}

test('receipt summary and external ID bind all exact-generation and model evidence', () => {
  const value = receipt();
  const rendered = renderReviewAttestation(value);
  assert.deepEqual(parseReviewAttestationSummary(rendered.summary, { expectedGeneration: generation, expectedRole: 'reviewer' }), value);
  const changed = receipt({ lifecycle_epoch: { kind: 'reopened', pull_node_id: lifecycle.pull_node_id, pull_created_at: lifecycle.pull_created_at, reopened_event_id: 99, reopened_at: '2026-08-19T00:00:00Z' } });
  assert.notEqual(reviewAttestationExternalId(value), reviewAttestationExternalId(changed));
  assert.notEqual(reviewAttestationExternalId(value), reviewAttestationExternalId(receipt({ provider_model: { ...value.provider_model, model: 'other-model' } })));
  assert.equal(rendered.summary, canonicalJson(value));
});

test('lifecycle parsing fails closed on truncation, ordering, and state conflicts', () => {
  const pull = {
    id: lifecycle.pull_node_id,
    createdAt: lifecycle.pull_created_at,
    state: 'OPEN',
    timelineItems: {
      nodes: [
        { __typename: 'ClosedEvent', databaseId: 7, createdAt: '2026-08-18T01:00:00Z' },
        { __typename: 'ReopenedEvent', databaseId: 8, createdAt: '2026-08-18T02:00:00Z' },
      ],
      pageInfo: { hasPreviousPage: false },
    },
  };
  assert.equal(lifecycleFromGraphqlPull(pull).reopened_event_id, 8);
  assert.throws(() => lifecycleFromGraphqlPull({ ...pull, timelineItems: { ...pull.timelineItems, pageInfo: { hasPreviousPage: true } } }), /truncated/);
  assert.throws(() => lifecycleFromGraphqlPull({ ...pull, timelineItems: { ...pull.timelineItems, nodes: [...pull.timelineItems.nodes].reverse() } }), /order/);
  assert.throws(() => lifecycleFromGraphqlPull({ ...pull, state: 'CLOSED' }), /conflicts/);
});

test('receipt rejects findings, verdict substitution, unknown keys, and stale generation', () => {
  assert.throws(() => receipt({ finding_count: 1 }), /zero-finding/);
  assert.throws(() => receipt({ verdict: 'fail' }), /zero-finding/);
  assert.throws(() => parseReviewAttestationSummary(`${canonicalJson(receipt()).slice(0, -1)},"extra":1}`), /canonical|unexpected/);
  assert.throws(() => parseReviewAttestationSummary(renderReviewAttestation(receipt()).summary, { expectedGeneration: { ...generation, head_sha: sha('d') } }), /generation/);
});
