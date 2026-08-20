import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';

export const WRITER_ARTIFACT_SCHEMA_VERSION = 2;
export const MAX_WRITER_ARTIFACT_BYTES = 1024 * 1024;
export const WRITER_FOUNDATION_LIMITS = Object.freeze({
  maximum_files: 50,
  maximum_patch_bytes: 65536,
  maximum_file_size_bytes: 524288,
  maximum_total_file_bytes: 2097152,
  maximum_fix_cycles: 2,
});

const SHA256 = /^[0-9a-f]{64}$/;
const COMMIT_SHA = /^[0-9a-f]{40}$/;
const REPOSITORY_NAME = /^[A-Za-z0-9][A-Za-z0-9_.-]{0,99}\/[A-Za-z0-9][A-Za-z0-9_.-]{0,99}$/;
const ACTOR = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$/;
const COMMAND = /^\/agent (?:implement|retry-write)$/;
const BRANCH = /^agent\/issue-[1-9][0-9]*$/;
const COMMAND_LINE = /^[A-Za-z0-9][A-Za-z0-9 ._/@+=:,;|&(){}[\]-]{0,500}$/;
const TEST_PLAN_ID = /^[a-z][a-z0-9]*(?:-[a-z0-9]+)*-v[1-9][0-9]*$/;
const TEST_PLAN_ORDER = Object.freeze(['diff-check-v1', 'rust-changed-packages-v1', 'frontend-v1']);
const SECRET_KEY = /(?:secret|token|password|passwd|authorization|credential|private[_-]?key|api[_-]?key|bearer|cookie|session)/i;
const SENSITIVE_VALUE_PATTERNS = [
  /(?:\b(?:bearer|basic)\s+[A-Za-z0-9._~+/=-]+|\b(?:authorization|proxy-authorization|x-api-key|x-auth-token|x-github-token|private-token|cookie|set-cookie)\s*:\s*\S+|\bgh(?:p|o|u|s|r)_[A-Za-z0-9_]{20,}\b|\bgithub_pat_[A-Za-z0-9_]{20,}\b|-----BEGIN(?: [A-Z]+)? PRIVATE KEY-----)/i,
  /\bsk-(?:proj-)?[A-Za-z0-9_-]{20,}\b/,
  /\b(?:AKIA|ASIA)[0-9A-Z]{16}\b/,
];
const CANDIDATE_STATES = new Set(['ready', 'rejected']);
const RECEIPT_STATES = new Set(['draft_created', 'draft_updated', 'no_changes', 'rejected', 'stale', 'failed', 'cancelled', 'residue']);
const WINDOWS_RESERVED_DEVICE = /^(?:con|prn|aux|nul|com[1-9¹²³]|lpt[1-9¹²³]|conin\$|conout\$|clock\$)$/i;
const WINDOWS_INVALID_PATH_CHARACTER = /[<>"|?*]/;

function fail(message) { throw new Error(message); }
function requireCondition(condition, message) { if (!condition) fail(message); }
function isObject(value) { return value !== null && typeof value === 'object' && !Array.isArray(value); }

function exactKeys(value, keys, name) {
  requireCondition(isObject(value), `${name} must be an object`);
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  requireCondition(actual.length === expected.length && actual.every((key, index) => key === expected[index]), `${name} has unexpected keys`);
}

function rejectSecretKeys(value, name = 'artifact') {
  if (typeof value === 'string') {
    requireCondition(!SENSITIVE_VALUE_PATTERNS.some((pattern) => pattern.test(value)), `${name} contains a sensitive value`);
    return;
  }
  if (Array.isArray(value)) value.forEach((item, index) => rejectSecretKeys(item, `${name}[${index}]`));
  if (!isObject(value)) return;
  for (const [key, child] of Object.entries(value)) {
    requireCondition(key === 'lease_token' || !SECRET_KEY.test(key), `${name} contains a secret-like key`);
    rejectSecretKeys(child, `${name}.${key}`);
  }
}

function string(value, name, maximum, pattern = null) {
  requireCondition(typeof value === 'string' && value.length > 0 && value.length <= maximum, `${name} length is invalid`);
  requireCondition(!/[\u0000-\u001f\u007f]/.test(value), `${name} contains control characters`);
  if (pattern) requireCondition(pattern.test(value), `${name} format is invalid`);
  return value;
}

function nullableString(value, name, maximum, pattern = null) {
  return value === null ? null : string(value, name, maximum, pattern);
}

function positiveInteger(value, name, maximum = Number.MAX_SAFE_INTEGER) {
  requireCondition(Number.isSafeInteger(value) && value > 0 && value <= maximum, `${name} must be a positive safe integer`);
  return value;
}

function nonNegativeInteger(value, name, maximum = Number.MAX_SAFE_INTEGER) {
  requireCondition(Number.isSafeInteger(value) && value >= 0 && value <= maximum, `${name} must be a non-negative safe integer`);
  return value;
}

function jsonBytes(value) {
  let encoded;
  try { encoded = JSON.stringify(value); } catch { fail('artifact must be JSON serializable'); }
  requireCondition(encoded !== undefined, 'artifact must be JSON serializable');
  return Buffer.byteLength(encoded, 'utf8');
}

function validateEnvelope(value, artifactType, keys) {
  rejectSecretKeys(value);
  exactKeys(value, ['schema_version', 'artifact_type', ...keys], `${artifactType} artifact`);
  requireCondition(value.schema_version === WRITER_ARTIFACT_SCHEMA_VERSION, `${artifactType} schema_version must be ${WRITER_ARTIFACT_SCHEMA_VERSION}`);
  requireCondition(value.artifact_type === artifactType, `artifact_type must be ${artifactType}`);
  requireCondition(jsonBytes(value) <= MAX_WRITER_ARTIFACT_BYTES, 'artifact exceeds maximum size');
}

function validateIdentity(value, name = 'intent') {
  exactKeys(value, ['repository_id', 'repository_name', 'issue_number', 'issue_url', 'issue_updated_at', 'issue_labels', 'input_sha', 'comment_id', 'actor', 'command', 'base_sha', 'source_sha', 'policy_sha', 'config_sha', 'run_id', 'agent', 'branch', 'expected_remote_head', 'pull_metadata_sha', 'lease_token', 'cancel_epoch', 'lease_expires_at'], name);
  const repositoryName = string(value.repository_name, `${name} repository_name`, 201, REPOSITORY_NAME);
  const issueNumber = positiveInteger(value.issue_number, `${name} issue_number`, 10 ** 9);
  requireCondition(Array.isArray(value.issue_labels), `${name} issue_labels must be an array`);
  const issueLabels = value.issue_labels.map((label, index) => string(label, `${name} issue_labels[${index}]`, 100));
  requireCondition(issueLabels.length <= 100 && new Set(issueLabels).size === issueLabels.length, `${name} issue_labels must be unique and bounded`);
  requireCondition(issueLabels.every((label, index) => index === 0 || issueLabels[index - 1].localeCompare(label, 'en') < 0), `${name} issue_labels must use canonical sort order`);
  const identity = {
    repository_id: positiveInteger(value.repository_id, `${name} repository_id`),
    repository_name: repositoryName,
    issue_number: issueNumber,
    issue_url: string(value.issue_url, `${name} issue_url`, 2048),
    issue_updated_at: string(value.issue_updated_at, `${name} issue_updated_at`, 30, /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?Z$/),
    issue_labels: issueLabels,
    input_sha: string(value.input_sha, `${name} input_sha`, 64, SHA256),
    comment_id: positiveInteger(value.comment_id, `${name} comment_id`),
    actor: string(value.actor, `${name} actor`, 39, ACTOR),
    command: string(value.command, `${name} command`, 130, COMMAND),
    base_sha: string(value.base_sha, `${name} base_sha`, 40, COMMIT_SHA),
    source_sha: string(value.source_sha, `${name} source_sha`, 40, COMMIT_SHA),
    policy_sha: string(value.policy_sha, `${name} policy_sha`, 40, COMMIT_SHA),
    config_sha: string(value.config_sha, `${name} config_sha`, 40, COMMIT_SHA),
    run_id: string(value.run_id, `${name} run_id`, 128, /^[A-Za-z0-9][A-Za-z0-9_.:-]*$/),
    agent: string(value.agent, `${name} agent`, 6, /^writer$/),
    branch: string(value.branch, `${name} branch`, 80, BRANCH),
    expected_remote_head: nullableString(value.expected_remote_head, `${name} expected_remote_head`, 40, COMMIT_SHA),
    pull_metadata_sha: nullableString(value.pull_metadata_sha, `${name} pull_metadata_sha`, 64, SHA256),
    lease_token: string(value.lease_token, `${name} lease_token`, 171, /^(?:[0-9a-f]{32,128}|[A-Za-z0-9_-]{43,171})$/),
    cancel_epoch: nonNegativeInteger(value.cancel_epoch, `${name} cancel_epoch`),
    lease_expires_at: string(value.lease_expires_at, `${name} lease_expires_at`, 30, /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?Z$/),
  };
  requireCondition(Number.isFinite(Date.parse(identity.issue_updated_at)), `${name} issue_updated_at must be an ISO timestamp`);
  let issueUrl;
  try { issueUrl = new URL(identity.issue_url); } catch { fail(`${name} issue_url format is invalid`); }
  requireCondition(
    issueUrl.origin === 'https://api.github.com' && issueUrl.username === '' && issueUrl.password === '' &&
      issueUrl.search === '' && issueUrl.hash === '' &&
      issueUrl.pathname === `/repos/${identity.repository_name}/issues/${identity.issue_number}`,
    `${name} issue_url must bind repository and Issue number`,
  );
  requireCondition(identity.branch === `agent/issue-${identity.issue_number}`, `${name} branch must bind the issue number`);
  requireCondition(Number.isFinite(Date.parse(identity.lease_expires_at)), `${name} lease_expires_at must be an ISO timestamp`);
  if (identity.command === '/agent implement') requireCondition(identity.expected_remote_head === null && identity.pull_metadata_sha === null && identity.source_sha === identity.base_sha, `${name} implement must bind source_sha to base_sha and not bind remote PR state`);
  if (identity.command === '/agent retry-write') requireCondition(identity.expected_remote_head !== null && identity.pull_metadata_sha !== null && identity.source_sha === identity.expected_remote_head, `${name} retry-write must bind source_sha to expected_remote_head and detailed PR metadata`);
  return identity;
}

export function writerFenceIsLive(identity, now = new Date()) {
  try {
    const validated = validateIdentity(identity);
    const nowMs = now instanceof Date ? now.getTime() : Number.NaN;
    return Number.isFinite(nowMs) && Date.parse(validated.lease_expires_at) > nowMs;
  } catch { return false; }
}

export function writerFenceHasMargin(identity, now = new Date(), minimumMarginMs = 180_000) {
  try {
    const validated = validateIdentity(identity);
    const nowMs = now instanceof Date ? now.getTime() : Number.NaN;
    return Number.isSafeInteger(minimumMarginMs) && minimumMarginMs >= 0 && Number.isFinite(nowMs) &&
      Date.parse(validated.lease_expires_at) - nowMs >= minimumMarginMs;
  } catch { return false; }
}

export function validateWriteIntentArtifact(value) {
  validateEnvelope(value, 'write_intent', ['intent']);
  return { schema_version: WRITER_ARTIFACT_SCHEMA_VERSION, artifact_type: 'write_intent', intent: validateIdentity(value.intent) };
}

function validateChangedPaths(value, fileCount, maximumFiles) {
  requireCondition(Array.isArray(value) && value.length === fileCount && value.length <= maximumFiles, 'changed_paths count is invalid');
  const paths = value.map((item, index) => {
    const candidate = string(item, `changed_paths[${index}]`, 512);
    requireCondition(candidate === candidate.normalize('NFC'), `changed_paths[${index}] must be NFC`);
    requireCondition(!candidate.startsWith('/') && !/^[A-Za-z]:/.test(candidate) && !candidate.includes('\\') && !candidate.includes(':'), `changed_paths[${index}] must be relative and slash-separated`);
    requireCondition(!candidate.split('/').some((segment) => segment.length === 0 || segment === '.' || segment === '..'), `changed_paths[${index}] contains an unsafe segment`);
    requireCondition(!WINDOWS_INVALID_PATH_CHARACTER.test(candidate), `changed_paths[${index}] contains a Windows-invalid character`);
    const foldedSegments = candidate.split('/').map((segment) => segment.normalize('NFC').toLocaleLowerCase('en-US'));
    requireCondition(!foldedSegments.includes('.git'), `changed_paths[${index}] is protected`);
    requireCondition(!candidate.split('/').some((segment) => /[. ]$/.test(segment) || WINDOWS_RESERVED_DEVICE.test(segment.replace(/\..*$/, ''))), `changed_paths[${index}] contains a Windows-reserved segment`);
    requireCondition(foldedSegments[0] !== '.github', `changed_paths[${index}] is protected`);
    requireCondition(!foldedSegments.includes('codeowners') && !foldedSegments.includes('.gitmodules'), `changed_paths[${index}] is protected`);
    return candidate;
  });
  const identities = paths.map((candidate) => candidate.normalize('NFC').toLocaleLowerCase('en-US'));
  requireCondition(new Set(identities).size === paths.length, 'changed_paths must not have case or NFC conflicts');
  return paths.map((path) => ({ path, ...classifyPath(path) }));
}

function classifyPath(path) {
  const parts = path.split('/');
  if (/^(?:apps|crates)\/[^/]+\/(?:src|tests)\/.*\.rs$/.test(path)) return { family: 'rust', scope: `${parts[0]}/${parts[1]}`, testPlan: ['diff-check-v1', 'rust-changed-packages-v1'] };
  if (/^frontend\/src\/.*\.(?:ts|tsx|vue|css)$/.test(path)) return { family: 'frontend', scope: 'frontend', testPlan: ['diff-check-v1', 'frontend-v1'] };
  if (/^docs\/.*\.md$/.test(path) && path !== 'docs/automation-architecture.md') return { family: 'docs', scope: 'docs', testPlan: ['diff-check-v1'] };
  fail(`changed path is not allowlisted: ${path}`);
}

function validateFileSizes(value, paths, maximumFileSize, totalFileBytes) {
  requireCondition(Array.isArray(value) && value.length === paths.length, 'file_sizes count is invalid');
  const seen = new Set();
  let total = 0;
  const sizes = value.map((entry, index) => {
    exactKeys(entry, ['path', 'bytes'], `file_sizes[${index}]`);
    const candidatePath = string(entry.path, `file_sizes[${index}] path`, 512);
    const normalized = candidatePath.normalize('NFC').toLocaleLowerCase('en-US');
    requireCondition(paths.some((item) => item.path === candidatePath) && !seen.has(normalized), 'file_sizes must bind changed_paths exactly');
    seen.add(normalized);
    const bytes = nonNegativeInteger(entry.bytes, `file_sizes[${index}] bytes`, maximumFileSize);
    total += bytes;
    return { path: candidatePath, bytes };
  });
  requireCondition(total === totalFileBytes, 'total_file_bytes must equal file_sizes');
  return sizes;
}

function validateTests(value, classifications, candidateState) {
  exactKeys(value, ['state', 'plan_ids', 'summary'], 'candidate tests');
  requireCondition(['passed', 'failed', 'not_run'].includes(value.state), 'candidate tests state is invalid');
  requireCondition(Array.isArray(value.plan_ids), 'candidate tests plan_ids are invalid');
  const planIds = value.plan_ids.map((item, index) => string(item, `candidate tests plan_ids[${index}]`, 80, TEST_PLAN_ID));
  requireCondition(new Set(planIds).size === planIds.length, 'candidate tests plan_ids must be unique');
  const expected = [...new Set(classifications.flatMap((item) => item.testPlan))].sort((a, b) => TEST_PLAN_ORDER.indexOf(a) - TEST_PLAN_ORDER.indexOf(b));
  requireCondition(planIds.length === expected.length && planIds.every((id, index) => id === expected[index]), 'candidate tests plan_ids must match changed paths');
  if (candidateState === 'rejected' || classifications.length === 0) requireCondition(value.state === 'not_run' && planIds.length === 0, 'rejected or empty candidate tests must be not_run with no plans');
  else requireCondition(value.state !== 'not_run' && planIds.length > 0, 'ready candidate tests must run trusted plans');
  return { state: value.state, plan_ids: planIds, summary: string(value.summary, 'candidate tests summary', 2000) };
}

function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  if (isObject(value)) return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(',')}}`;
  return JSON.stringify(value);
}

function candidateEffect(candidate) {
  return {
    schema_version: candidate.schema_version, state: candidate.state, intent: candidate.intent, patch_sha: candidate.patch_sha,
    changed_paths: candidate.changed_paths, file_sizes: candidate.file_sizes, file_count: candidate.file_count,
    patch_bytes: candidate.patch_bytes, total_file_bytes: candidate.total_file_bytes, limits: candidate.limits,
    fix_cycle: candidate.fix_cycle, tests: { state: candidate.tests.state, plan_ids: candidate.tests.plan_ids },
  };
}

export function calculateWriterCandidateSha(candidate) { return createHash('sha256').update(canonicalJson(candidateEffect(candidate)), 'utf8').digest('hex'); }

export function validateWriterCandidateArtifact(value) {
  validateEnvelope(value, 'candidate', ['state', 'intent', 'patch_sha', 'candidate_sha', 'changed_paths', 'file_sizes', 'file_count', 'patch_bytes', 'total_file_bytes', 'limits', 'fix_cycle', 'tests']);
  requireCondition(CANDIDATE_STATES.has(value.state), 'candidate state is invalid');
  const intent = validateIdentity(value.intent, 'candidate intent');
  const limits = value.limits;
  exactKeys(limits, Object.keys(WRITER_FOUNDATION_LIMITS), 'candidate limits');
  const maxFiles = positiveInteger(limits.maximum_files, 'candidate limits maximum_files', WRITER_FOUNDATION_LIMITS.maximum_files);
  const maxPatchBytes = positiveInteger(limits.maximum_patch_bytes, 'candidate limits maximum_patch_bytes', WRITER_FOUNDATION_LIMITS.maximum_patch_bytes);
  const maxFileSizeBytes = positiveInteger(limits.maximum_file_size_bytes, 'candidate limits maximum_file_size_bytes', WRITER_FOUNDATION_LIMITS.maximum_file_size_bytes);
  const maxTotalFileBytes = positiveInteger(limits.maximum_total_file_bytes, 'candidate limits maximum_total_file_bytes', WRITER_FOUNDATION_LIMITS.maximum_total_file_bytes);
  const maxFixCycles = nonNegativeInteger(limits.maximum_fix_cycles, 'candidate limits maximum_fix_cycles', WRITER_FOUNDATION_LIMITS.maximum_fix_cycles);
  requireCondition(maxFiles === WRITER_FOUNDATION_LIMITS.maximum_files && maxPatchBytes === WRITER_FOUNDATION_LIMITS.maximum_patch_bytes && maxFileSizeBytes === WRITER_FOUNDATION_LIMITS.maximum_file_size_bytes && maxTotalFileBytes === WRITER_FOUNDATION_LIMITS.maximum_total_file_bytes && maxFixCycles === WRITER_FOUNDATION_LIMITS.maximum_fix_cycles, 'candidate limits must match foundation configuration');
  const fileCount = nonNegativeInteger(value.file_count, 'candidate file_count', maxFiles);
  const patchBytes = nonNegativeInteger(value.patch_bytes, 'candidate patch_bytes', maxPatchBytes);
  const totalFileBytes = nonNegativeInteger(value.total_file_bytes, 'candidate total_file_bytes', maxTotalFileBytes);
  const classifiedPaths = validateChangedPaths(value.changed_paths, fileCount, maxFiles);
  const changedPaths = classifiedPaths.map((item) => item.path);
  const candidate = {
    schema_version: WRITER_ARTIFACT_SCHEMA_VERSION,
    artifact_type: 'candidate',
    state: value.state,
    intent,
    patch_sha: nullableString(value.patch_sha, 'candidate patch_sha', 64, SHA256),
    candidate_sha: string(value.candidate_sha, 'candidate candidate_sha', 64, SHA256),
    changed_paths: changedPaths,
    file_sizes: validateFileSizes(value.file_sizes, classifiedPaths, maxFileSizeBytes, totalFileBytes),
    file_count: fileCount,
    patch_bytes: patchBytes,
    total_file_bytes: totalFileBytes,
    limits: { maximum_files: maxFiles, maximum_patch_bytes: maxPatchBytes, maximum_file_size_bytes: maxFileSizeBytes, maximum_total_file_bytes: maxTotalFileBytes, maximum_fix_cycles: maxFixCycles },
    fix_cycle: nonNegativeInteger(value.fix_cycle, 'candidate fix_cycle', maxFixCycles),
    tests: validateTests(value.tests, classifiedPaths, value.state),
  };
  if (candidate.intent.command === '/agent implement') requireCondition(candidate.fix_cycle === 0, 'implement candidate fix_cycle must be zero');
  if (candidate.intent.command === '/agent retry-write') requireCondition(candidate.fix_cycle > 0, 'retry-write candidate fix_cycle must be positive');
  if (candidate.state === 'ready') requireCondition(candidate.patch_sha !== null && candidate.file_count > 0 && candidate.changed_paths.length > 0 && candidate.patch_bytes > 0, 'ready candidate requires a non-empty patch');
  if (candidate.state === 'rejected') requireCondition(candidate.patch_sha === null && candidate.file_count === 0 && candidate.patch_bytes === 0 && candidate.total_file_bytes === 0 && candidate.changed_paths.length === 0 && candidate.file_sizes.length === 0, 'rejected candidate must not carry a patch');
  requireCondition(candidate.candidate_sha === calculateWriterCandidateSha(candidate), 'candidate candidate_sha does not match canonical effect');
  return candidate;
}

export function validateWriterReceiptArtifact(value) {
  validateEnvelope(value, 'receipt', ['state', 'reason', 'candidate', 'candidate_sha', 'commit_sha', 'ref', 'pr_number', 'pr_url', 'draft']);
  requireCondition(RECEIPT_STATES.has(value.state), 'receipt state is invalid');
  const candidate = validateWriterCandidateArtifact(value.candidate);
  const receipt = {
    schema_version: WRITER_ARTIFACT_SCHEMA_VERSION,
    artifact_type: 'receipt',
    state: value.state,
    reason: string(value.reason, 'receipt reason', 160, /^[A-Za-z][A-Za-z0-9_-]*$/),
    candidate,
    candidate_sha: string(value.candidate_sha, 'receipt candidate_sha', 64, SHA256),
    commit_sha: nullableString(value.commit_sha, 'receipt commit_sha', 40, COMMIT_SHA),
    ref: nullableString(value.ref, 'receipt ref', 80, BRANCH),
    pr_number: value.pr_number === null ? null : positiveInteger(value.pr_number, 'receipt pr_number'),
    pr_url: nullableString(value.pr_url, 'receipt pr_url', 2048),
    draft: value.draft,
  };
  requireCondition(receipt.candidate_sha === candidate.candidate_sha, 'receipt candidate_sha must bind candidate');
  requireCondition(typeof receipt.draft === 'boolean' || receipt.draft === null, 'receipt draft must be boolean or null');
  if (receipt.pr_url !== null) {
    let url;
    try { url = new URL(receipt.pr_url); } catch { fail('receipt pr_url format is invalid'); }
    requireCondition(url.protocol === 'https:' && url.username === '' && url.password === '' && url.hostname === 'github.com', 'receipt pr_url must be a credential-free GitHub HTTPS URL');
    requireCondition(url.search === '' && url.hash === '', 'receipt pr_url must not contain a query or hash');
    requireCondition(/^\/[^/]+\/[^/]+\/pull\/[1-9][0-9]*$/.test(url.pathname), 'receipt pr_url must be a pull request URL');
    requireCondition(url.pathname === `/${candidate.intent.repository_name}/pull/${receipt.pr_number}`, 'receipt pr_url must bind intent repository and pr_number');
  }
  const published = receipt.state === 'draft_created' || receipt.state === 'draft_updated';
  if (published) {
    requireCondition(candidate.state === 'ready', 'draft receipt requires ready candidate');
    requireCondition(candidate.tests.state === 'passed', 'draft receipt requires passed candidate tests');
    if (receipt.state === 'draft_created') requireCondition(candidate.intent.command === '/agent implement', 'draft_created receipt requires implement command');
    if (receipt.state === 'draft_updated') requireCondition(candidate.intent.command === '/agent retry-write', 'draft_updated receipt requires retry-write command');
    requireCondition(receipt.commit_sha !== null && receipt.ref === candidate.intent.branch && receipt.pr_number !== null && receipt.pr_url !== null && receipt.draft === true, 'draft receipt is incomplete or does not bind candidate branch');
    requireCondition(receipt.pr_url.endsWith(`/pull/${receipt.pr_number}`), 'receipt pr_url must bind pr_number');
  } else if (receipt.state !== 'residue') {
    requireCondition(receipt.commit_sha === null && receipt.ref === null && receipt.pr_number === null && receipt.pr_url === null && receipt.draft === null, 'terminal receipt must not claim a remote write');
    if (receipt.state === 'no_changes' || receipt.state === 'rejected') requireCondition(candidate.state === 'rejected', `${receipt.state} receipt requires rejected candidate`);
  } else {
    requireCondition(receipt.pr_url === null, 'residue receipt must not claim a verified pull request URL');
    requireCondition(receipt.draft === null, 'residue receipt must not claim verified Draft state');
    requireCondition(receipt.ref === null || receipt.ref === candidate.intent.branch, 'residue receipt ref must bind candidate branch');
  }
  return receipt;
}

export function validateWriterArtifact(value, expectedType = null) {
  requireCondition(isObject(value), 'artifact must be an object');
  const type = expectedType ?? value.artifact_type;
  const validators = { write_intent: validateWriteIntentArtifact, candidate: validateWriterCandidateArtifact, receipt: validateWriterReceiptArtifact };
  requireCondition(Object.hasOwn(validators, type), `unsupported writer artifact type: ${String(type)}`);
  if (expectedType !== null) requireCondition(value.artifact_type === expectedType, `expected ${expectedType} artifact`);
  return validators[type](value);
}

export function readWriterArtifact(filePath, expectedType = null) {
  const stat = fs.lstatSync(filePath);
  requireCondition(stat.isFile() && !stat.isSymbolicLink(), 'writer artifact must be a regular file');
  requireCondition(stat.size <= MAX_WRITER_ARTIFACT_BYTES, 'writer artifact exceeds maximum size');
  const content = fs.readFileSync(filePath);
  requireCondition(content.length <= MAX_WRITER_ARTIFACT_BYTES, 'writer artifact exceeds maximum size');
  let parsed;
  try { parsed = JSON.parse(content.toString('utf8')); } catch { fail('writer artifact JSON is invalid'); }
  return validateWriterArtifact(parsed, expectedType);
}

export function writeWriterArtifactAtomic(filePath, value, expectedType = null) {
  const artifact = validateWriterArtifact(value, expectedType);
  const serialized = `${JSON.stringify(artifact)}\n`;
  requireCondition(Buffer.byteLength(serialized, 'utf8') <= MAX_WRITER_ARTIFACT_BYTES, 'writer artifact exceeds maximum size');
  const directory = path.dirname(filePath);
  fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
  const temporary = path.join(directory, `.${path.basename(filePath)}.${process.pid}.${Date.now()}.${Math.random().toString(16).slice(2)}.tmp`);
  try {
    fs.writeFileSync(temporary, serialized, { encoding: 'utf8', flag: 'wx', mode: 0o600 });
    fs.renameSync(temporary, filePath);
  } finally {
    try { fs.unlinkSync(temporary); } catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  return artifact;
}
