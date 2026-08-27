import crypto from 'node:crypto';

import { validateWorkspaceCandidateExecutor } from './ai-executor-contract.mjs';

const SHA = /^[0-9a-f]{40}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const APP_SLUG = /^[a-z0-9][a-z0-9-]{0,99}$/;
const RUN_ID = /^(?:0|[1-9][0-9]*)$/;
const TASK_ID = /^issue:([1-9][0-9]*)$/;
const BRANCH = /^agent\/issue-([1-9][0-9]*)$/;
const BASE_REF = /^refs\/heads\/main$/;
const BASE64URL = /^[A-Za-z0-9_-]+$/;
const MAXIMUM_SUMMARY_BYTES = 4096;
const MAXIMUM_TARGET_BYTES = 4096;

export const WRITER_PUBLISHER_ATTESTATION_VERSION = 1;
export const WRITER_PUBLISHER_CHECK_NAME = 'Aeris Autonomy Publisher / attestation';
export const WRITER_PUBLISHER_CHECK_TITLE = 'Writer App publisher attestation';
const SUMMARY_PREFIX = 'aeris-autonomy-publisher-attestation:v1:';
const EXTERNAL_ID_PREFIX = 'aeris-pub-v1-';
export const WRITER_PUBLISHER_TARGET_ARTIFACT_TYPE = 'aeris-autonomy-publisher-target';
export const WRITER_PUBLISHER_TARGET_SCHEMA_VERSION = 1;

export class WriterPublisherAttestationError extends Error {
  constructor(message) {
    super(message);
    this.name = 'WriterPublisherAttestationError';
  }
}

function reject(message) {
  throw new WriterPublisherAttestationError(message);
}

function exactKeys(value, keys, name) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) reject(`${name} is invalid`);
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index])) {
    reject(`${name} fields are invalid`);
  }
}

function required(value, name, pattern = null) {
  if (typeof value !== 'string' || value.length === 0 || /[\u0000-\u001f\u007f]/.test(value) ||
      (pattern && !pattern.test(value))) {
    reject(`${name} is invalid`);
  }
  return value;
}

function positiveInteger(value, name) {
  if (!Number.isSafeInteger(value) || value <= 0) reject(`${name} must be a positive integer`);
  return value;
}

function runAttempt(value, name) {
  if (!Number.isSafeInteger(value) || value <= 0) reject(`${name} must be a positive integer`);
  return value;
}

function normalizeExecutor(value) {
  try {
    const executor = validateWorkspaceCandidateExecutor(value, 'Writer publisher attestation executor');
    return Object.freeze({
      id: executor.id,
      protocol: executor.protocol,
      kind: executor.kind,
      action_sha: executor.action_sha,
      tool_version: executor.tool_version,
    });
  } catch {
    reject('Writer publisher attestation executor is invalid');
  }
}

export function normalizeWriterPublisherAttestation(value) {
  exactKeys(value, [
    'base_ref',
    'base_sha',
    'candidate_run_attempt',
    'candidate_run_id',
    'executor',
    'head_ref',
    'head_sha',
    'issue_number',
    'patch_sha256',
    'pull_number',
    'publisher_run_attempt',
    'publisher_run_id',
    'repository',
    'repository_id',
    'schema_version',
    'task_id',
  ], 'Writer publisher attestation');
  if (value.schema_version !== WRITER_PUBLISHER_ATTESTATION_VERSION) {
    reject('Writer publisher attestation version is invalid');
  }
  const taskId = required(value.task_id, 'Writer publisher attestation task ID', TASK_ID);
  const taskIssue = Number(TASK_ID.exec(taskId)[1]);
  const issueNumber = positiveInteger(value.issue_number, 'Writer publisher attestation issue number');
  const headRef = required(value.head_ref, 'Writer publisher attestation head ref', BRANCH);
  const branchIssue = Number(BRANCH.exec(headRef)[1]);
  if (taskIssue !== issueNumber || branchIssue !== issueNumber) {
    reject('Writer publisher attestation issue binding is invalid');
  }
  return Object.freeze({
    schema_version: WRITER_PUBLISHER_ATTESTATION_VERSION,
    repository: required(value.repository, 'Writer publisher attestation repository', REPOSITORY),
    repository_id: positiveInteger(value.repository_id, 'Writer publisher attestation repository ID'),
    task_id: taskId,
    issue_number: issueNumber,
    pull_number: positiveInteger(value.pull_number, 'Writer publisher attestation pull request number'),
    head_ref: headRef,
    head_sha: required(value.head_sha, 'Writer publisher attestation head SHA', SHA),
    base_ref: required(value.base_ref, 'Writer publisher attestation base ref', BASE_REF),
    base_sha: required(value.base_sha, 'Writer publisher attestation base SHA', SHA),
    patch_sha256: required(value.patch_sha256, 'Writer publisher attestation patch digest', SHA256),
    candidate_run_id: required(value.candidate_run_id, 'Writer publisher attestation candidate run ID', RUN_ID),
    candidate_run_attempt: runAttempt(value.candidate_run_attempt, 'Writer publisher attestation candidate run attempt'),
    publisher_run_id: required(value.publisher_run_id, 'Writer publisher attestation publisher run ID', RUN_ID),
    publisher_run_attempt: runAttempt(value.publisher_run_attempt, 'Writer publisher attestation publisher run attempt'),
    executor: normalizeExecutor(value.executor),
  });
}

