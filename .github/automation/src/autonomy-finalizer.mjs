import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import { AutonomyPolicyGitHubClient, evaluateAutonomyPolicy } from './autonomy-policy-runtime.mjs';
import {
  executorDescriptorForRoute,
  validateExecutorRegistry,
  validateWorkspaceCandidateExecutor,
} from './ai-executor-contract.mjs';
import { validateWriterPermissions } from './github-app-attestation.mjs';
import {
  GOVERNANCE_FENCE_RULESET_NAME,
  validateGovernanceFence,
  validateWriterSecretLane,
} from './governance-fence.mjs';
import {
  decodeWriterPublisherAttestationSummary,
  normalizeWriterPublisherAttestation,
  parseWriterPublisherTarget,
  validateWriterPublisherTarget,
  validateWriterPublisherCheckRun,
  WRITER_PUBLISHER_CHECK_NAME,
  WriterPublisherAttestationError,
} from './autonomy-publisher-attestation.mjs';
import { policyConfigFromEnvironment } from './run-autonomy-policy.mjs';

const GITHUB_ACTIONS_APP_ID = 15368;
const GITHUB_ACTIONS_APP_SLUG = 'github-actions';
const MANAGED_MARKER = '<!-- aeris-autonomy-managed -->';
const SHA = /^[0-9a-f]{40}$/;
const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const RFC3339_TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}:\d{2})$/;
const REQUIRED_CHECKS = Object.freeze(new Map([
  ['Rust CI / check', Object.freeze({ workflow_name: 'Rust CI', workflow_path: '.github/workflows/rust-ci.yml' })],
  ['Frontend CI / check', Object.freeze({ workflow_name: 'Frontend CI', workflow_path: '.github/workflows/frontend-ci.yml' })],
  ['Automation Policy / gate', Object.freeze({ workflow_name: 'Automation Policy', workflow_path: '.github/workflows/automation-policy.yml' })],
]));
const REQUIRED_PROTECTION_CHECKS = Object.freeze([...REQUIRED_CHECKS.keys()]);
const MANUAL_LABELS = new Set(['autonomy-manual', 'do-not-merge']);
const WORKFLOW_IDENTITIES = Object.freeze(new Map([
  ['Automation Policy', '.github/workflows/automation-policy.yml'],
  ['Rust CI', '.github/workflows/rust-ci.yml'],
  ['Frontend CI', '.github/workflows/frontend-ci.yml'],
]));
const MAXIMUM_GRAPHQL_BYTES = 4 * 1024 * 1024;
const MAXIMUM_EXECUTOR_REGISTRY_BYTES = 65_536;
const MAXIMUM_ACTIVE_RULESETS = 20;
const EXECUTOR_REGISTRY_PATH = '.github/ai-executors.json';
const RESPONSE_LOSS_CANARY_FAULT = 'drop_merge_response_after_success';
const RESPONSE_LOSS_CANARY_MARKER = 'response_loss_after_merge_response';
const BASE64 = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;
const EXECUTOR_TRAILER_FIELDS = Object.freeze({
  'Aeris-Autonomy-Executor-ID': 'id',
  'Aeris-Autonomy-Executor-Protocol': 'protocol',
  'Aeris-Autonomy-Executor-Action-SHA': 'action_sha',
  'Aeris-Autonomy-Executor-Tool-Version': 'tool_version',
});
const PATCH_TRAILER = 'Aeris-Autonomy-Patch';
const SHA256 = /^[0-9a-f]{64}$/;
const CANDIDATE_WORKFLOW = Object.freeze({
  name: 'Agent candidate',
  path: '.github/workflows/agent-candidate.yml',
  event: 'workflow_dispatch',
});
const PUBLISHER_WORKFLOW = Object.freeze({
  name: 'Autonomy Publisher',
  path: '.github/workflows/autonomy-publisher.yml',
  event: 'workflow_run',
});

export class AutonomyFinalizerError extends Error {
  constructor(message) {
    super(message);
    this.name = 'AutonomyFinalizerError';
  }
}

function reject(message) {
  throw new AutonomyFinalizerError(message);
}

function required(value, name, pattern = null) {
  if (typeof value !== 'string' || value.length === 0 || /[\u0000-\u001f\u007f]/.test(value)) reject(`${name} is invalid`);
  if (pattern && !pattern.test(value)) reject(`${name} format is invalid`);
  return value;
}

function timestamp(value, name) {
  const normalized = required(value, name, RFC3339_TIMESTAMP);
  if (!Number.isFinite(Date.parse(normalized))) reject(`${name} is invalid`);
  return normalized;
}

function positiveInteger(value, name) {
  const parsed = typeof value === 'string' && /^[1-9][0-9]*$/.test(value) ? Number(value) : value;
  if (!Number.isSafeInteger(parsed) || parsed <= 0) reject(`${name} must be a positive integer`);
  return parsed;
}

function boolean(value, name) {
  if (typeof value !== 'boolean') reject(`${name} must be a boolean`);
  return value;
}

function nonNegativeInteger(value, name) {
  if (!Number.isSafeInteger(value) || value < 0) reject(`${name} must be a non-negative integer`);
  return value;
}

function object(value, name) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) reject(`${name} must be an object`);
  return value;
}

function exactObjectKeys(value, keys, name) {
  const candidate = object(value, name);
  const actual = Object.keys(candidate).sort();
  const expected = [...keys].sort();
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index])) {
    reject(`${name} has unexpected keys`);
  }
  return candidate;
}

function nullableString(value, name) {
  if (value === null) return null;
  return required(value, name);
}

function multilineString(value, name) {
  if (typeof value !== 'string' || value.length === 0 || /[\u0000\u007f]/.test(value)) reject(`${name} is invalid`);
  return value;
}

function candidateExecutorEquals(left, right) {
  return left.id === right.id && left.protocol === right.protocol && left.kind === right.kind &&
    left.action_sha === right.action_sha && left.tool_version === right.tool_version;
}

function decodeTrustedRegistryFile(value) {
  const file = object(value, 'trusted candidate executor registry response');
  if (file.type !== 'file' || file.encoding !== 'base64' ||
      !Number.isSafeInteger(file.size) || file.size <= 0 || file.size > MAXIMUM_EXECUTOR_REGISTRY_BYTES ||
      typeof file.content !== 'string' || file.content.length === 0 ||
      file.content.length > MAXIMUM_EXECUTOR_REGISTRY_BYTES * 2 || file.content.includes('\r') ||
      /[^A-Za-z0-9+/=\n]/.test(file.content)) {
    reject('trusted candidate executor registry response is invalid');
  }
  const compact = file.content.replaceAll('\n', '');
  if (!BASE64.test(compact)) reject('trusted candidate executor registry response is invalid');
  const bytes = Buffer.from(compact, 'base64');
  if (bytes.length !== file.size || bytes.length === 0 || bytes.length > MAXIMUM_EXECUTOR_REGISTRY_BYTES ||
      bytes.toString('base64') !== compact) {
    reject('trusted candidate executor registry response is invalid');
  }
  const text = bytes.toString('utf8');
  if (!Buffer.from(text, 'utf8').equals(bytes)) reject('trusted candidate executor registry is not UTF-8');
  return text;
}

function candidateExecutorFromRegistryFile(value) {
  let registry;
  try {
    registry = JSON.parse(decodeTrustedRegistryFile(value));
  } catch (error) {
    if (error instanceof AutonomyFinalizerError) throw error;
    reject('trusted candidate executor registry is invalid');
  }
  try {
    return executorDescriptorForRoute(validateExecutorRegistry(registry), 'candidate');
  } catch {
    reject('trusted candidate executor registry is invalid');
  }
}

function candidateExecutorFromCommitMessage(message) {
  if (typeof message !== 'string' || message.length === 0 || message.length > MAXIMUM_EXECUTOR_REGISTRY_BYTES ||
      /[\u0000-\u0009\u000b-\u001f\u007f]/.test(message)) {
    reject('candidate commit executor provenance is invalid');
  }
  const values = Object.create(null);
  for (const line of message.split('\n')) {
    if (!line.startsWith('Aeris-Autonomy-Executor-')) continue;
    const match = /^(Aeris-Autonomy-Executor-(?:ID|Protocol|Action-SHA|Tool-Version)): (.+)$/.exec(line);
    if (!match || !Object.hasOwn(EXECUTOR_TRAILER_FIELDS, match[1])) {
      reject('candidate commit executor provenance is invalid');
    }
    const field = EXECUTOR_TRAILER_FIELDS[match[1]];
    if (Object.hasOwn(values, field)) reject('candidate commit executor provenance has duplicate trailers');
    values[field] = match[2];
  }
  for (const field of Object.values(EXECUTOR_TRAILER_FIELDS)) {
    if (!Object.hasOwn(values, field)) reject('candidate commit executor provenance is incomplete');
  }
  try {
    return validateWorkspaceCandidateExecutor({
      id: values.id,
      protocol: values.protocol,
      kind: 'workspace_candidate',
      action_sha: values.action_sha,
      tool_version: values.tool_version,
    }, 'candidate commit executor');
  } catch {
    reject('candidate commit executor provenance is invalid');
  }
}

function candidatePatchDigestFromCommitMessage(message) {
  if (typeof message !== 'string' || message.length === 0 || message.length > MAXIMUM_EXECUTOR_REGISTRY_BYTES ||
      /[\u0000-\u0009\u000b-\u001f\u007f]/.test(message)) {
    reject('candidate commit patch provenance is invalid');
  }
  let patch = null;
  for (const line of message.split('\n')) {
    if (!line.startsWith(`${PATCH_TRAILER}:`)) continue;
    const match = new RegExp(`^${PATCH_TRAILER}: ([0-9a-f]{64})$`).exec(line);
    if (!match || patch !== null) reject('candidate commit patch provenance is invalid');
    patch = match[1];
  }
  if (patch === null || !SHA256.test(patch)) reject('candidate commit patch provenance is incomplete');
  return patch;
}

async function assertCandidateExecutorProvenance(client, governance) {
  if (typeof client?.getGitCommit !== 'function' || typeof client?.getRepositoryContent !== 'function') {
    reject('Finalizer client cannot verify candidate executor provenance');
  }
  const [commit, registryFile] = await Promise.all([
    client.getGitCommit(governance.headRefOid),
    client.getRepositoryContent(EXECUTOR_REGISTRY_PATH, governance.baseRefOid),
  ]);
  if (commit?.sha !== governance.headRefOid) reject('candidate commit identity drifted during provenance verification');
  const committed = candidateExecutorFromCommitMessage(commit.message);
  const trusted = candidateExecutorFromRegistryFile(registryFile);
  if (!candidateExecutorEquals(committed, trusted)) {
    reject('candidate commit executor provenance does not match the trusted base registry');
  }
  return Object.freeze({ executor: trusted, patch_sha256: candidatePatchDigestFromCommitMessage(commit.message) });
}

export function validateResponseLossCanaryBinding(value, expected) {
  if (value === undefined || value === '') return false;
  if (typeof value !== 'string' || value.length > 1024 || /[\u0000-\u001f\u007f]/.test(value)) {
    reject('Finalizer response-loss canary binding is invalid');
  }
  let binding;
  try { binding = JSON.parse(value); } catch { reject('Finalizer response-loss canary binding is not valid JSON'); }
  object(binding, 'Finalizer response-loss canary binding');
  const requiredKeys = ['base_sha', 'fault', 'head_sha', 'pull_number', 'version'];
  if (JSON.stringify(Object.keys(binding).sort()) !== JSON.stringify(requiredKeys)) {
    reject('Finalizer response-loss canary binding fields are invalid');
  }
  if (binding.version !== 1 || binding.fault !== RESPONSE_LOSS_CANARY_FAULT) {
    reject('Finalizer response-loss canary binding version or fault is invalid');
  }
  const pullNumber = positiveInteger(binding.pull_number, 'Finalizer response-loss canary pull number');
  const headSha = required(binding.head_sha, 'Finalizer response-loss canary head SHA', SHA);
  const baseSha = required(binding.base_sha, 'Finalizer response-loss canary base SHA', SHA);
  if (pullNumber !== expected.pull_number) return false;
  if (headSha !== expected.head_sha || baseSha !== expected.base_sha) {
    reject('Finalizer response-loss canary does not match the exact eligibility snapshot');
  }
  return true;
}

