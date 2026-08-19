import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

export const POLICY_ARTIFACT_SCHEMA_VERSION = 1;
export const MAX_POLICY_ARTIFACT_BYTES = 64 * 1024;

const SHA = /^[0-9a-f]{40}$/;
const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const REASON = /^[a-z][a-z0-9_]{0,79}$/;
const TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?Z$/;

function fail(message) {
  throw new Error(message);
}

function requireCondition(condition, message) {
  if (!condition) fail(message);
}

function isObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function exactKeys(value, keys, name) {
  requireCondition(isObject(value), `${name} must be an object`);
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  requireCondition(
    actual.length === expected.length && actual.every((key, index) => key === expected[index]),
    `${name} has unexpected keys`,
  );
}

function string(value, name, maximumLength, pattern = null) {
  requireCondition(typeof value === 'string' && value.length > 0 && value.length <= maximumLength, `${name} is invalid`);
  requireCondition(!/[\u0000-\u001f\u007f]/.test(value), `${name} contains control characters`);
  if (pattern) requireCondition(pattern.test(value), `${name} format is invalid`);
  return value;
}

function positiveInteger(value, name) {
  requireCondition(Number.isSafeInteger(value) && value > 0, `${name} must be a positive safe integer`);
  return value;
}

function stringArray(value, name, { maximumItems, maximumLength, pattern = null, paths = false }) {
  requireCondition(Array.isArray(value) && value.length <= maximumItems, `${name} is invalid`);
  const result = value.map((item, index) => {
    const normalized = string(item, `${name}[${index}]`, maximumLength, pattern);
    if (paths) {
      requireCondition(!normalized.includes('\\') && !normalized.startsWith('/') && !normalized.endsWith('/'), `${name}[${index}] is invalid`);
      requireCondition(
        normalized.split('/').every((segment) => segment.length > 0 && segment !== '.' && segment !== '..'),
        `${name}[${index}] is invalid`,
      );
    }
    return normalized;
  });
  requireCondition(new Set(result).size === result.length, `${name} must not contain duplicates`);
  return result;
}

function jsonBytes(value) {
  let encoded;
  try {
    encoded = JSON.stringify(value);
  } catch {
    fail('policy artifact must be JSON serializable');
  }
  requireCondition(encoded !== undefined, 'policy artifact must be JSON serializable');
  return Buffer.byteLength(encoded, 'utf8');
}

function validateResult(value) {
  exactKeys(
    value,
    [
      'mode',
      'verdict',
      'enforcement',
      'eligible_for_automatic_merge',
      'reason_codes',
      'unsuccessful_checks',
      'human_review_paths',
      'changed_file_count',
    ],
    'policy result',
  );
  requireCondition(['shadow', 'human'].includes(value.mode), 'policy result mode is invalid');
  requireCondition(['pass', 'block', 'pending', 'human_required'].includes(value.verdict), 'policy result verdict is invalid');
  requireCondition(['advisory', 'enforced'].includes(value.enforcement), 'policy result enforcement is invalid');
  requireCondition(typeof value.eligible_for_automatic_merge === 'boolean', 'policy result eligibility is invalid');
  requireCondition(value.enforcement === 'advisory', 'Phase 4 policy result must remain advisory');
  requireCondition(value.eligible_for_automatic_merge === false, 'Phase 4 policy result cannot authorize automatic merge');
  const reasonCodes = stringArray(value.reason_codes, 'policy result reason_codes', {
    maximumItems: 32,
    maximumLength: 80,
    pattern: REASON,
  });
  const unsuccessfulChecks = stringArray(value.unsuccessful_checks, 'policy result unsuccessful_checks', {
    maximumItems: 20,
    maximumLength: 160,
  });
  const humanReviewPaths = stringArray(value.human_review_paths, 'policy result human_review_paths', {
    maximumItems: 100,
    maximumLength: 1024,
    paths: true,
  });
  requireCondition(
    Number.isSafeInteger(value.changed_file_count) && value.changed_file_count > 0 && value.changed_file_count <= 300,
    'policy result changed_file_count is invalid',
  );
  return {
    mode: value.mode,
    verdict: value.verdict,
    enforcement: value.enforcement,
    eligible_for_automatic_merge: value.eligible_for_automatic_merge,
    reason_codes: reasonCodes,
    unsuccessful_checks: unsuccessfulChecks,
    human_review_paths: humanReviewPaths,
    changed_file_count: value.changed_file_count,
  };
}