function canonicalAttestation(attestation) {
  return JSON.stringify(normalizeWriterPublisherAttestation(attestation));
}

export function writerPublisherAttestationSha256(attestation) {
  return crypto.createHash('sha256').update(canonicalAttestation(attestation), 'utf8').digest('hex');
}

export function writerPublisherAttestationExternalId(attestation) {
  const digest = writerPublisherAttestationSha256(attestation);
  return `${EXTERNAL_ID_PREFIX}${digest.slice(0, 26)}`;
}

export function normalizeWriterPublisherTarget(value) {
  exactKeys(value, [
    'artifact_type',
    'attestation_check_run_id',
    'attestation_sha256',
    'base_ref',
    'base_sha',
    'head_ref',
    'head_sha',
    'publisher_run_attempt',
    'publisher_run_id',
    'pull_number',
    'repository',
    'repository_id',
    'schema_version',
  ], 'Writer publisher target');
  if (value.schema_version !== WRITER_PUBLISHER_TARGET_SCHEMA_VERSION ||
      value.artifact_type !== WRITER_PUBLISHER_TARGET_ARTIFACT_TYPE) {
    reject('Writer publisher target version is invalid');
  }
  return Object.freeze({
    schema_version: WRITER_PUBLISHER_TARGET_SCHEMA_VERSION,
    artifact_type: WRITER_PUBLISHER_TARGET_ARTIFACT_TYPE,
    repository: required(value.repository, 'Writer publisher target repository', REPOSITORY),
    repository_id: positiveInteger(value.repository_id, 'Writer publisher target repository ID'),
    publisher_run_id: required(value.publisher_run_id, 'Writer publisher target publisher run ID', RUN_ID),
    publisher_run_attempt: runAttempt(value.publisher_run_attempt, 'Writer publisher target publisher run attempt'),
    pull_number: positiveInteger(value.pull_number, 'Writer publisher target pull request number'),
    head_ref: required(value.head_ref, 'Writer publisher target head ref', BRANCH),
    head_sha: required(value.head_sha, 'Writer publisher target head SHA', SHA),
    base_ref: required(value.base_ref, 'Writer publisher target base ref', BASE_REF),
    base_sha: required(value.base_sha, 'Writer publisher target base SHA', SHA),
    attestation_check_run_id: positiveInteger(value.attestation_check_run_id, 'Writer publisher target check run ID'),
    attestation_sha256: required(value.attestation_sha256, 'Writer publisher target attestation digest', SHA256),
  });
}

export function createWriterPublisherTarget(attestation, attestationCheckRunId) {
  const normalized = normalizeWriterPublisherAttestation(attestation);
  return normalizeWriterPublisherTarget({
    schema_version: WRITER_PUBLISHER_TARGET_SCHEMA_VERSION,
    artifact_type: WRITER_PUBLISHER_TARGET_ARTIFACT_TYPE,
    repository: normalized.repository,
    repository_id: normalized.repository_id,
    publisher_run_id: normalized.publisher_run_id,
    publisher_run_attempt: normalized.publisher_run_attempt,
    pull_number: normalized.pull_number,
    head_ref: normalized.head_ref,
    head_sha: normalized.head_sha,
    base_ref: normalized.base_ref,
    base_sha: normalized.base_sha,
    attestation_check_run_id: positiveInteger(attestationCheckRunId, 'Writer publisher target check run ID'),
    attestation_sha256: writerPublisherAttestationSha256(normalized),
  });
}

export function serializeWriterPublisherTarget(target) {
  const serialized = `${JSON.stringify(normalizeWriterPublisherTarget(target))}\n`;
  if (Buffer.byteLength(serialized, 'utf8') > MAXIMUM_TARGET_BYTES) {
    reject('Writer publisher target is too large');
  }
  return serialized;
}

export function parseWriterPublisherTarget(value) {
  if (!Buffer.isBuffer(value) && typeof value !== 'string') reject('Writer publisher target is invalid');
  const bytes = Buffer.isBuffer(value) ? value : Buffer.from(value, 'utf8');
  if (bytes.length === 0 || bytes.length > MAXIMUM_TARGET_BYTES || !bytes.toString('utf8').endsWith('\n')) {
    reject('Writer publisher target is invalid');
  }
  const text = bytes.toString('utf8');
  if (!Buffer.from(text, 'utf8').equals(bytes)) reject('Writer publisher target is invalid');
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch {
    reject('Writer publisher target is invalid');
  }
  const normalized = normalizeWriterPublisherTarget(parsed);
  if (serializeWriterPublisherTarget(normalized) !== text) reject('Writer publisher target is noncanonical');
  return normalized;
}