function checkCandidates(checkRuns) {
  if (!Array.isArray(checkRuns)) reject('check run projection is invalid');
  const candidates = new Map([...REQUIRED_CHECKS.keys()].map((name) => [name, []]));
  for (const check of checkRuns) {
    if (!candidates.has(check?.name) || !Number.isSafeInteger(check?.id) || check.id <= 0) continue;
    candidates.get(check.name).push(check);
  }
  for (const values of candidates.values()) values.sort((left, right) => right.id - left.id);
  return candidates;
}

function writerPublisherIdentity(value, config) {
  const trusted = object(value, 'Writer publisher attestation trust');
  const appId = positiveInteger(trusted.app_id, 'Writer publisher App ID');
  const appSlug = required(trusted.app_slug, 'Writer publisher App slug', /^[a-z0-9][a-z0-9-]{0,99}$/);
  if (`${appSlug}[bot]` !== config.writer_login) {
    reject('Writer publisher attestation App does not match policy');
  }
  return Object.freeze({ app_id: appId, app_slug: appSlug });
}

function samePublisherAttestation(left, right) {
  try {
    return JSON.stringify(normalizeWriterPublisherAttestation(left)) === JSON.stringify(normalizeWriterPublisherAttestation(right));
  } catch (error) {
    if (error instanceof WriterPublisherAttestationError) reject(error.message);
    throw error;
  }
}

function validateAttestationWorkflowRun(run, { run_id: runId, run_attempt: runAttempt }, expected, identity, label) {
  if (String(run?.id) !== runId || run?.run_attempt !== runAttempt ||
      run?.name !== identity.name || run?.path !== identity.path || run?.event !== identity.event ||
      run?.status !== 'completed' || run?.conclusion !== 'success' || run?.head_sha !== expected.base_sha ||
      run?.repository?.id !== expected.repository_id || run?.repository?.full_name !== expected.repository ||
      run?.head_repository?.id !== expected.repository_id || run?.head_repository?.full_name !== expected.repository) {
    reject(`${label} workflow run binding is invalid`);
  }
}

function validateCandidateWorkflowRun(run, attestation, expected) {
  validateAttestationWorkflowRun(run, {
    run_id: attestation.candidate_run_id,
    run_attempt: attestation.candidate_run_attempt,
  }, expected, CANDIDATE_WORKFLOW, 'candidate');
}

function validatePublisherWorkflowRun(run, attestation, expected) {
  validateAttestationWorkflowRun(run, {
    run_id: attestation.publisher_run_id,
    run_attempt: attestation.publisher_run_attempt,
  }, expected, PUBLISHER_WORKFLOW, 'Writer publisher');
}

async function writerPublisherAttestationReady(
  client, checkRuns, expected, candidateProvenance, writerApp, expectedCheckRunId = null,
) {
  if (!Array.isArray(checkRuns) || typeof client?.getCheckRun !== 'function' || typeof client?.getWorkflowRun !== 'function') {
    reject('Finalizer client cannot verify Writer attestation');
  }
  const candidates = checkRuns.filter((check) => check?.name === WRITER_PUBLISHER_CHECK_NAME &&
    check?.app?.id === writerApp.app_id && check?.app?.slug === writerApp.app_slug);
  if (candidates.length > 1) reject('managed candidate has ambiguous Writer attestations');
  if (candidates.length === 0) return Object.freeze({ ready: false, reason: 'writer_attestation_missing' });
  const listed = candidates[0];
  if (!Number.isSafeInteger(listed?.id) || listed.id <= 0) reject('Writer attestation check run identity is invalid');
  if (expectedCheckRunId !== null && listed.id !== expectedCheckRunId) {
    reject('Writer attestation check run drifted from the Publisher target');
  }
  if (listed.status !== 'completed' || listed.conclusion !== 'success') {
    return Object.freeze({ ready: false, reason: 'writer_attestation_not_successful' });
  }
  const check = await client.getCheckRun(listed.id);
  if (check?.id !== listed.id) reject('Writer attestation check run identity drifted');
  let attestation;
  try {
    attestation = decodeWriterPublisherAttestationSummary(check?.output?.summary);
    validateWriterPublisherCheckRun(check, { attestation, writer_app: writerApp });
  } catch (error) {
    if (error instanceof WriterPublisherAttestationError) reject(error.message);
    throw error;
  }
  const expectedAttestation = {
    schema_version: 1,
    repository: expected.repository,
    repository_id: expected.repository_id,
    task_id: expected.task_id,
    issue_number: expected.issue_number,
    pull_number: expected.pull_number,
    head_ref: expected.branch_name,
    head_sha: expected.head_sha,
    base_ref: `refs/heads/${expected.base_ref}`,
    base_sha: expected.base_sha,
    patch_sha256: candidateProvenance.patch_sha256,
    candidate_run_id: attestation.candidate_run_id,
    candidate_run_attempt: attestation.candidate_run_attempt,
    publisher_run_id: attestation.publisher_run_id,
    publisher_run_attempt: attestation.publisher_run_attempt,
    executor: candidateProvenance.executor,
  };
  if (!samePublisherAttestation(attestation, expectedAttestation)) {
    reject('Writer publisher attestation does not bind the exact candidate');
  }
  if (expectedCheckRunId !== null &&
      (check?.repository?.id !== expected.repository_id || check?.repository?.full_name !== expected.repository)) {
    reject('Writer attestation check run repository identity is invalid');
  }
  const [candidateRun, publisherRun] = await Promise.all([
    client.getWorkflowRun(attestation.candidate_run_id),
    client.getWorkflowRun(attestation.publisher_run_id),
  ]);
  validateCandidateWorkflowRun(candidateRun, attestation, expected);
  validatePublisherWorkflowRun(publisherRun, attestation, expected);
  return Object.freeze({ ready: true, check_run_id: listed.id });
}

function validateCompleteConnection(connection, name) {
  const value = object(connection, name);
  if (!Array.isArray(value.nodes)) reject(`${name} nodes are invalid`);
  const pageInfo = object(value.pageInfo, `${name} pageInfo`);
  const totalCount = nonNegativeInteger(value.totalCount, `${name} totalCount`);
  if (boolean(pageInfo.hasNextPage, `${name} hasNextPage`) || value.nodes.length !== totalCount) {
    reject(`${name} pagination is incomplete`);
  }
  return value;
}

export function validateBranchProtection(proof, defaultBranch) {
  const repository = object(proof, 'branch protection proof');
  const repositoryProfile = Object.freeze({
    mergeCommitAllowed: false,
    rebaseMergeAllowed: false,
    squashMergeAllowed: true,
    isArchived: false,
    isDisabled: false,
    isLocked: false,
  });
  for (const [field, expected] of Object.entries(repositoryProfile)) {
    if (boolean(repository[field], `repository ${field}`) !== expected) {
      reject(`repository ${field} does not match the autonomous merge profile`);
    }
  }
  const value = validateCompleteConnection(
    repository.branchProtectionRules,
    'branch protection rules',
  );
  if (value.totalCount !== 1) reject('repository branch protection profile is ambiguous');
  const rules = value.nodes.filter((rule) => rule?.pattern === defaultBranch);
  if (rules.length !== 1) reject('default branch protection rule is missing or ambiguous');
  const rule = object(rules[0], 'default branch protection rule');
  if (rule.requiresStatusChecks !== true || rule.requiresStrictStatusChecks !== true) {
    reject('default branch required checks are not strict');
  }
  if (rule.isAdminEnforced !== true) reject('default branch protection is not admin-enforced');
  if (rule.requiresConversationResolution !== true) {
    reject('default branch conversation resolution is not required');
  }
  const booleanProfile = Object.freeze({
    allowsDeletions: false,
    allowsForcePushes: false,
    blocksCreations: false,
    dismissesStaleReviews: true,
    lockAllowsFetchAndMerge: false,
    lockBranch: false,
    requireLastPushApproval: false,
    requiresApprovingReviews: true,
    requiresCodeOwnerReviews: false,
    requiresCommitSignatures: false,
    requiresDeployments: false,
    requiresLinearHistory: true,
    restrictsPushes: false,
    restrictsReviewDismissals: false,
  });
  for (const [field, expected] of Object.entries(booleanProfile)) {
    if (boolean(rule[field], `default branch ${field}`) !== expected) {
      reject(`default branch ${field} does not match the autonomous merge profile`);
    }
  }
  if (nonNegativeInteger(rule.requiredApprovingReviewCount, 'default branch required approving review count') !== 0) {
    reject('default branch requires approving reviews');
  }
  if (!Array.isArray(rule.requiredDeploymentEnvironments) || rule.requiredDeploymentEnvironments.length !== 0) {
    reject('default branch required deployment environments are not empty');
  }
  for (const field of [
    'bypassPullRequestAllowances',
    'bypassForcePushAllowances',
    'pushAllowances',
    'reviewDismissalAllowances',
  ]) {
    const allowance = validateCompleteConnection(rule[field], `default branch ${field}`);
    if (allowance.totalCount !== 0) {
      reject(`default branch ${field} are not empty`);
    }
  }
  if (!Array.isArray(rule.requiredStatusChecks)) reject('required status check descriptions are invalid');
  if (rule.requiredStatusChecks.length !== REQUIRED_PROTECTION_CHECKS.length) {
    reject('default branch required checks contain unexpected entries');
  }
  const contexts = new Set();
  for (const check of rule.requiredStatusChecks) {
    const context = required(check?.context, 'required status check context');
    if (!REQUIRED_PROTECTION_CHECKS.includes(context) || contexts.has(context)) {
      reject(`required status check is missing or ambiguous: ${context}`);
    }
    contexts.add(context);
  }
  for (const context of REQUIRED_PROTECTION_CHECKS) {
    const descriptions = rule.requiredStatusChecks.filter((check) => check?.context === context);
    if (descriptions.length !== 1) reject(`required status check is missing or ambiguous: ${context}`);
    const app = object(descriptions[0].app, `required status check App: ${context}`);
    if (app.databaseId !== GITHUB_ACTIONS_APP_ID || app.slug !== GITHUB_ACTIONS_APP_SLUG) {
      reject(`required status check source is invalid: ${context}`);
    }
  }
  const rulesets = validateCompleteConnection(
    repository.rulesets,
    'branch rulesets including parents',
  );
  let activeGovernanceFence = null;
  for (const ruleset of rulesets.nodes) {
    const enforcement = required(ruleset?.enforcement, 'branch ruleset enforcement');
    if (!['ACTIVE', 'DISABLED', 'EVALUATE'].includes(enforcement)) reject('branch ruleset enforcement is unknown');
    if (ruleset?.target !== 'BRANCH') reject('branch ruleset target is invalid');
    if (enforcement !== 'ACTIVE') continue;
    if (required(ruleset?.name, 'active branch ruleset name') !== GOVERNANCE_FENCE_RULESET_NAME ||
        activeGovernanceFence !== null) {
      reject('an unexpected active branch ruleset can alter merge governance');
    }
    activeGovernanceFence = ruleset;
  }
  return Object.freeze({
    pattern: rule.pattern,
    contexts: REQUIRED_PROTECTION_CHECKS,
    rulesets: rulesets.totalCount,
    governance_fence_ruleset_id: activeGovernanceFence === null
      ? null
      : positiveInteger(activeGovernanceFence.databaseId, 'active governance fence database id'),
    profile: 'direct-squash-v1',
  });
}