export function validatePolicyEvaluationArtifact(value) {
  exactKeys(
    value,
    [
      'schema_version',
      'artifact_type',
      'repository_id',
      'repository',
      'pull_number',
      'head_sha',
      'base_sha',
      'policy_sha',
      'snapshot_sha',
      'evaluated_at',
      'result',
    ],
    'policy evaluation artifact',
  );
  requireCondition(value.schema_version === POLICY_ARTIFACT_SCHEMA_VERSION, 'policy artifact schema_version is invalid');
  requireCondition(value.artifact_type === 'policy_evaluation', 'policy artifact_type is invalid');
  requireCondition(jsonBytes(value) <= MAX_POLICY_ARTIFACT_BYTES, 'policy artifact exceeds maximum size');
  string(value.repository, 'policy repository', 200, REPOSITORY);
  string(value.head_sha, 'policy head_sha', 40, SHA);
  string(value.base_sha, 'policy base_sha', 40, SHA);
  string(value.policy_sha, 'policy policy_sha', 40, SHA);
  string(value.snapshot_sha, 'policy snapshot_sha', 64, /^[0-9a-f]{64}$/);
  string(value.evaluated_at, 'policy evaluated_at', 64, TIMESTAMP);
  requireCondition(Number.isFinite(Date.parse(value.evaluated_at)), 'policy evaluated_at is invalid');
  return {
    schema_version: POLICY_ARTIFACT_SCHEMA_VERSION,
    artifact_type: 'policy_evaluation',
    repository_id: positiveInteger(value.repository_id, 'policy repository_id'),
    repository: value.repository,
    pull_number: positiveInteger(value.pull_number, 'policy pull_number'),
    head_sha: value.head_sha,
    base_sha: value.base_sha,
    policy_sha: value.policy_sha,
    snapshot_sha: value.snapshot_sha,
    evaluated_at: value.evaluated_at,
    result: validateResult(value.result),
  };
}

export function validatePolicyReceiptArtifact(value) {
  exactKeys(
    value,
    [
      'schema_version',
      'artifact_type',
      'state',
      'evaluation',
      'run_id',
      'policy_app_id',
      'check_run_id',
      'check_url',
      'conclusion',
      'published_at',
    ],
    'policy receipt artifact',
  );
  requireCondition(value.schema_version === POLICY_ARTIFACT_SCHEMA_VERSION, 'policy receipt schema_version is invalid');
  requireCondition(value.artifact_type === 'policy_receipt', 'policy receipt artifact_type is invalid');
  requireCondition(value.state === 'published', 'policy receipt state is invalid');
  requireCondition(jsonBytes(value) <= MAX_POLICY_ARTIFACT_BYTES, 'policy receipt exceeds maximum size');
  const evaluation = validatePolicyEvaluationArtifact(value.evaluation);
  string(value.run_id, 'policy receipt run_id', 128, /^[A-Za-z0-9._:-]+$/);
  positiveInteger(value.policy_app_id, 'policy receipt policy_app_id');
  positiveInteger(value.check_run_id, 'policy receipt check_run_id');
  string(value.check_url, 'policy receipt check_url', 2048, /^https:\/\/github\.com\//);
  requireCondition(['neutral', 'success', 'failure'].includes(value.conclusion), 'policy receipt conclusion is invalid');
  const expectedConclusion = evaluation.result.mode === 'shadow'
    ? 'neutral'
    : (['block', 'pending'].includes(evaluation.result.verdict) ? 'failure' : 'success');
  requireCondition(value.conclusion === expectedConclusion, 'policy receipt conclusion does not match its evaluation');
  string(value.published_at, 'policy receipt published_at', 64, TIMESTAMP);
  requireCondition(Number.isFinite(Date.parse(value.published_at)), 'policy receipt published_at is invalid');
  return {
    schema_version: POLICY_ARTIFACT_SCHEMA_VERSION,
    artifact_type: 'policy_receipt',
    state: 'published',
    evaluation,
    run_id: value.run_id,
    policy_app_id: value.policy_app_id,
    check_run_id: value.check_run_id,
    check_url: value.check_url,
    conclusion: value.conclusion,
    published_at: value.published_at,
  };
}

export function readPolicyArtifact(filePath, expectedType = null) {
  const stat = fs.statSync(filePath);
  requireCondition(stat.isFile(), 'policy artifact path must be a file');
  requireCondition(stat.size <= MAX_POLICY_ARTIFACT_BYTES, 'policy artifact exceeds maximum size');
  const text = fs.readFileSync(filePath, 'utf8');
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch {
    fail('policy artifact is not valid JSON');
  }
  const type = expectedType ?? parsed?.artifact_type;
  if (type === 'policy_evaluation') return validatePolicyEvaluationArtifact(parsed);
  if (type === 'policy_receipt') return validatePolicyReceiptArtifact(parsed);
  fail('policy artifact_type is unsupported');
}

export function writePolicyArtifactAtomic(filePath, value, expectedType = null) {
  const type = expectedType ?? value?.artifact_type;
  const artifact = type === 'policy_evaluation'
    ? validatePolicyEvaluationArtifact(value)
    : type === 'policy_receipt'
      ? validatePolicyReceiptArtifact(value)
      : fail('policy artifact_type is unsupported');
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  const temporary = path.join(
    path.dirname(filePath),
    `.${path.basename(filePath)}.${process.pid}.${Date.now()}.${Math.random().toString(16).slice(2)}.tmp`,
  );
  try {
    fs.writeFileSync(temporary, `${JSON.stringify(artifact)}${os.EOL}`, { encoding: 'utf8', mode: 0o600, flag: 'wx' });
    fs.renameSync(temporary, filePath);
  } finally {
    if (fs.existsSync(temporary)) fs.rmSync(temporary, { force: true });
  }
  return artifact;
}