export function validateWriterPublisherTarget(target, { attestation, attestation_check_run_id: checkRunId } = {}) {
  const actual = normalizeWriterPublisherTarget(target);
  const expected = createWriterPublisherTarget(attestation, checkRunId);
  if (JSON.stringify(actual) !== JSON.stringify(expected)) reject('Writer publisher target binding is invalid');
  return actual;
}

export function writerPublisherAttestationDetailsUrl(attestation) {
  const normalized = normalizeWriterPublisherAttestation(attestation);
  return `https://github.com/${normalized.repository}/actions/runs/${normalized.publisher_run_id}/attempts/${normalized.publisher_run_attempt}`;
}

export function writerPublisherAttestationSummary(attestation) {
  const canonical = canonicalAttestation(attestation);
  const summary = `${SUMMARY_PREFIX}${Buffer.from(canonical, 'utf8').toString('base64url')}`;
  if (Buffer.byteLength(summary, 'utf8') > MAXIMUM_SUMMARY_BYTES) reject('Writer publisher attestation summary is too large');
  return summary;
}

export function decodeWriterPublisherAttestationSummary(value) {
  if (typeof value !== 'string' || value.length <= SUMMARY_PREFIX.length ||
      Buffer.byteLength(value, 'utf8') > MAXIMUM_SUMMARY_BYTES || !value.startsWith(SUMMARY_PREFIX)) {
    reject('Writer publisher attestation summary is invalid');
  }
  const encoded = value.slice(SUMMARY_PREFIX.length);
  if (!BASE64URL.test(encoded)) reject('Writer publisher attestation summary is invalid');
  const bytes = Buffer.from(encoded, 'base64url');
  if (bytes.length === 0 || bytes.length > MAXIMUM_SUMMARY_BYTES || bytes.toString('base64url') !== encoded) {
    reject('Writer publisher attestation summary is invalid');
  }
  const text = bytes.toString('utf8');
  if (!Buffer.from(text, 'utf8').equals(bytes)) reject('Writer publisher attestation summary is invalid');
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch {
    reject('Writer publisher attestation summary is invalid');
  }
  const normalized = normalizeWriterPublisherAttestation(parsed);
  if (canonicalAttestation(normalized) !== text) reject('Writer publisher attestation summary is noncanonical');
  return normalized;
}

export function createWriterPublisherCheckRun(attestation) {
  const normalized = normalizeWriterPublisherAttestation(attestation);
  return Object.freeze({
    name: WRITER_PUBLISHER_CHECK_NAME,
    head_sha: normalized.head_sha,
    status: 'completed',
    conclusion: 'success',
    external_id: writerPublisherAttestationExternalId(normalized),
    details_url: writerPublisherAttestationDetailsUrl(normalized),
    output: Object.freeze({
      title: WRITER_PUBLISHER_CHECK_TITLE,
      summary: writerPublisherAttestationSummary(normalized),
    }),
  });
}

export function updateWriterPublisherCheckRun(attestation) {
  const created = createWriterPublisherCheckRun(attestation);
  const { head_sha, ...update } = created;
  return Object.freeze(update);
}

function expectedWriterApp(value) {
  exactKeys(value, ['app_id', 'app_slug'], 'Writer publisher attestation App identity');
  return Object.freeze({
    app_id: positiveInteger(value.app_id, 'Writer publisher attestation App ID'),
    app_slug: required(value.app_slug, 'Writer publisher attestation App slug', APP_SLUG),
  });
}

function sameAttestation(left, right) {
  return canonicalAttestation(left) === canonicalAttestation(right);
}

export function validateWriterPublisherCheckRun(check, { attestation, writer_app } = {}) {
  const expected = normalizeWriterPublisherAttestation(attestation);
  const writerApp = expectedWriterApp(writer_app);
  if (!check || typeof check !== 'object' || Array.isArray(check) ||
      !Number.isSafeInteger(check.id) || check.id <= 0 ||
      check.name !== WRITER_PUBLISHER_CHECK_NAME || check.head_sha !== expected.head_sha ||
      check.status !== 'completed' || check.conclusion !== 'success' ||
      check.external_id !== writerPublisherAttestationExternalId(expected) ||
      check.details_url !== writerPublisherAttestationDetailsUrl(expected) ||
      check?.app?.id !== writerApp.app_id || check?.app?.slug !== writerApp.app_slug ||
      check?.repository?.id !== expected.repository_id || check?.repository?.full_name !== expected.repository ||
      check?.output?.title !== WRITER_PUBLISHER_CHECK_TITLE) {
    reject('Writer publisher check run is invalid');
  }
  const actual = decodeWriterPublisherAttestationSummary(check.output.summary);
  if (!sameAttestation(actual, expected)) reject('Writer publisher check run binding is invalid');
  return Object.freeze({ id: check.id, attestation: actual });
}