function normalizedRulesetSummary(value, name) {
  const ruleset = object(value, name);
  return Object.freeze({
    id: positiveInteger(ruleset.id, `${name} id`),
    name: required(ruleset.name, `${name} name`),
    target: required(ruleset.target, `${name} target`).toLowerCase(),
    enforcement: required(ruleset.enforcement, `${name} enforcement`).toLowerCase(),
  });
}

function redactedFieldShape(value, present) {
  if (!present) return 'missing';
  if (value === null) return 'null';
  if (Array.isArray(value)) return `array:length=${value.length}`;
  return `non-array:${typeof value}`;
}

function writerGovernanceBaseline(value) {
  const trust = object(value, 'Writer governance trust');
  return Object.freeze({
    app_id: positiveInteger(trust.proof_app_id, 'Writer governance proof App id'),
    ruleset_id: positiveInteger(
      trust.governance_fence_ruleset_id,
      'Writer governance fence ruleset id',
    ),
    updated_at: timestamp(
      trust.governance_fence_updated_at,
      'Writer governance fence updated_at',
    ),
  });
}

function normalizedActiveRuleset(value, summary, name, baseline) {
  const ruleset = object(value, name);
  const identity = normalizedRulesetSummary(ruleset, name);
  if (JSON.stringify(identity) !== JSON.stringify(summary)) {
    reject(`${name} identity drifted from the ruleset inventory`);
  }
  if (summary.id !== baseline.ruleset_id) {
    reject(`${name} does not match the pinned Writer governance fence`);
  }
  if (timestamp(ruleset.updated_at, `${name} updated_at`) !== baseline.updated_at) {
    reject(`${name} updated_at does not match the pinned Writer governance fence`);
  }
  if (required(ruleset.current_user_can_bypass, `${name} current user bypass`) !== 'always') {
    reject(`${name} current user cannot always bypass the pinned Writer governance fence`);
  }
  const conditions = exactObjectKeys(ruleset.conditions, ['ref_name'], `${name} conditions`);
  const refName = exactObjectKeys(
    conditions.ref_name,
    ['include', 'exclude'],
    `${name} ref_name condition`,
  );
  if (!Array.isArray(refName.include) || !Array.isArray(refName.exclude) ||
      refName.include.some((entry) => typeof entry !== 'string') ||
      refName.exclude.some((entry) => typeof entry !== 'string')) {
    reject(`${name} ref_name condition is invalid`);
  }
  if (!Array.isArray(ruleset.rules) || ruleset.rules.length > 32) {
    reject(`${name} rules are invalid`);
  }
  const rules = ruleset.rules.map((rule, index) => {
    const ruleName = `${name} rules[${index}]`;
    const candidate = object(rule, ruleName);
    const type = required(candidate.type, `${ruleName}.type`);
    if (type === 'update') {
      const normalized = exactObjectKeys(candidate, ['type', 'parameters'], ruleName);
      const parameters = exactObjectKeys(
        normalized.parameters,
        ['update_allows_fetch_and_merge'],
        `${ruleName}.parameters`,
      );
      if (parameters.update_allows_fetch_and_merge !== false) {
        reject(`${ruleName}.parameters.update_allows_fetch_and_merge must be false`);
      }
    } else {
      exactObjectKeys(candidate, ['type'], ruleName);
    }
    return Object.freeze({ type });
  });
  rules.sort((left, right) => left.type.localeCompare(right.type));
  const bypassPresent = Object.hasOwn(ruleset, 'bypass_actors');
  let bypassActors;
  if (!bypassPresent) {
    // GitHub redacts this field from App tokens without ruleset-write authority.
    bypassActors = [Object.freeze({
      actor_id: baseline.app_id,
      actor_type: 'Integration',
      bypass_mode: 'always',
    })];
  } else if (!Array.isArray(ruleset.bypass_actors) || ruleset.bypass_actors.length > 32) {
    reject(
      `${name} bypass actors are invalid (shape=${redactedFieldShape(
        ruleset.bypass_actors,
        bypassPresent,
      )})`,
    );
  } else {
    bypassActors = ruleset.bypass_actors.map((actor, index) => {
      const normalized = exactObjectKeys(
        actor,
        ['actor_id', 'actor_type', 'bypass_mode'],
        `${name} bypass_actors[${index}]`,
      );
      return Object.freeze({
        actor_id: positiveInteger(normalized.actor_id, `${name} bypass actor id`),
        actor_type: required(normalized.actor_type, `${name} bypass actor type`),
        bypass_mode: required(normalized.bypass_mode, `${name} bypass mode`),
      });
    });
  }
  bypassActors.sort((left, right) => left.actor_id - right.actor_id ||
    left.actor_type.localeCompare(right.actor_type) || left.bypass_mode.localeCompare(right.bypass_mode));
  const include = [...refName.include].sort();
  const exclude = [...refName.exclude].sort();
  return Object.freeze({
    ...summary,
    conditions: Object.freeze({
      ref_name: Object.freeze({
        include: Object.freeze(include),
        exclude: Object.freeze(exclude),
      }),
    }),
    rules: Object.freeze(rules),
    bypass_actors: Object.freeze(bypassActors),
  });
}

function collaboratorPermission(value, name) {
  const collaborator = object(value, name);
  const permissions = object(collaborator.permissions, `${name} permissions`);
  const levels = [
    ['admin', 'ADMIN'],
    ['maintain', 'MAINTAIN'],
    ['push', 'WRITE'],
    ['triage', 'TRIAGE'],
    ['pull', 'READ'],
  ];
  for (const [field] of levels) boolean(permissions[field], `${name} permissions.${field}`);
  const match = levels.find(([field]) => permissions[field] === true);
  if (!match) reject(`${name} permission is missing`);
  return match[1];
}

function normalizedDirectCollaborators(value) {
  const connection = object(value, 'repository direct collaborator inventory');
  if (!Array.isArray(connection.items)) reject('repository direct collaborator items are invalid');
  const items = connection.items.map((entry, index) => {
    const collaborator = object(entry, `repository direct collaborators[${index}]`);
    return Object.freeze({
      login: required(collaborator.login, `repository direct collaborators[${index}].login`),
      database_id: positiveInteger(collaborator.id, `repository direct collaborators[${index}].id`),
      type: required(collaborator.type, `repository direct collaborators[${index}].type`),
      permission: collaboratorPermission(collaborator, `repository direct collaborators[${index}]`),
    });
  });
  items.sort((left, right) => left.database_id - right.database_id || left.login.localeCompare(right.login));
  return Object.freeze({
    affiliation: 'direct',
    items: Object.freeze(items),
    truncated: boolean(connection.truncated, 'repository direct collaborator pagination status'),
  });
}

function normalizedRulesets(value, activeDetails, baseline) {
  const connection = object(value, 'repository ruleset inventory');
  if (!Array.isArray(connection.items)) reject('repository ruleset items are invalid');
  const summaries = connection.items.map((entry, index) =>
    normalizedRulesetSummary(entry, `repository rulesets[${index}]`));
  const detailById = new Map(activeDetails.map(({ summary, detail }, index) => [
    summary.id,
    normalizedActiveRuleset(detail, summary, `active repository rulesets[${index}]`, baseline),
  ]));
  const items = summaries.map((summary) => summary.enforcement === 'active'
    ? detailById.get(summary.id)
    : summary);
  if (items.some((entry) => entry === undefined) || detailById.size !== activeDetails.length) {
    reject('active repository ruleset detail inventory is incomplete');
  }
  items.sort((left, right) => left.id - right.id);
  return Object.freeze({
    includes_parents: true,
    items: Object.freeze(items),
    truncated: boolean(connection.truncated, 'repository ruleset pagination status'),
  });
}

function normalizedWriterSecretLane({ actionsPermissions, workflowPermissions, environment, branchPolicies }) {
  const actions = object(actionsPermissions, 'Writer Actions permissions response');
  const workflow = object(workflowPermissions, 'Writer workflow permissions response');
  const writerEnvironment = object(environment, 'Writer Environment response');
  const deploymentPolicy = object(
    writerEnvironment.deployment_branch_policy,
    'Writer Environment deployment branch policy',
  );
  const policies = object(branchPolicies, 'Writer deployment branch policy response');
  if (!Array.isArray(policies.items)) reject('Writer deployment branch policy items are invalid');
  const items = policies.items.map((entry, index) => {
    const policy = object(entry, `Writer deployment branch policies[${index}]`);
    return Object.freeze({
      name: required(policy.name, `Writer deployment branch policies[${index}].name`),
      type: required(policy.type, `Writer deployment branch policies[${index}].type`),
    });
  });
  items.sort((left, right) => left.type.localeCompare(right.type) || left.name.localeCompare(right.name));
  return Object.freeze({
    actions_permissions: Object.freeze({
      enabled: boolean(actions.enabled, 'Writer Actions enabled'),
      allowed_actions: required(actions.allowed_actions, 'Writer Actions allowed_actions'),
      sha_pinning_required: boolean(actions.sha_pinning_required, 'Writer Actions sha_pinning_required'),
    }),
    workflow_permissions: Object.freeze({
      default_workflow_permissions: required(
        workflow.default_workflow_permissions,
        'Writer default workflow permissions',
      ),
      can_approve_pull_request_reviews: boolean(
        workflow.can_approve_pull_request_reviews,
        'Writer workflow review permission',
      ),
    }),
    environment: Object.freeze({
      name: required(writerEnvironment.name, 'Writer Environment name'),
      custom_branch_policies: boolean(
        deploymentPolicy.custom_branch_policies,
        'Writer Environment custom branch policies',
      ),
      protected_branches: boolean(
        deploymentPolicy.protected_branches,
        'Writer Environment protected branches',
      ),
      can_admins_bypass_secrets_and_variables: boolean(
        writerEnvironment.can_admins_bypass,
        'Writer Environment admin bypass',
      ),
    }),
    deployment_branch_policies: Object.freeze({
      environment_name: 'writer',
      items: Object.freeze(items),
      truncated: boolean(policies.truncated, 'Writer deployment branch policy pagination status'),
    }),
  });
}

export function validateWriterGovernanceSnapshot(snapshot, { trust, writerTrust, classicProtection }) {
  if (classicProtection?.profile !== 'direct-squash-v1') {
    reject('Writer governance proof requires verified classic main protection');
  }
  const value = object(snapshot, 'Writer governance snapshot');
  writerGovernanceBaseline(writerTrust);
  const expected = Object.freeze({
    repository: trust.repository,
    repository_id: trust.repository_id,
    trusted_owner_login: required(writerTrust.proof_app_owner_login, 'Writer proof App owner login'),
    trusted_owner_database_id: positiveInteger(
      writerTrust.proof_app_owner_database_id,
      'Writer proof App owner database id',
    ),
    app_id: positiveInteger(writerTrust.proof_app_id, 'Writer proof App id'),
    app_slug: required(writerTrust.proof_app_slug, 'Writer proof App slug'),
  });
  try {
    const fence = validateGovernanceFence({
      ...object(value.governance_fence, 'Writer governance fence snapshot'),
      classicMainProtectionVerified: true,
    }, expected);
    const secretLane = validateWriterSecretLane(
      object(value.secret_lane, 'Writer secret lane snapshot'),
      { default_branch: trust.default_branch },
    );
    if (!Number.isSafeInteger(classicProtection.governance_fence_ruleset_id) ||
        classicProtection.governance_fence_ruleset_id <= 0 ||
        classicProtection.governance_fence_ruleset_id !== fence.ruleset_id) {
      reject('classic and REST governance fence identities do not match');
    }
    return Object.freeze({ snapshot: value, fence, secret_lane: secretLane });
  } catch (error) {
    if (error instanceof AutonomyFinalizerError) throw error;
    reject(`Writer governance proof failed: ${error instanceof Error ? error.message : 'validation failed'}`);
  }
}

