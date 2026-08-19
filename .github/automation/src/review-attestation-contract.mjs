import { createHash } from 'node:crypto';

export const REVIEW_ATTESTATION_SCHEMA_VERSION = 1;
export const REVIEW_ATTESTATION_ROLES = Object.freeze(['reviewer', 'security']);
export const REVIEW_ATTESTATION_CHECK_NAMES = Object.freeze({
  reviewer: 'Automation Review Attestation / reviewer',
  security: 'Automation Review Attestation / security',
});
export const GITHUB_ACTIONS_APP = Object.freeze({ id: 15368, slug: 'github-actions' });

const SHA = /^[0-9a-f]{40}$/;
const HASH = /^[0-9a-f]{64}$/;
const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const SAFE = /^[A-Za-z0-9][A-Za-z0-9._:/-]*$/;
const REASON = /^[a-z][a-z0-9_]{0,79}$/;
const TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?Z$/;
const NODE_ID = /^[A-Za-z0-9_=-]{1,256}$/;

function requireCondition(condition, message) {
  if (!condition) throw new Error(message);
}

function exactKeys(value, keys, name) {
  requireCondition(value && typeof value === 'object' && !Array.isArray(value), `${name} must be an object`);
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  requireCondition(actual.length === expected.length && actual.every((key, index) => key === expected[index]), `${name} has unexpected keys`);
}

function string(value, name, maximum, pattern = null) {
  requireCondition(typeof value === 'string' && value.length > 0 && value.length <= maximum, `${name} is invalid`);
  requireCondition(!/[\u0000-\u001f\u007f]/.test(value), `${name} contains control characters`);
  if (pattern) requireCondition(pattern.test(value), `${name} format is invalid`);
  return value;
}

function positive(value, name) {
  requireCondition(Number.isSafeInteger(value) && value > 0, `${name} is invalid`);
  return value;
}

function timestamp(value, name) {
  string(value, name, 64, TIMESTAMP);
  requireCondition(Number.isFinite(Date.parse(value)), `${name} is invalid`);
  return value;
}

function role(value) {
  requireCondition(REVIEW_ATTESTATION_ROLES.includes(value), 'review attestation role is invalid');
  return value;
}

