import assert from 'node:assert/strict';
import test from 'node:test';

import {
  WRITER_PUBLISHER_CHECK_NAME,
  WriterPublisherAttestationError,
  createWriterPublisherTarget,
  createWriterPublisherCheckRun,
  decodeWriterPublisherAttestationSummary,
  parseWriterPublisherTarget,
  serializeWriterPublisherTarget,
  validateWriterPublisherTarget,
  writerPublisherAttestationSha256,
  validateWriterPublisherCheckRun,
  writerPublisherAttestationExternalId,
  writerPublisherAttestationSummary,
} from '../src/autonomy-publisher-attestation.mjs';

const REPOSITORY = 'JinPengGeng/aeris-token';
const REPOSITORY_ID = 1316750512;
const HEAD_SHA = 'a'.repeat(40);
const BASE_SHA = 'b'.repeat(40);
const WRITER_APP = Object.freeze({ app_id: 4667256, app_slug: 'aeris-token-writer' });

function attestation(overrides = {}) {
  return {
    schema_version: 1,
    repository: REPOSITORY,
    repository_id: REPOSITORY_ID,
    task_id: 'issue:74',
    issue_number: 74,
    pull_number: 91,
    head_ref: 'agent/issue-74',
    head_sha: HEAD_SHA,
    base_ref: 'refs/heads/main',
    base_sha: BASE_SHA,
    patch_sha256: 'c'.repeat(64),
    candidate_run_id: '123456',
    candidate_run_attempt: 2,
    publisher_run_id: '987654',
    publisher_run_attempt: 1,
    executor: {
      id: 'workspace-candidate',
      protocol: 'openai-responses',
      kind: 'workspace_candidate',
      action_sha: 'd'.repeat(40),
      tool_version: '1.2.3',
    },
    ...overrides,
  };
}

function checkRun(value = attestation()) {
  return {
    id: 19,
    ...createWriterPublisherCheckRun(value),
    app: { id: WRITER_APP.app_id, slug: WRITER_APP.app_slug },
    repository: { id: REPOSITORY_ID, full_name: REPOSITORY },
  };
}

function encodedSummary(payload) {
  return `aeris-autonomy-publisher-attestation:v1:${Buffer.from(payload, 'utf8').toString('base64url')}`;
}

test('Writer publisher attestation has a canonical, stable, round-trip payload', () => {
  const value = attestation();
  const summary = writerPublisherAttestationSummary(value);
  const decoded = decodeWriterPublisherAttestationSummary(summary);

  assert.deepEqual(decoded, value);
  assert.equal(writerPublisherAttestationSummary(decoded), summary);
  assert.equal(writerPublisherAttestationExternalId(decoded), writerPublisherAttestationExternalId(value));

  const check = checkRun(value);
  assert.equal(check.name, WRITER_PUBLISHER_CHECK_NAME);
  assert.deepEqual(validateWriterPublisherCheckRun(check, { attestation: value, writer_app: WRITER_APP }), {
    id: check.id,
    attestation: decoded,
  });
});

test('Writer publisher check run rejects a wrong App, head, external ID, or summary binding', () => {
  const expected = attestation();
  const cases = [
    { app: { id: WRITER_APP.app_id + 1, slug: WRITER_APP.app_slug } },
    { head_sha: 'e'.repeat(40) },
    { external_id: 'aeris-pub-v1-incorrect' },
    { output: { ...checkRun(expected).output, summary: writerPublisherAttestationSummary(attestation({ candidate_run_id: '123457' })) } },
  ];

  for (const mutation of cases) {
    const check = { ...checkRun(expected), ...mutation };
    assert.throws(
      () => validateWriterPublisherCheckRun(check, { attestation: expected, writer_app: WRITER_APP }),
      WriterPublisherAttestationError,
    );
  }
});

test('Writer publisher attestation decoder rejects noncanonical and malformed payloads', () => {
  const value = attestation();
  const noncanonical = JSON.stringify({ repository: value.repository, ...value });
  const withExtraField = JSON.stringify({ ...value, unexpected: true });
  const invalidUtf8 = 'aeris-autonomy-publisher-attestation:v1:wyg';
  const invalidBase64 = 'aeris-autonomy-publisher-attestation:v1:abc=';

  for (const summary of [
    encodedSummary(noncanonical),
    encodedSummary(withExtraField),
    invalidUtf8,
    invalidBase64,
  ]) {
    assert.throws(() => decodeWriterPublisherAttestationSummary(summary), WriterPublisherAttestationError);
  }
});

test('Writer publisher check run remains bound to every signed payload field', () => {
  const expected = attestation();
  const boundFields = [
    ['base_sha', 'f'.repeat(40)],
    ['pull_number', 92],
    ['patch_sha256', '0'.repeat(64)],
    ['candidate_run_id', '123457'],
    ['publisher_run_attempt', 2],
    ['executor', { ...expected.executor, tool_version: '9.9.9' }],
  ];
  const baseline = checkRun(expected);

  for (const [field, changed] of boundFields) {
    const altered = attestation({ [field]: changed });
    const forged = {
      ...baseline,
      output: { ...baseline.output, summary: writerPublisherAttestationSummary(altered) },
    };
    assert.throws(
      () => validateWriterPublisherCheckRun(forged, { attestation: expected, writer_app: WRITER_APP }),
      WriterPublisherAttestationError,
    );
  }
});

test('Writer publisher target is canonical and binds the complete attestation', () => {
  const value = attestation();
  const target = createWriterPublisherTarget(value, 731);
  const serialized = serializeWriterPublisherTarget(target);

  assert.match(serialized, /\n$/);
  assert.equal(serialized.includes('\r'), false);
  assert.equal(target.attestation_sha256, writerPublisherAttestationSha256(value));
  assert.deepEqual(parseWriterPublisherTarget(serialized), target);
  assert.deepEqual(validateWriterPublisherTarget(target, { attestation: value, attestation_check_run_id: 731 }), target);
});

test('Writer publisher target rejects noncanonical, oversized, and drifted bindings', () => {
  const value = attestation();
  const target = createWriterPublisherTarget(value, 731);
  const reordered = `${JSON.stringify({ repository: target.repository, ...target })}\n`;
  const extra = `${JSON.stringify({ ...target, unexpected: true })}\n`;

  for (const source of [reordered, extra, Buffer.alloc(4097, 0x20)]) {
    assert.throws(() => parseWriterPublisherTarget(source), WriterPublisherAttestationError);
  }
  assert.throws(
    () => validateWriterPublisherTarget({ ...target, head_sha: 'f'.repeat(40) }, {
      attestation: value,
      attestation_check_run_id: 731,
    }),
    WriterPublisherAttestationError,
  );
});