async function proveWriterGovernance(client, context) {
  if (!client || typeof client.getWriterGovernanceSnapshot !== 'function') {
    reject('Writer client cannot read the live governance fence');
  }
  try {
    return validateWriterGovernanceSnapshot(
      await client.getWriterGovernanceSnapshot(context.writerTrust),
      context,
    );
  } catch (error) {
    if (error instanceof AutonomyFinalizerError) throw error;
    reject(`Writer governance snapshot failed: ${error instanceof Error ? error.message : 'read failed'}`);
  }
}

function validateWriterIdentity(repositories, expected) {
  const accessible = object(repositories, 'Writer installation repositories');
  if (nonNegativeInteger(accessible.total_count, 'Writer installation repository count') !== 1 ||
      !Array.isArray(accessible.repositories) || accessible.repositories.length !== 1) {
    reject('Writer token repository scope is not exact');
  }
  const repository = object(accessible.repositories[0], 'Writer installation repository');
  if (positiveInteger(repository.id, 'Writer repository id') !== expected.repository_id ||
      required(repository.full_name, 'Writer repository full_name', REPOSITORY) !== expected.repository ||
      required(object(repository.owner, 'Writer repository owner').login, 'Writer repository owner login') !== expected.owner) {
    reject('Writer token repository identity is invalid');
  }
  return Object.freeze({ app_id: expected.app_id, app_slug: expected.app_slug, installation_id: expected.installation_id });
}

function validateWriterBot(user, expected) {
  const bot = object(user, 'Writer Bot identity');
  const login = required(bot.login, 'Writer Bot login');
  if (bot.type !== 'Bot' || login !== expected.writer_login || boolean(bot.site_admin, 'Writer Bot site_admin')) {
    reject('Writer Bot REST identity is invalid');
  }
  return Object.freeze({
    login,
    graphql_login: expected.graphql_login,
    database_id: positiveInteger(bot.id, 'Writer Bot database id'),
    node_id: required(bot.node_id, 'Writer Bot node id'),
  });
}

async function proveWriterIdentity(writerClient, expected) {
  const [repositories, bot] = await Promise.all([
    writerClient.getInstallationRepositories(),
    writerClient.getUser(expected.writer_login),
  ]);
  return Object.freeze({
    installation: validateWriterIdentity(repositories, expected),
    bot: validateWriterBot(bot, expected),
  });
}

function workflowRunIdFromCheck(check, repository) {
  if (typeof check?.details_url !== 'string') return null;
  let url;
  try { url = new URL(check.details_url); } catch { return null; }
  if (url.protocol !== 'https:' || url.hostname !== 'github.com' || url.search || url.hash) return null;
  const escapedRepository = repository.split('/').map((part) => part.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('\\/');
  const match = new RegExp(`^/${escapedRepository}/actions/runs/([1-9][0-9]*)/job/[1-9][0-9]*$`, 'i').exec(url.pathname);
  if (!match) return null;
  const runId = Number(match[1]);
  return Number.isSafeInteger(runId) ? runId : null;
}

function checkRunMatches(run, check, identity, expected, runId) {
  return run?.id === runId && run?.check_suite_id === check?.check_suite?.id &&
    run?.name === identity.workflow_name && run?.path === identity.workflow_path && run?.event === 'pull_request' &&
    run?.head_sha === expected.head_sha && run?.repository?.id === expected.repository_id &&
    run?.repository?.full_name === expected.repository && run?.head_repository?.id === expected.repository_id &&
    run?.head_repository?.full_name === expected.repository && Array.isArray(run?.pull_requests) &&
    run.pull_requests.length === 1 && run.pull_requests[0]?.number === expected.pull_number;
}

function checkSuiteMatches(suite, suiteId, expected) {
  return suite?.id === suiteId && suite?.head_sha === expected.head_sha &&
    suite?.app?.id === GITHUB_ACTIONS_APP_ID && suite?.app?.slug === 'github-actions' &&
    Array.isArray(suite?.pull_requests) && suite.pull_requests.length === 1 &&
    suite.pull_requests[0]?.number === expected.pull_number;
}

async function requiredChecksReady(client, checkRuns, expected) {
  const candidates = checkCandidates(checkRuns);
  const unsuccessful = [];
  for (const [name, identity] of REQUIRED_CHECKS) {
    let matched = null;
    let matchedRun = null;
    for (const check of candidates.get(name)) {
      if (check?.head_sha !== expected.head_sha || !Number.isSafeInteger(check?.check_suite?.id) || check.check_suite.id <= 0 ||
          check?.app?.id !== GITHUB_ACTIONS_APP_ID || check?.app?.slug !== 'github-actions') continue;
      const suite = await client.getCheckSuite(check.check_suite.id);
      if (!checkSuiteMatches(suite, check.check_suite.id, expected)) continue;
      const runId = workflowRunIdFromCheck(check, expected.repository);
      if (runId === null) continue;
      const run = await client.getWorkflowRun(runId);
      if (!checkRunMatches(run, check, identity, expected, runId)) continue;
      matched = check;
      matchedRun = run;
      break;
    }
    if (matched?.status !== 'completed' || matched?.conclusion !== 'success' ||
        matchedRun?.status !== 'completed' || matchedRun?.conclusion !== 'success') {
      unsuccessful.push(name);
    }
  }
  return Object.freeze({ ready: unsuccessful.length === 0, unsuccessful: Object.freeze(unsuccessful) });
}

function validateWorkflowRun(run, expected) {
  if (run?.id !== expected.run_id || run?.run_attempt !== expected.run_attempt || run?.event !== 'pull_request' ||
      run?.status !== 'completed' || run?.conclusion !== 'success') reject('trigger workflow lifecycle is invalid');
  const expectedPath = WORKFLOW_IDENTITIES.get(run?.name);
  if (!expectedPath || run?.path !== expectedPath) reject('trigger workflow identity is invalid');
  if (run?.repository?.id !== expected.repository_id || run?.repository?.full_name !== expected.repository ||
      run?.head_repository?.id !== expected.repository_id || run?.head_repository?.full_name !== expected.repository) {
    reject('trigger workflow repository identity is invalid');
  }
  const headSha = required(run.head_sha, 'trigger workflow head SHA', SHA);
  if (!Array.isArray(run.pull_requests) || run.pull_requests.length !== 1) reject('trigger workflow must bind exactly one pull request');
  const pullNumber = positiveInteger(run.pull_requests[0]?.number, 'trigger pull request number');
  return Object.freeze({ pull_number: pullNumber, head_sha: headSha, workflow_name: run.name });
}

function publisherTargetFromPath(targetPath) {
  if (typeof targetPath !== 'string' || targetPath.length === 0 || /[\u0000\r\n]/.test(targetPath)) {
    reject('Publisher target path is invalid');
  }
  const resolved = path.resolve(targetPath);
  let stat;
  try { stat = fs.lstatSync(resolved); } catch { reject('Publisher target artifact is unavailable'); }
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size <= 0 || stat.size > 4096) {
    reject('Publisher target artifact is invalid');
  }
  let value;
  try { value = fs.readFileSync(resolved); } catch { reject('Publisher target artifact is unavailable'); }
  if (value.length !== stat.size) reject('Publisher target artifact changed during read');
  try { return parseWriterPublisherTarget(value); } catch (error) {
    if (error instanceof WriterPublisherAttestationError) reject(error.message);
    throw error;
  }
}

function validatePublisherTriggerRun(run, trigger, trust) {
  if (run?.id !== trigger.run_id || run?.run_attempt !== trigger.run_attempt ||
      run?.name !== PUBLISHER_WORKFLOW.name || run?.path !== PUBLISHER_WORKFLOW.path ||
      run?.event !== PUBLISHER_WORKFLOW.event || run?.status !== 'completed' || run?.conclusion !== 'success' ||
      run?.head_branch !== trust.default_branch || run?.head_sha !== trust.policy_sha ||
      run?.repository?.id !== trust.repository_id || run?.repository?.full_name !== trust.repository ||
      run?.head_repository?.id !== trust.repository_id || run?.head_repository?.full_name !== trust.repository) {
    reject('Publisher trigger workflow run binding is invalid');
  }
}

async function publisherTriggerBinding(client, trigger, trust, writerTrust, config) {
  if (typeof client?.getCheckRun !== 'function') reject('Finalizer client cannot verify Publisher target');
  const run = await client.getWorkflowRun(trigger.run_id);
  validatePublisherTriggerRun(run, trigger, trust);
  const target = publisherTargetFromPath(trigger.publisher_target_path);
  if (target.publisher_run_id !== String(trigger.run_id) || target.publisher_run_attempt !== trigger.run_attempt ||
      target.repository !== trust.repository || target.repository_id !== trust.repository_id ||
      target.base_ref !== `refs/heads/${trust.default_branch}` || target.base_sha !== trust.policy_sha) {
    reject('Publisher target does not bind the source workflow run');
  }
  const writerApp = writerPublisherIdentity(writerTrust, config);
  const check = await client.getCheckRun(target.attestation_check_run_id);
  if (check?.id !== target.attestation_check_run_id || check?.repository?.id !== trust.repository_id ||
      check?.repository?.full_name !== trust.repository) {
    reject('Publisher target attestation check run identity is invalid');
  }
  let attestation;
  try {
    attestation = decodeWriterPublisherAttestationSummary(check?.output?.summary);
    validateWriterPublisherCheckRun(check, { attestation, writer_app: writerApp });
    validateWriterPublisherTarget(target, { attestation, attestation_check_run_id: check.id });
  } catch (error) {
    if (error instanceof WriterPublisherAttestationError) reject(error.message);
    throw error;
  }
  if (attestation.publisher_run_id !== String(trigger.run_id) || attestation.publisher_run_attempt !== trigger.run_attempt ||
      attestation.repository !== trust.repository || attestation.repository_id !== trust.repository_id ||
      attestation.base_ref !== `refs/heads/${trust.default_branch}` || attestation.base_sha !== trust.policy_sha ||
      attestation.pull_number !== target.pull_number || attestation.head_sha !== target.head_sha) {
    reject('Publisher attestation does not bind the source workflow run');
  }
  return Object.freeze({
    pull_number: attestation.pull_number,
    head_sha: attestation.head_sha,
    workflow_name: PUBLISHER_WORKFLOW.name,
    attestation_check_run_id: check.id,
  });
}

function managedIssueBinding(policy, config) {
  const branchPrefix = required(config?.branch_prefix, 'managed branch prefix');
  const branchName = required(policy?.snapshot?.source?.branch, 'managed branch name');
  if (!branchName.startsWith(branchPrefix)) reject('managed pull request branch prefix is invalid');
  const issueNumber = positiveInteger(branchName.slice(branchPrefix.length), 'managed issue number');
  return Object.freeze({
    branch_name: branchName,
    issue_number: issueNumber,
    task_id: `issue:${issueNumber}`,
  });
}

function normalizePullLifecycleEvents(events) {
  if (!Array.isArray(events)) reject('pull request lifecycle event projection is invalid');
  const lifecycle = [];
  const ids = new Set();
  for (const [index, event] of events.entries()) {
    if (!event || typeof event !== 'object' || Array.isArray(event)) {
      reject(`pull request lifecycle event ${index} is invalid`);
    }
    if (!['closed', 'reopened'].includes(event.event)) continue;
    const id = Number.isSafeInteger(event.id) && event.id > 0
      ? String(event.id)
      : typeof event.id === 'string' && /^[1-9][0-9]*$/.test(event.id)
        ? event.id
        : null;
    if (id === null || ids.has(id)) reject('pull request lifecycle event identity is invalid');
    ids.add(id);
    lifecycle.push(Object.freeze({ id, event: event.event }));
  }
  lifecycle.sort((left, right) => left.id.localeCompare(right.id) || left.event.localeCompare(right.event));
  return Object.freeze(lifecycle);
}