export function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  if (value !== null && typeof value === 'object') {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(',')}}`;
  }
  const encoded = JSON.stringify(value);
  requireCondition(encoded !== undefined, 'value is not canonical JSON');
  return encoded;
}

export function sha256(value) {
  return createHash('sha256').update(value, 'utf8').digest('hex');
}

export function validateLifecycleEpoch(value) {
  exactKeys(value, ['kind', 'pull_node_id', 'pull_created_at', 'reopened_event_id', 'reopened_at'], 'review attestation lifecycle epoch');
  requireCondition(['initial', 'reopened'].includes(value.kind), 'review attestation lifecycle kind is invalid');
  const pullCreatedAt = timestamp(value.pull_created_at, 'review attestation pull creation time');
  string(value.pull_node_id, 'review attestation pull node ID', 256, NODE_ID);
  if (value.kind === 'initial') {
    requireCondition(value.reopened_event_id === null && value.reopened_at === null, 'initial review attestation lifecycle contains a reopen event');
  } else {
    positive(value.reopened_event_id, 'review attestation reopen event ID');
    timestamp(value.reopened_at, 'review attestation reopen time');
    requireCondition(Date.parse(value.reopened_at) >= Date.parse(pullCreatedAt), 'review attestation reopen predates pull creation');
  }
  return Object.freeze({
    kind: value.kind,
    pull_node_id: value.pull_node_id,
    pull_created_at: pullCreatedAt,
    reopened_event_id: value.reopened_event_id,
    reopened_at: value.reopened_at,
  });
}

export function lifecycleEpochId(value) {
  const epoch = validateLifecycleEpoch(value);
  return epoch.kind === 'initial'
    ? `initial:${epoch.pull_node_id}:${epoch.pull_created_at}`
    : `reopened:${epoch.pull_node_id}:${epoch.pull_created_at}:${epoch.reopened_event_id}:${epoch.reopened_at}`;
}

export function lifecycleFromGraphqlPull(pull) {
  requireCondition(pull && typeof pull === 'object' && !Array.isArray(pull), 'GitHub pull lifecycle response is invalid');
  string(pull.id, 'GitHub pull node ID', 256, NODE_ID);
  timestamp(pull.createdAt, 'GitHub pull creation time');
  requireCondition(['OPEN', 'CLOSED', 'MERGED'].includes(pull.state), 'GitHub pull lifecycle state is invalid');
  exactKeys(pull.timelineItems, ['nodes', 'pageInfo'], 'GitHub pull lifecycle timeline');
  requireCondition(pull.timelineItems.pageInfo?.hasPreviousPage === false, 'GitHub pull lifecycle history is truncated');
  requireCondition(Array.isArray(pull.timelineItems.nodes) && pull.timelineItems.nodes.length <= 100, 'GitHub pull lifecycle events are invalid');
  const seen = new Set();
  let expectedType = 'ClosedEvent';
  let previousTime = Date.parse(pull.createdAt);
  const events = pull.timelineItems.nodes.map((event) => {
    requireCondition(event?.__typename === expectedType, 'GitHub pull lifecycle event order is invalid');
    positive(event.databaseId, 'GitHub pull lifecycle event ID');
    requireCondition(!seen.has(event.databaseId), 'GitHub pull lifecycle event ID is duplicated');
    timestamp(event.createdAt, 'GitHub pull lifecycle event time');
    requireCondition(Date.parse(event.createdAt) >= previousTime, 'GitHub pull lifecycle events are not chronological');
    seen.add(event.databaseId);
    previousTime = Date.parse(event.createdAt);
    expectedType = expectedType === 'ClosedEvent' ? 'ReopenedEvent' : 'ClosedEvent';
    return { type: event.__typename, database_id: event.databaseId, created_at: event.createdAt };
  });
  const latest = events.at(-1) ?? null;
  requireCondition(
    pull.state === 'OPEN' ? latest === null || latest.type === 'ReopenedEvent' : latest?.type === 'ClosedEvent',
    'GitHub pull state conflicts with lifecycle history',
  );
  const reopened = [...events].reverse().find((event) => event.type === 'ReopenedEvent') ?? null;
  return validateLifecycleEpoch({
    kind: reopened === null ? 'initial' : 'reopened',
    pull_node_id: pull.id,
    pull_created_at: pull.createdAt,
    reopened_event_id: reopened?.database_id ?? null,
    reopened_at: reopened?.created_at ?? null,
  });
}

export function validateReviewGeneration(value) {
  exactKeys(value, ['repository_id', 'repository', 'pull_number', 'head_sha', 'base_sha', 'policy_sha', 'lifecycle_epoch'], 'review attestation generation');
  return Object.freeze({
    repository_id: positive(value.repository_id, 'review attestation repository ID'),
    repository: string(value.repository, 'review attestation repository', 200, REPOSITORY),
    pull_number: positive(value.pull_number, 'review attestation pull number'),
    head_sha: string(value.head_sha, 'review attestation head SHA', 40, SHA),
    base_sha: string(value.base_sha, 'review attestation base SHA', 40, SHA),
    policy_sha: string(value.policy_sha, 'review attestation policy SHA', 40, SHA),
    lifecycle_epoch: validateLifecycleEpoch(value.lifecycle_epoch),
  });
}

function validateCoverage(value) {
  exactKeys(value, ['complete', 'file_count', 'patch_bytes', 'manifest_sha', 'raw_diff_sha'], 'review attestation coverage');
  requireCondition(value.complete === true, 'successful review attestation coverage is incomplete');
  return Object.freeze({
    complete: true,
    file_count: positive(value.file_count, 'review attestation file count'),
    patch_bytes: positive(value.patch_bytes, 'review attestation patch bytes'),
    manifest_sha: string(value.manifest_sha, 'review attestation manifest SHA', 64, HASH),
    raw_diff_sha: string(value.raw_diff_sha, 'review attestation raw diff SHA', 64, HASH),
  });
}

function validateRequestedModel(value) {
  exactKeys(value, ['alias', 'id'], 'review attestation requested model');
  return Object.freeze({
    alias: string(value.alias, 'review attestation model alias', 128, SAFE),
    id: string(value.id, 'review attestation requested model ID', 256, SAFE),
  });
}

function validateProviderModel(value) {
  exactKeys(value, ['response_id', 'model', 'system_fingerprint'], 'review attestation provider model');
  return Object.freeze({
    response_id: string(value.response_id, 'review attestation provider response ID', 256, SAFE),
    model: string(value.model, 'review attestation provider model', 256, SAFE),
    system_fingerprint: value.system_fingerprint === null ? null : string(value.system_fingerprint, 'review attestation system fingerprint', 256, SAFE),
  });
}

export function validateReviewAttestation(value, { expectedGeneration = null, expectedRole = null } = {}) {
  exactKeys(value, [
    'schema_version', 'artifact_type', 'role', 'repository_id', 'repository', 'pull_number',
    'head_sha', 'base_sha', 'policy_sha', 'lifecycle_epoch', 'input_sha', 'prompt_sha',
    'profile_sha', 'coverage', 'requested_model', 'provider_model', 'result_sha', 'verdict',
    'finding_count', 'run_group_id', 'run_id', 'completed_at',
  ], 'review attestation');
  requireCondition(value.schema_version === REVIEW_ATTESTATION_SCHEMA_VERSION && value.artifact_type === 'review_attestation', 'review attestation schema is invalid');
  const generation = validateReviewGeneration({
    repository_id: value.repository_id,
    repository: value.repository,
    pull_number: value.pull_number,
    head_sha: value.head_sha,
    base_sha: value.base_sha,
    policy_sha: value.policy_sha,
    lifecycle_epoch: value.lifecycle_epoch,
  });
  const normalized = Object.freeze({
    schema_version: 1,
    artifact_type: 'review_attestation',
    role: role(value.role),
    ...generation,
    input_sha: string(value.input_sha, 'review attestation input SHA', 64, HASH),
    prompt_sha: string(value.prompt_sha, 'review attestation prompt SHA', 64, HASH),
    profile_sha: string(value.profile_sha, 'review attestation profile SHA', 64, HASH),
    coverage: validateCoverage(value.coverage),
    requested_model: validateRequestedModel(value.requested_model),
    provider_model: validateProviderModel(value.provider_model),
    result_sha: string(value.result_sha, 'review attestation result SHA', 64, HASH),
    verdict: value.verdict,
    finding_count: value.finding_count,
    run_group_id: string(value.run_group_id, 'review attestation run group ID', 128, /^[A-Za-z0-9._:-]+$/),
    run_id: string(value.run_id, 'review attestation run ID', 128, /^[A-Za-z0-9._:-]+$/),
    completed_at: timestamp(value.completed_at, 'review attestation completion time'),
  });
  requireCondition(normalized.verdict === 'pass' && normalized.finding_count === 0, 'review attestation is not a zero-finding pass');
  if (expectedRole !== null) requireCondition(normalized.role === role(expectedRole), 'review attestation role does not match');
  if (expectedGeneration !== null) {
    requireCondition(canonicalJson(generation) === canonicalJson(validateReviewGeneration(expectedGeneration)), 'review attestation generation does not match');
  }
  requireCondition(Buffer.byteLength(canonicalJson(normalized), 'utf8') <= 32 * 1024, 'review attestation exceeds maximum size');
  return normalized;
}

export function parseReviewAttestationSummary(summary, options = {}) {
  requireCondition(typeof summary === 'string' && Buffer.byteLength(summary, 'utf8') <= 32 * 1024, 'review attestation summary is invalid');
  let value;
  try { value = JSON.parse(summary); } catch { throw new Error('review attestation summary is not JSON'); }
  requireCondition(canonicalJson(value) === summary, 'review attestation summary is not canonical JSON');
  return validateReviewAttestation(value, options);
}

export function reviewAttestationExternalId(value) {
  const receipt = validateReviewAttestation(value);
  return `aeris-review-attestation:v1:${receipt.role}:${sha256(canonicalJson(receipt))}`;
}

export function renderReviewAttestation(value) {
  const receipt = validateReviewAttestation(value);
  return Object.freeze({
    title: `Automation ${receipt.role} attestation: pass`,
    summary: canonicalJson(receipt),
  });
}

export function buildReviewFailure({ role: roleValue, generation, failureCode, runId, recordedAt }) {
  const normalizedGeneration = validateReviewGeneration(generation);
  const value = Object.freeze({
    schema_version: 1,
    artifact_type: 'review_attestation_failure',
    role: role(roleValue),
    generation: normalizedGeneration,
    failure_code: string(failureCode, 'review attestation failure code', 80, REASON),
    run_id: string(runId, 'review attestation failure run ID', 128, /^[A-Za-z0-9._:-]+$/),
    recorded_at: timestamp(recordedAt, 'review attestation failure time'),
  });
  return value;
}

export function reviewFailureExternalId(value) {
  return `aeris-review-attestation-failure:v1:${value.role}:${sha256(canonicalJson(value))}`;
}

export function renderReviewFailure(value) {
  return Object.freeze({
    title: `Automation ${value.role} attestation: failed`,
    summary: canonicalJson(value),
  });
}