function validateGovernance(snapshot, expected, { requireReady = false } = {}) {
  if (!snapshot || typeof snapshot !== 'object') reject('pull request governance projection is invalid');
  if (snapshot.number !== expected.pull_number || snapshot.state !== 'OPEN') reject('pull request is no longer open');
  const lifecycle = normalizePullLifecycleEvents(snapshot.lifecycle);
  if (lifecycle.length !== 0) reject('managed pull request is tombstoned by close or reopen history');
  if (snapshot.headRefOid !== expected.head_sha || snapshot.headRefName !== expected.branch_name ||
      snapshot.headRepository !== expected.repository) reject('managed pull request head identity drifted');
  if (snapshot.baseRefName !== 'main' || snapshot.baseRefOid !== expected.base_sha) reject('managed pull request base drifted');
  const expectedGraphQlLogin = expected.writer_graphql_login ?? expected.writer_login.replace(/\[bot\]$/, '');
  if (snapshot.authorType !== 'Bot' || snapshot.author !== expectedGraphQlLogin) {
    reject('managed pull request author is not the Writer App');
  }
  if (!Number.isSafeInteger(snapshot.authorDatabaseId) || snapshot.authorDatabaseId <= 0 ||
      typeof snapshot.authorId !== 'string' || snapshot.authorId.length === 0) {
    reject('managed pull request author identity is incomplete');
  }
  if (expected.writer_bot && (snapshot.authorId !== expected.writer_bot.node_id ||
      snapshot.authorDatabaseId !== expected.writer_bot.database_id)) {
    reject('managed pull request author does not match the live Writer Bot');
  }
  if (typeof snapshot.body !== 'string' || !snapshot.body.includes(MANAGED_MARKER) ||
      !snapshot.body.includes(`<!-- aeris-autonomy-task:${expected.task_id} -->`)) reject('managed pull request marker is invalid');
  if (!Array.isArray(snapshot.labels) || snapshot.labels.some((label) => MANUAL_LABELS.has(label))) {
    reject('managed pull request has a manual-only label');
  }
  if (!Array.isArray(snapshot.reviewThreads) || snapshot.reviewThreads.some((thread) => thread?.isResolved !== true)) {
    reject('managed pull request has unresolved review discussions');
  }
  if (![null, 'APPROVED'].includes(snapshot.reviewDecision)) reject('managed pull request review decision is blocking');
  if (snapshot.autoMergeRequest !== null) reject('managed pull request has a preexisting auto-merge request');
  if (snapshot.merged !== false || snapshot.mergedAt !== null || snapshot.mergedBy !== null || snapshot.mergeCommit !== null) {
    reject('managed pull request already has merge outcome state');
  }
  if (snapshot.mergeable !== 'MERGEABLE') reject('managed pull request is conflicting or mergeability is unknown');
  if (requireReady) {
    if (snapshot.isDraft !== false || snapshot.mergeStateStatus !== 'CLEAN') reject('managed pull request is not ready and clean');
  } else if (snapshot.isDraft !== true && snapshot.isDraft !== false) {
    reject('managed pull request draft state is invalid');
  }
  return snapshot;
}

async function boundedGraphql(client, query, variables) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 30_000);
  try {
    const response = await client.fetchImpl('https://api.github.com/graphql', {
      method: 'POST',
      signal: controller.signal,
      headers: {
        accept: 'application/vnd.github+json',
        authorization: `Bearer ${client.token}`,
        'content-type': 'application/json',
        'x-github-api-version': '2022-11-28',
      },
      body: JSON.stringify({ query, variables }),
    });
    const text = await response.text();
    if (Buffer.byteLength(text, 'utf8') > MAXIMUM_GRAPHQL_BYTES) reject('GitHub GraphQL response is too large');
    if (!response.ok) reject(`GitHub GraphQL returned HTTP ${response.status}`);
    let value;
    try { value = JSON.parse(text); } catch { reject('GitHub GraphQL returned invalid JSON'); }
    if (!value || (Array.isArray(value.errors) && value.errors.length > 0)) reject('GitHub GraphQL returned errors');
    return value.data;
  } catch (error) {
    if (controller.signal.aborted) reject('GitHub GraphQL request timed out');
    throw error;
  } finally {
    clearTimeout(timeout);
  }
}

function normalizeGovernancePage(pull) {
  object(pull, 'pull request projection');
  if (!Object.hasOwn(pull, 'reviewDecision')) reject('pull request reviewDecision is missing');
  if (!Object.hasOwn(pull, 'autoMergeRequest')) reject('pull request autoMergeRequest is missing');
  for (const field of ['merged', 'mergedAt', 'mergedBy', 'mergeCommit']) {
    if (!Object.hasOwn(pull, field)) reject(`pull request ${field} is missing`);
  }

  const labelsConnection = object(pull.labels, 'pull request labels');
  if (!Array.isArray(labelsConnection.nodes)) reject('pull request label nodes are invalid');
  const labelPageInfo = object(labelsConnection.pageInfo, 'pull request label pageInfo');
  const labelsHaveNext = boolean(labelPageInfo.hasNextPage, 'pull request labels hasNextPage');
  if (labelsHaveNext) reject('pull request labels exceed the governance limit');
  const labels = labelsConnection.nodes.map((label, index) => required(object(label, `pull request labels[${index}]`).name, `pull request labels[${index}].name`));
  if (new Set(labels).size !== labels.length) reject('pull request labels contain duplicates');
  labels.sort();

  const threadsConnection = object(pull.reviewThreads, 'review thread connection');
  if (!Array.isArray(threadsConnection.nodes)) reject('review thread nodes are invalid');
  const threadPageInfo = object(threadsConnection.pageInfo, 'review thread pageInfo');
  const threadsHaveNext = boolean(threadPageInfo.hasNextPage, 'review thread hasNextPage');
  const endCursor = nullableString(threadPageInfo.endCursor, 'review thread endCursor');
  if (threadsHaveNext && endCursor === null) reject('review thread cursor is missing');
  const reviewThreads = threadsConnection.nodes.map((thread, index) => {
    const value = object(thread, `reviewThreads[${index}]`);
    return Object.freeze({
      id: required(value.id, `reviewThreads[${index}].id`),
      isResolved: boolean(value.isResolved, `reviewThreads[${index}].isResolved`),
    });
  });

  let autoMergeRequest = null;
  if (pull.autoMergeRequest !== null) {
    const request = object(pull.autoMergeRequest, 'pull request autoMergeRequest');
    const enabledBy = object(request.enabledBy, 'pull request autoMergeRequest.enabledBy');
    autoMergeRequest = Object.freeze({
      enabledAt: required(request.enabledAt, 'pull request autoMergeRequest.enabledAt'),
      mergeMethod: required(request.mergeMethod, 'pull request autoMergeRequest.mergeMethod'),
      enabledBy: Object.freeze({
        type: required(enabledBy.__typename, 'pull request autoMergeRequest.enabledBy.__typename'),
        login: required(enabledBy.login, 'pull request autoMergeRequest.enabledBy.login'),
        id: required(enabledBy.id, 'pull request autoMergeRequest.enabledBy.id'),
        databaseId: positiveInteger(enabledBy.databaseId, 'pull request autoMergeRequest.enabledBy.databaseId'),
      }),
    });
  }

  let mergedBy = null;
  if (pull.mergedBy !== null) {
    const actor = object(pull.mergedBy, 'pull request mergedBy');
    mergedBy = Object.freeze({
      type: required(actor.__typename, 'pull request mergedBy.__typename'),
      login: required(actor.login, 'pull request mergedBy.login'),
      id: required(actor.id, 'pull request mergedBy.id'),
      databaseId: positiveInteger(actor.databaseId, 'pull request mergedBy.databaseId'),
    });
  }

  let mergeCommit = null;
  if (pull.mergeCommit !== null) {
    const commit = object(pull.mergeCommit, 'pull request mergeCommit');
    const parents = validateCompleteConnection(commit.parents, 'pull request mergeCommit parents');
    mergeCommit = Object.freeze({
      oid: required(commit.oid, 'pull request mergeCommit oid', SHA),
      parents: Object.freeze(parents.nodes.map((parent, index) => Object.freeze({
        oid: required(object(parent, `pull request mergeCommit parents[${index}]`).oid,
          `pull request mergeCommit parents[${index}].oid`, SHA),
      }))),
      parentCount: parents.totalCount,
    });
  }

  const author = object(pull.author, 'pull request author');
  const core = Object.freeze({
    id: required(pull.id, 'pull request node id'),
    number: positiveInteger(pull.number, 'pull request number'),
    state: required(pull.state, 'pull request state'),
    isDraft: boolean(pull.isDraft, 'pull request isDraft'),
    body: multilineString(pull.body, 'pull request body'),
    headRefName: required(pull.headRefName, 'pull request headRefName'),
    headRefOid: required(pull.headRefOid, 'pull request headRefOid', SHA),
    headRepository: required(object(pull.headRepository, 'pull request headRepository').nameWithOwner, 'pull request headRepository.nameWithOwner'),
    baseRefName: required(pull.baseRefName, 'pull request baseRefName'),
    baseRefOid: required(pull.baseRefOid, 'pull request baseRefOid', SHA),
    authorType: required(author.__typename, 'pull request author.__typename'),
    author: required(author.login, 'pull request author.login'),
    authorId: required(author.id, 'pull request author.id'),
    authorDatabaseId: positiveInteger(author.databaseId, 'pull request author.databaseId'),
    mergeable: required(pull.mergeable, 'pull request mergeable'),
    mergeStateStatus: required(pull.mergeStateStatus, 'pull request mergeStateStatus'),
    merged: boolean(pull.merged, 'pull request merged'),
    mergedAt: nullableString(pull.mergedAt, 'pull request mergedAt'),
    mergedBy,
    mergeCommit,
    reviewDecision: nullableString(pull.reviewDecision, 'pull request reviewDecision'),
    autoMergeRequest,
    labels: Object.freeze(labels),
  });
  return Object.freeze({ core, reviewThreads: Object.freeze(reviewThreads), hasNextPage: threadsHaveNext, endCursor });
}

async function readGovernanceSnapshot(client, owner, name, number) {
  let cursor = null;
  let core = null;
  let stableCore = null;
  const threads = [];
  const threadIds = new Set();
  for (let page = 0; page < 10; page += 1) {
    const data = await boundedGraphql(client, `
      query($owner: String!, $name: String!, $number: Int!, $cursor: String) {
        repository(owner: $owner, name: $name) {
          pullRequest(number: $number) {
            id number state isDraft body headRefName headRefOid baseRefName baseRefOid
            headRepository { nameWithOwner }
            author {
              __typename login
              ... on Bot { id databaseId }
              ... on User { id databaseId }
            }
            mergeable mergeStateStatus merged mergedAt reviewDecision
            mergedBy {
              __typename login
              ... on Bot { id databaseId }
              ... on User { id databaseId }
            }
            mergeCommit {
              oid
              parents(first: 2) {
                totalCount nodes { oid } pageInfo { hasNextPage endCursor }
              }
            }
            autoMergeRequest {
              enabledAt mergeMethod
              enabledBy {
                __typename login
                ... on Bot { id databaseId }
                ... on User { id databaseId }
              }
            }
            labels(first: 100) { nodes { name } pageInfo { hasNextPage } }
            reviewThreads(first: 100, after: $cursor) {
              nodes { id isResolved }
              pageInfo { hasNextPage endCursor }
            }
          }
        }
      }`, { owner, name, number, cursor });
    const pageValue = normalizeGovernancePage(data?.repository?.pullRequest);
    const encodedCore = JSON.stringify(pageValue.core);
    if (stableCore === null) {
      stableCore = encodedCore;
      core = pageValue.core;
    } else if (encodedCore !== stableCore) {
      reject('pull request governance drifted during pagination');
    }
    for (const thread of pageValue.reviewThreads) {
      if (threadIds.has(thread.id)) reject('review thread pagination contains duplicates');
      threadIds.add(thread.id);
      threads.push(thread);
    }
    if (!pageValue.hasNextPage) {
      return Object.freeze({ ...core, reviewThreads: Object.freeze(threads) });
    }
    cursor = pageValue.endCursor;
  }
  reject('review thread pagination exceeds the governance limit');
}

export class AutonomyFinalizerGitHubClient extends AutonomyPolicyGitHubClient {
  getRepositoryContent(filePath, ref) {
    if (filePath !== EXECUTOR_REGISTRY_PATH) reject('Finalizer requested an untrusted repository file');
    return this.request(
      'GET',
      `/repos/${this.repository}/contents/${EXECUTOR_REGISTRY_PATH}?ref=${encodeURIComponent(required(ref, 'trusted registry base SHA', SHA))}`,
    );
  }

  getWorkflowRun(runId) {
    return this.request('GET', `/repos/${this.repository}/actions/runs/${runId}`);
  }

  getCheckSuite(suiteId) {
    return this.request('GET', `/repos/${this.repository}/check-suites/${positiveInteger(suiteId, 'check suite id')}`);
  }

  getCheckRun(checkRunId) {
    return this.request('GET', `/repos/${this.repository}/check-runs/${positiveInteger(checkRunId, 'check run ID')}`);
  }

  async getBranchProtection() {
    const [owner, name] = this.repository.split('/');
    const data = await boundedGraphql(this, `
      query($owner: String!, $name: String!) {
        repository(owner: $owner, name: $name) {
          mergeCommitAllowed
          rebaseMergeAllowed
          squashMergeAllowed
          isArchived
          isDisabled
          isLocked
          branchProtectionRules(first: 100) {
            nodes {
              pattern
              allowsDeletions
              allowsForcePushes
              blocksCreations
              dismissesStaleReviews
              requiresStatusChecks
              requiresStrictStatusChecks
              isAdminEnforced
              lockAllowsFetchAndMerge
              lockBranch
              requireLastPushApproval
              requiredApprovingReviewCount
              requiredDeploymentEnvironments
              requiresApprovingReviews
              requiresCodeOwnerReviews
              requiresCommitSignatures
              requiresConversationResolution
              requiresDeployments
              requiresLinearHistory
              restrictsPushes
              restrictsReviewDismissals
              bypassPullRequestAllowances(first: 100) {
                totalCount nodes { id } pageInfo { hasNextPage endCursor }
              }
              bypassForcePushAllowances(first: 100) {
                totalCount nodes { id } pageInfo { hasNextPage endCursor }
              }
              pushAllowances(first: 100) {
                totalCount nodes { id } pageInfo { hasNextPage endCursor }
              }
              reviewDismissalAllowances(first: 100) {
                totalCount nodes { id } pageInfo { hasNextPage endCursor }
              }
              requiredStatusChecks { context app { databaseId slug } }
            }
            totalCount pageInfo { hasNextPage endCursor }
          }
          rulesets(first: 100, includeParents: true, targets: [BRANCH]) {
            nodes { id databaseId name enforcement target }
            totalCount pageInfo { hasNextPage endCursor }
          }
        }
      }`, { owner, name });
    return Object.freeze({
      mergeCommitAllowed: data?.repository?.mergeCommitAllowed,
      rebaseMergeAllowed: data?.repository?.rebaseMergeAllowed,
      squashMergeAllowed: data?.repository?.squashMergeAllowed,
      isArchived: data?.repository?.isArchived,
      isDisabled: data?.repository?.isDisabled,
      isLocked: data?.repository?.isLocked,
      branchProtectionRules: data?.repository?.branchProtectionRules,
      rulesets: data?.repository?.rulesets,
    });
  }

  async readWriterGovernanceSnapshotOnce(writerTrust) {
    const baseline = writerGovernanceBaseline(writerTrust);
    const [
      repository,
      directCollaborators,
      rulesets,
      actionsPermissions,
      workflowPermissions,
      environment,
      branchPolicies,
    ] = await Promise.all([
      this.getRepository(),
      this.listDirectCollaborators(),
      this.listRepositoryRulesetsIncludingParents(),
      this.getActionsPermissions(),
      this.getDefaultWorkflowPermissions(),
      this.getWriterEnvironment(),
      this.listWriterDeploymentBranchPolicies(),
    ]);
    if (directCollaborators?.truncated === true) {
      reject('repository direct collaborator pagination is incomplete');
    }
    if (rulesets?.truncated === true) reject('repository ruleset pagination is incomplete');
    if (branchPolicies?.truncated === true) {
      reject('Writer deployment branch policy pagination is incomplete');
    }
    if (!Array.isArray(rulesets?.items)) reject('repository ruleset items are invalid');
    const active = rulesets.items
      .map((entry, index) => normalizedRulesetSummary(entry, `repository rulesets[${index}]`))
      .filter((entry) => entry.enforcement === 'active');
    if (active.length > MAXIMUM_ACTIVE_RULESETS) {
      reject('active repository ruleset inventory exceeds the governance proof limit');
    }
    const activeDetails = await Promise.all(active.map(async (summary) => Object.freeze({
      summary,
      detail: await this.getRepositoryRuleset(summary.id),
    })));
    const repo = object(repository, 'Writer governance repository response');
    return Object.freeze({
      governance_fence: Object.freeze({
        repository: Object.freeze({
          id: positiveInteger(repo.id, 'Writer governance repository id'),
          full_name: required(repo.full_name, 'Writer governance repository full_name', REPOSITORY),
        }),
        direct_collaborators: normalizedDirectCollaborators(directCollaborators),
        rulesets: normalizedRulesets(rulesets, activeDetails, baseline),
      }),
      secret_lane: normalizedWriterSecretLane({
        actionsPermissions,
        workflowPermissions,
        environment,
        branchPolicies,
      }),
    });
  }

  async getWriterGovernanceSnapshot(writerTrust) {
    const initial = await this.readWriterGovernanceSnapshotOnce(writerTrust);
    const confirmed = await this.readWriterGovernanceSnapshotOnce(writerTrust);
    if (JSON.stringify(initial) !== JSON.stringify(confirmed)) {
      reject('Writer governance drifted between complete reads');
    }
    return confirmed;
  }

  getInstallationRepositories() {
    return this.request('GET', '/installation/repositories?per_page=100&page=1');
  }

  getUser(login) {
    return this.request('GET', `/users/${encodeURIComponent(required(login, 'GitHub user login'))}`);
  }

  async getPullGovernance(number) {
    const [owner, name] = this.repository.split('/');
    const read = async () => Object.freeze({
      ...await readGovernanceSnapshot(this, owner, name, number),
      lifecycle: normalizePullLifecycleEvents(await this.listPullTimelineEvents(number)),
    });
    const initial = await read();
    const confirmed = await read();
    if (JSON.stringify(initial) !== JSON.stringify(confirmed)) reject('pull request governance drifted between complete reads');
    return confirmed;
  }

  async markPullReady(pullRequestId) {
    const data = await boundedGraphql(this, `
      mutation($id: ID!) {
        markPullRequestReadyForReview(input: { pullRequestId: $id }) {
          pullRequest { id isDraft headRefOid }
        }
      }`, { id: pullRequestId });
    return data?.markPullRequestReadyForReview?.pullRequest;
  }

  async convertPullToDraft(pullRequestId) {
    const data = await boundedGraphql(this, `
      mutation($id: ID!) {
        convertPullRequestToDraft(input: { pullRequestId: $id }) {
          pullRequest { id isDraft headRefOid }
        }
      }`, { id: pullRequestId });
    return data?.convertPullRequestToDraft?.pullRequest;
  }

  async mergePullRequest(pullRequestId, expectedHeadOid) {
    const data = await boundedGraphql(this, `
      mutation($id: ID!, $head: GitObjectID!) {
        mergePullRequest(input: { pullRequestId: $id, mergeMethod: SQUASH, expectedHeadOid: $head }) {
          pullRequest {
            id headRefOid merged mergeCommit { oid }
          }
        }
      }`, { id: pullRequestId, head: expectedHeadOid });
    return data?.mergePullRequest?.pullRequest;
  }
}

async function evaluateAutonomyFinalizerGates({
  client, protectionClient, trigger, trust, config, writerTrust = config.writer_trust, proofLevel,
  checkAttempts = 3, sleepImpl = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
}) {
  const bound = trigger.source === 'publisher'
    ? await publisherTriggerBinding(client, trigger, trust, writerTrust, config)
    : validateWorkflowRun(
      await client.getWorkflowRun(trigger.run_id),
      { ...trigger, repository: trust.repository, repository_id: trust.repository_id },
    );
  const policy = await evaluateAutonomyPolicy({ client, trigger: bound, trust, config });
  if (policy.decision.classification !== 'eligible') {
    return Object.freeze({
      eligible: false,
      reason: `policy_${policy.decision.classification}`,
      proof_level: proofLevel,
      bound,
      policy,
    });
  }
  const protection = proofLevel === 'full'
    ? validateBranchProtection(await protectionClient.getBranchProtection(), trust.default_branch)
    : null;
  const issue = managedIssueBinding(policy, config);
  const writerApp = writerPublisherIdentity(writerTrust, config);
  const expected = Object.freeze({
    ...bound,
    ...issue,
    repository: trust.repository,
    repository_id: trust.repository_id,
    base_ref: trust.default_branch,
    base_sha: trust.policy_sha,
    writer_login: config.writer_login,
  });
  let governance;
  let checks;
  let readiness;
  let attestationReadiness;
  for (let attempt = 1; attempt <= checkAttempts; attempt += 1) {
    [governance, checks] = await Promise.all([
      client.getPullGovernance(bound.pull_number),
      client.listCheckRunsForRef(bound.head_sha),
    ]);
    validateGovernance(governance, expected);
    const candidateProvenance = await assertCandidateExecutorProvenance(client, governance);
    readiness = await requiredChecksReady(client, checks, expected);
    attestationReadiness = await writerPublisherAttestationReady(
      client, checks, expected, candidateProvenance, writerApp, bound.attestation_check_run_id ?? null,
    );
    if (readiness.ready && attestationReadiness.ready) {
      return Object.freeze({
        eligible: true,
        reason: null,
        proof_level: proofLevel,
        bound,
        policy,
        governance,
        checks: readiness,
        attestation: attestationReadiness,
        protection,
      });
    }
    if (attempt < checkAttempts) await sleepImpl(2_000 * attempt);
  }
  return Object.freeze({
    eligible: false,
    reason: readiness.ready
      ? attestationReadiness.reason
      : `required_checks_not_successful:${readiness.unsuccessful.join(',')}`,
    proof_level: proofLevel,
    bound,
    policy,
    governance,
  });
}

export async function evaluateAutonomyFinalizerPreliminary({
  client, trigger, trust, config, writerTrust = config.writer_trust,
  checkAttempts = 3, sleepImpl = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
}) {
  return evaluateAutonomyFinalizerGates({
    client,
    protectionClient: null,
    trigger,
    trust,
    config, writerTrust,
    proofLevel: 'preliminary',
    checkAttempts,
    sleepImpl,
  });
}

export async function evaluateAutonomyFinalizer({
  client, protectionClient = client, trigger, trust, config, writerTrust = config.writer_trust,
  checkAttempts = 3, sleepImpl = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
}) {
  if (!protectionClient || typeof protectionClient.getBranchProtection !== 'function') {
    reject('full Finalizer proof requires a branch-protection client');
  }
  return evaluateAutonomyFinalizerGates({
    client,
    protectionClient,
    trigger,
    trust,
    config, writerTrust,
    proofLevel: 'full',
    checkAttempts,
    sleepImpl,
  });
}

function exactOpenPull(snapshot, expected, pullRequestId) {
  try {
    if (normalizePullLifecycleEvents(snapshot?.lifecycle).length !== 0) return false;
  } catch {
    return false;
  }
  return snapshot?.id === pullRequestId && snapshot?.number === expected.pull_number && snapshot?.state === 'OPEN' &&
    snapshot?.merged === false && snapshot?.mergedAt === null && snapshot?.mergedBy === null && snapshot?.mergeCommit === null &&
    snapshot?.headRefName === expected.branch_name && snapshot?.headRefOid === expected.head_sha &&
    snapshot?.headRepository === expected.repository && snapshot?.baseRefName === 'main' &&
    snapshot?.baseRefOid === expected.base_sha &&
    snapshot?.autoMergeRequest === null;
}

async function restoreDraft({ readClient, writerClient, pullRequestId, pullNumber, expected, observed }) {
  if (!exactOpenPull(observed, expected, pullRequestId)) reject('pull request state is ambiguous; Draft rollback was not attempted');
  let latest;
  try {
    latest = await readClient.getPullGovernance(pullNumber);
  } catch (error) {
    reject(`Draft rollback precondition read failed: ${error instanceof Error ? error.message : 'read failed'}`);
  }
  if (!exactOpenPull(latest, expected, pullRequestId)) {
    reject('pull request state drifted before Draft rollback; mutation was not attempted');
  }
  if (latest.isDraft === true) return;
  if (latest.isDraft !== false) reject('pull request draft state is ambiguous; Draft rollback was not attempted');
  let mutationError = null;
  try {
    const restored = await writerClient.convertPullToDraft(pullRequestId);
    if (restored?.id !== pullRequestId || restored?.isDraft !== true || restored?.headRefOid !== expected.head_sha) {
      mutationError = new AutonomyFinalizerError('GitHub returned an invalid Draft restoration result');
    }
  } catch (error) {
    mutationError = error;
  }
  let confirmed;
  try { confirmed = await readClient.getPullGovernance(pullNumber); } catch (error) {
    throw mutationError ?? error;
  }
  if (exactOpenPull(confirmed, expected, pullRequestId) && confirmed.isDraft === true) return;
  throw mutationError ?? new AutonomyFinalizerError('Draft restoration could not be independently confirmed');
}

async function rollbackAndRethrow(error, context) {
  try {
    await restoreDraft(context);
  } catch (rollbackError) {
    const cause = error instanceof Error ? error.message : 'unknown finalization failure';
    const rollback = rollbackError instanceof Error ? rollbackError.message : 'unknown rollback failure';
    reject(`${cause}; Draft rollback failed: ${rollback}`);
  }
  throw error;
}

function exactMergedOutcome(snapshot, expected, pullRequestId) {
  let lifecycle;
  try { lifecycle = normalizePullLifecycleEvents(snapshot?.lifecycle); } catch { return false; }
  // A successful GitHub merge records one terminal `closed` timeline event. Any
  // prior close/reopen history adds further lifecycle evidence and is ambiguous.
  if (lifecycle.length !== 1 || lifecycle[0].event !== 'closed') return false;
  const expectedGraphQlLogin = expected.writer_graphql_login ?? expected.writer_login.replace(/\[bot\]$/, '');
  const merger = snapshot?.mergedBy;
  const commit = snapshot?.mergeCommit;
  return snapshot?.id === pullRequestId && snapshot?.number === expected.pull_number && snapshot?.state === 'MERGED' &&
    snapshot?.merged === true && snapshot?.isDraft === false && typeof snapshot?.mergedAt === 'string' &&
    snapshot?.headRefName === expected.branch_name && snapshot?.headRefOid === expected.head_sha &&
    snapshot?.headRepository === expected.repository && snapshot?.baseRefName === 'main' && snapshot?.autoMergeRequest === null &&
    snapshot?.authorType === 'Bot' && snapshot?.author === expectedGraphQlLogin &&
    snapshot?.authorId === expected.writer_bot.node_id && snapshot?.authorDatabaseId === expected.writer_bot.database_id &&
    merger?.type === 'Bot' && merger?.login === expectedGraphQlLogin && merger?.id === expected.writer_bot.node_id &&
    merger?.databaseId === expected.writer_bot.database_id && commit?.parentCount === 1 && commit?.parents?.length === 1 &&
    commit.parents[0]?.oid === expected.base_sha && commit?.oid !== expected.base_sha && commit?.oid !== expected.head_sha;
}

async function readOutcomeOrReject({ readClient, pullNumber, mutationError }) {
  try {
    return await readClient.getPullGovernance(pullNumber);
  } catch (readError) {
    const mutation = mutationError instanceof Error ? mutationError.message : 'merge mutation response was inconclusive';
    const read = readError instanceof Error ? readError.message : 'independent merge outcome read failed';
    reject(`${mutation}; independent merge outcome is ambiguous: ${read}`);
  }
}

export async function finalizeAutonomyPull({
  readClient, writerClient, protectionClient = writerClient, trigger, trust, config,
  writerTrust = config.writer_trust, responseLossCanaryBinding, sleepImpl,
}) {
  const configuredWriter = object(writerTrust, 'Writer trust configuration');
  const writerAppSlug = required(configuredWriter.app_slug, 'Writer trusted App slug');
  if (`${writerAppSlug}[bot]` !== config.writer_login) reject('Writer trusted App slug does not match policy');
  const writerAppId = positiveInteger(configuredWriter.app_id, 'Writer trusted App id');
  if (positiveInteger(configuredWriter.proof_app_id, 'Writer proof App id') !== writerAppId ||
      required(configuredWriter.proof_app_slug, 'Writer proof App slug') !== writerAppSlug) {
    reject('Writer App JWT proof does not match the trusted App identity');
  }
  const repositoryOwner = trust.repository.split('/')[0];
  const proofOwnerLogin = required(configuredWriter.proof_app_owner_login, 'Writer proof App owner login');
  const proofOwnerType = required(configuredWriter.proof_app_owner_type, 'Writer proof App owner type');
  required(configuredWriter.proof_app_node_id, 'Writer proof App node id');
  positiveInteger(configuredWriter.proof_app_owner_database_id, 'Writer proof App owner database id');
  if (proofOwnerLogin !== repositoryOwner || !['User', 'Organization'].includes(proofOwnerType)) {
    reject('Writer App JWT proof does not match the repository owner');
  }
  const writerInstallationId = positiveInteger(configuredWriter.installation_id, 'Writer trusted installation id');
  if (positiveInteger(configuredWriter.proof_installation_id, 'Writer proof installation id') !== writerInstallationId) {
    reject('Writer App JWT proof does not match the trusted installation');
  }
  if (required(configuredWriter.proof_installation_account_login, 'Writer proof installation account login') !== proofOwnerLogin ||
      required(configuredWriter.proof_installation_account_type, 'Writer proof installation account type') !== proofOwnerType ||
      configuredWriter.proof_repository_selection !== 'selected') {
    reject('Writer App JWT proof does not match the repository installation');
  }
  if (positiveInteger(configuredWriter.token_installation_id, 'Writer token installation id') !== writerInstallationId) {
    reject('Writer token installation does not match the configured installation');
  }
  if (required(configuredWriter.token_app_slug, 'Writer token App slug') !== writerAppSlug) {
    reject('Writer token App does not match the configured App');
  }
  const appPermissions = validateWriterPermissions(
    configuredWriter.proof_app_permissions,
    'Writer proof App permissions',
  );
  const installationPermissions = validateWriterPermissions(
    configuredWriter.proof_installation_permissions,
    'Writer proof installation permissions',
  );
  if (JSON.stringify(appPermissions) !== JSON.stringify(installationPermissions)) {
    reject('Writer App and installation permissions do not match');
  }
  const writerIdentity = await proveWriterIdentity(writerClient, Object.freeze({
    app_id: writerAppId,
    installation_id: writerInstallationId,
    app_slug: writerAppSlug,
    writer_login: config.writer_login,
    graphql_login: config.writer_login.replace(/\[bot\]$/, ''),
    owner: repositoryOwner,
    repository: trust.repository,
    repository_id: trust.repository_id,
  }));
  const initial = await evaluateAutonomyFinalizer({
    client: readClient, protectionClient, trigger, trust, config, writerTrust: configuredWriter, sleepImpl,
  });
  if (!initial.eligible) return Object.freeze({ action: 'skipped', ...initial });
  const expected = Object.freeze({
    ...initial.bound,
    ...managedIssueBinding(initial.policy, config),
    repository: trust.repository,
    repository_id: trust.repository_id,
    base_sha: trust.policy_sha,
    writer_login: config.writer_login,
    writer_graphql_login: config.writer_login.replace(/\[bot\]$/, ''),
    writer_bot: writerIdentity.bot,
  });
  const pullRequestId = initial.governance.id;
  const pullNumber = initial.bound.pull_number;
  validateGovernance(initial.governance, expected);
  if (initial.proof_level !== 'full') reject('response-loss canary requires a full eligibility proof');
  const initialWriterGovernance = await proveWriterGovernance(writerClient, {
    trust,
    writerTrust: configuredWriter,
    classicProtection: initial.protection,
  });
  const responseLossCanary = validateResponseLossCanaryBinding(responseLossCanaryBinding, expected);
  let transitionedToReady = false;

  if (initial.governance.isDraft) {
    const beforeReady = await evaluateAutonomyFinalizer({
      client: readClient, protectionClient, trigger, trust, config, writerTrust: configuredWriter, checkAttempts: 1, sleepImpl,
    });
    if (!beforeReady.eligible) reject(`pre-ready verification failed: ${beforeReady.reason}`);
    if (beforeReady.proof_level !== 'full' || beforeReady.bound.pull_number !== expected.pull_number ||
        beforeReady.bound.head_sha !== expected.head_sha) {
      reject('pre-ready full proof drifted from the exact pull request');
    }
    validateGovernance(beforeReady.governance, expected);
    if (beforeReady.governance.id !== pullRequestId || beforeReady.governance.isDraft !== true ||
        JSON.stringify(beforeReady.governance) !== JSON.stringify(initial.governance)) {
      reject('pull request drifted before the ready-for-review transition');
    }
    let readyError = null;
    let readyMutationConfirmed = false;
    try {
      const ready = await writerClient.markPullReady(pullRequestId);
      if (ready?.id === pullRequestId && ready?.isDraft === false && ready?.headRefOid === expected.head_sha) {
        readyMutationConfirmed = true;
      } else {
        readyError = new AutonomyFinalizerError('GitHub returned an invalid ready-for-review transition result');
      }
    } catch (error) {
      readyError = error;
    }
    let observedReady;
    try { observedReady = await readClient.getPullGovernance(pullNumber); } catch (error) {
      const mutation = readyError instanceof Error ? readyError.message : 'ready-for-review mutation response was inconclusive';
      reject(`${mutation}; ready-for-review state is ambiguous: ${error instanceof Error ? error.message : 'read failed'}`);
    }
    if (!exactOpenPull(observedReady, expected, pullRequestId) || observedReady.isDraft !== false) {
      if (exactOpenPull(observedReady, expected, pullRequestId) && observedReady.isDraft === true) {
        throw readyError ?? new AutonomyFinalizerError('GitHub did not persist the ready-for-review transition');
      }
      reject('ready-for-review state is ambiguous; Draft rollback was not attempted');
    }
    transitionedToReady = readyMutationConfirmed;
  }

  let beforeMerge;
  try {
    beforeMerge = await evaluateAutonomyFinalizer({
      client: readClient, protectionClient, trigger, trust, config, writerTrust: configuredWriter, checkAttempts: 1, sleepImpl,
    });
  } catch (error) {
    if (transitionedToReady) {
      let observed;
      try { observed = await readClient.getPullGovernance(pullNumber); } catch { throw error; }
      if (exactOpenPull(observed, expected, pullRequestId)) {
        await rollbackAndRethrow(error, { readClient, writerClient, pullRequestId, pullNumber, expected, observed });
      }
    }
    throw error;
  }
  if (!beforeMerge.eligible) {
    const error = new AutonomyFinalizerError(`pre-merge verification failed: ${beforeMerge.reason}`);
    let observed = beforeMerge.governance;
    if (transitionedToReady && !observed) {
      try { observed = await readClient.getPullGovernance(pullNumber); } catch { throw error; }
    }
    if (transitionedToReady && exactOpenPull(observed, expected, pullRequestId) && observed.isDraft === false) {
      await rollbackAndRethrow(error, {
        readClient, writerClient, pullRequestId, pullNumber, expected, observed,
      });
    }
    throw error;
  }
  if (beforeMerge.proof_level !== 'full' || beforeMerge.bound.pull_number !== expected.pull_number ||
      beforeMerge.bound.head_sha !== expected.head_sha || beforeMerge.governance.id !== pullRequestId) {
    reject('pre-merge full proof drifted from the exact pull request');
  }
  try {
    validateGovernance(beforeMerge.governance, expected, { requireReady: true });
  } catch (error) {
    if (transitionedToReady && exactOpenPull(beforeMerge.governance, expected, pullRequestId)) {
      await rollbackAndRethrow(error, {
        readClient, writerClient, pullRequestId, pullNumber, expected, observed: beforeMerge.governance,
      });
    }
    throw error;
  }

  try {
    const immediatelyBeforeMerge = await readClient.getPullGovernance(pullNumber);
    validateGovernance(immediatelyBeforeMerge, expected, { requireReady: true });
    if (JSON.stringify(immediatelyBeforeMerge) !== JSON.stringify(beforeMerge.governance)) {
      reject('pull request governance drifted immediately before merge mutation');
    }
    const immediateWriterGovernance = await proveWriterGovernance(writerClient, {
      trust,
      writerTrust: configuredWriter,
      classicProtection: beforeMerge.protection,
    });
    if (JSON.stringify(immediateWriterGovernance.snapshot) !== JSON.stringify(initialWriterGovernance.snapshot)) {
      reject('Writer governance drifted before merge mutation');
    }
  } catch (error) {
    if (transitionedToReady) {
      let observed;
      try { observed = await readClient.getPullGovernance(pullNumber); } catch { throw error; }
      if (exactOpenPull(observed, expected, pullRequestId)) {
        await rollbackAndRethrow(error, {
          readClient, writerClient, pullRequestId, pullNumber, expected, observed,
        });
      }
    }
    throw error;
  }

  let mutationError = null;
  try {
    // GitHub can CAS only expectedHeadOid here. Governance metadata can still
    // change after the final complete read, so that platform TOCTOU remains.
    await writerClient.mergePullRequest(pullRequestId, expected.head_sha);
    if (responseLossCanary) {
      console.log(`AERIS_FINALIZER_CANARY=${RESPONSE_LOSS_CANARY_MARKER}`);
      mutationError = new AutonomyFinalizerError('exact-bound live canary discarded the merge mutation response');
    }
  } catch (error) { mutationError = error; }
  const outcome = await readOutcomeOrReject({ readClient, pullNumber, mutationError });
  if (exactMergedOutcome(outcome, expected, pullRequestId)) {
    return Object.freeze({
      action: 'merged', pull_number: pullNumber, head_sha: expected.head_sha,
      merge_commit_sha: outcome.mergeCommit.oid,
      ...(responseLossCanary ? { canary_marker: RESPONSE_LOSS_CANARY_MARKER } : {}),
    });
  }
  if (exactOpenPull(outcome, expected, pullRequestId)) {
    const error = mutationError ?? new AutonomyFinalizerError('GitHub did not persist the exact squash merge');
    if (!transitionedToReady) throw error;
    await rollbackAndRethrow(error, {
      readClient, writerClient, pullRequestId, pullNumber, expected, observed: outcome,
    });
  }
  reject('independent merge outcome is ambiguous; Draft rollback was not attempted');
}

export async function runAutonomyFinalizer(environment = process.env, dependencies = {}) {
  const config = policyConfigFromEnvironment(environment);
  const repository = required(environment.GITHUB_REPOSITORY, 'GITHUB_REPOSITORY', REPOSITORY);
  if (config.repository !== repository) reject('finalizer repository configuration drifted');
  const triggerSource = environment.AERIS_FINALIZER_TRIGGER_SOURCE ?? 'required_check';
  if (!['required_check', 'publisher'].includes(triggerSource)) {
    reject('AERIS_FINALIZER_TRIGGER_SOURCE is invalid');
  }
  const trigger = Object.freeze({
    source: triggerSource,
    run_id: positiveInteger(environment.AERIS_TRIGGER_RUN_ID, 'AERIS_TRIGGER_RUN_ID'),
    run_attempt: positiveInteger(environment.AERIS_TRIGGER_RUN_ATTEMPT, 'AERIS_TRIGGER_RUN_ATTEMPT'),
    ...(triggerSource === 'publisher'
      ? { publisher_target_path: required(environment.AERIS_PUBLISHER_TARGET_PATH, 'AERIS_PUBLISHER_TARGET_PATH') }
      : {}),
  });
  const trust = Object.freeze({
    repository,
    repository_id: positiveInteger(environment.GITHUB_REPOSITORY_ID, 'GITHUB_REPOSITORY_ID'),
    default_branch: required(environment.AERIS_DEFAULT_BRANCH, 'AERIS_DEFAULT_BRANCH'),
    policy_ref: required(environment.AERIS_POLICY_REF, 'AERIS_POLICY_REF'),
    policy_sha: required(environment.AERIS_POLICY_SHA, 'AERIS_POLICY_SHA', SHA),
  });
  const readClient = dependencies.readClient ?? new AutonomyFinalizerGitHubClient({
    token: required(environment.GITHUB_TOKEN, 'GITHUB_TOKEN'), repository, apiUrl: environment.GITHUB_API_URL,
  });
  const proofLevel = required(environment.AERIS_FINALIZER_PROOF_LEVEL, 'AERIS_FINALIZER_PROOF_LEVEL');
  if (!['preliminary', 'full'].includes(proofLevel)) reject('AERIS_FINALIZER_PROOF_LEVEL is invalid');
  const mutate = environment.AERIS_FINALIZER_MUTATE === 'true';
  if (mutate !== (proofLevel === 'full')) {
    reject('Finalizer mutation mode requires full proof and preliminary proof must be read-only');
  }
  const writerAttestationTrust = Object.freeze({
    app_id: environment.AERIS_WRITER_APP_ID,
    app_slug: environment.AERIS_WRITER_APP_SLUG,
  });
  const writerTrust = mutate
    ? Object.freeze({
        ...writerAttestationTrust,
        proof_app_id: positiveInteger(environment.AERIS_WRITER_PROOF_APP_ID, 'AERIS_WRITER_PROOF_APP_ID'),
        proof_app_slug: required(environment.AERIS_WRITER_PROOF_APP_SLUG, 'AERIS_WRITER_PROOF_APP_SLUG'),
        proof_app_node_id: required(environment.AERIS_WRITER_PROOF_APP_NODE_ID, 'AERIS_WRITER_PROOF_APP_NODE_ID'),
        proof_app_owner_login: required(environment.AERIS_WRITER_PROOF_APP_OWNER_LOGIN, 'AERIS_WRITER_PROOF_APP_OWNER_LOGIN'),
        proof_app_owner_database_id: positiveInteger(
          environment.AERIS_WRITER_PROOF_APP_OWNER_DATABASE_ID,
          'AERIS_WRITER_PROOF_APP_OWNER_DATABASE_ID',
        ),
        proof_app_owner_type: required(environment.AERIS_WRITER_PROOF_APP_OWNER_TYPE, 'AERIS_WRITER_PROOF_APP_OWNER_TYPE'),
        proof_app_permissions: environment.AERIS_WRITER_PROOF_APP_PERMISSIONS,
        installation_id: positiveInteger(environment.AERIS_WRITER_INSTALLATION_ID, 'AERIS_WRITER_INSTALLATION_ID'),
        proof_installation_id: positiveInteger(environment.AERIS_WRITER_PROOF_INSTALLATION_ID, 'AERIS_WRITER_PROOF_INSTALLATION_ID'),
        proof_installation_account_login: required(environment.AERIS_WRITER_PROOF_INSTALLATION_ACCOUNT_LOGIN, 'AERIS_WRITER_PROOF_INSTALLATION_ACCOUNT_LOGIN'),
        proof_installation_account_type: required(environment.AERIS_WRITER_PROOF_INSTALLATION_ACCOUNT_TYPE, 'AERIS_WRITER_PROOF_INSTALLATION_ACCOUNT_TYPE'),
        proof_installation_permissions: environment.AERIS_WRITER_PROOF_INSTALLATION_PERMISSIONS,
        proof_repository_selection: required(environment.AERIS_WRITER_PROOF_REPOSITORY_SELECTION, 'AERIS_WRITER_PROOF_REPOSITORY_SELECTION'),
        token_installation_id: positiveInteger(environment.AERIS_WRITER_TOKEN_INSTALLATION_ID, 'AERIS_WRITER_TOKEN_INSTALLATION_ID'),
        token_app_slug: required(environment.AERIS_WRITER_TOKEN_APP_SLUG, 'AERIS_WRITER_TOKEN_APP_SLUG'),
        governance_fence_ruleset_id: positiveInteger(
          environment.AERIS_WRITER_GOVERNANCE_FENCE_RULESET_ID,
          'AERIS_WRITER_GOVERNANCE_FENCE_RULESET_ID',
        ),
        governance_fence_updated_at: timestamp(
          environment.AERIS_WRITER_GOVERNANCE_FENCE_UPDATED_AT,
          'AERIS_WRITER_GOVERNANCE_FENCE_UPDATED_AT',
        ),
      })
    : writerAttestationTrust;
  const writerClient = mutate
    ? dependencies.writerClient ?? new AutonomyFinalizerGitHubClient({
        token: required(environment.AERIS_WRITER_TOKEN, 'AERIS_WRITER_TOKEN'),
        repository,
        apiUrl: environment.GITHUB_API_URL,
      })
    : null;
  const result = mutate
    ? await finalizeAutonomyPull({
        readClient,
        writerClient,
        protectionClient: dependencies.protectionClient ?? writerClient,
        writerTrust,
        trigger, trust, config,
        responseLossCanaryBinding: environment.AERIS_FINALIZER_RESPONSE_LOSS_CANARY,
        sleepImpl: dependencies.sleepImpl,
      })
    : await evaluateAutonomyFinalizerPreliminary({
        client: readClient, trigger, trust, config, writerTrust, sleepImpl: dependencies.sleepImpl,
      });
  if (environment.GITHUB_OUTPUT) {
    const eligible = mutate ? result.action !== 'skipped' : result.eligible;
    fs.appendFileSync(environment.GITHUB_OUTPUT, [
      `eligible=${eligible ? 'true' : 'false'}`,
      `reason=${result.reason ?? ''}`,
      `pull_number=${result.pull_number ?? result.bound?.pull_number ?? ''}`,
      `head_sha=${result.head_sha ?? result.bound?.head_sha ?? ''}`,
      `action=${result.action ?? (result.proof_level === 'preliminary' ? 'candidate' : 'verified')}`,
      `proof_level=${result.proof_level ?? proofLevel}`,
      `canary_marker=${result.canary_marker ?? ''}`,
      '',
    ].join('\n'));
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  await runAutonomyFinalizer();
}
