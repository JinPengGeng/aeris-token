import fs from 'node:fs';
import path from 'node:path';
import yaml from 'js-yaml';

import { WRITER_FOUNDATION_LIMITS } from './writer-phase-contract.mjs';
import { canonicalWriterCommand, WRITER_COMMANDS } from './writer-guard.mjs';

export class ContractError extends Error {
  constructor(message) {
    super(message);
    this.name = 'ContractError';
  }
}

const MAXIMUM_MODEL_OUTPUT_TOKENS = 16_384;
const REVIEWER_LIMIT_KEYS = new Set([
  'maximum_input_characters',
  'maximum_patch_characters_per_file',
  'request_timeout_seconds',
]);
const MINIMUM_REVIEWER_INPUT_CHARACTERS = 24_000;
const MAXIMUM_REVIEWER_INPUT_CHARACTERS = 262_144;
const MINIMUM_REVIEWER_PATCH_CHARACTERS = 1;
const MAXIMUM_REVIEWER_PATCH_CHARACTERS = 65_536;
const MINIMUM_REVIEWER_REQUEST_TIMEOUT_SECONDS = 120;
const MAXIMUM_REVIEWER_REQUEST_TIMEOUT_SECONDS = 600;
const GLOBAL_ENABLED_VARIABLE = 'AERIS_AGENTS_ENABLED';
const GLOBAL_ENABLED_VALUES = Object.freeze(['1', 'true']);
const WRITER_ENABLED_VARIABLE = 'AERIS_WRITER_ENABLED';
const WRITER_APP_ID_VARIABLE = 'AERIS_WRITER_APP_ID';
const WRITER_APP_SLUG_VARIABLE = 'AERIS_WRITER_APP_SLUG';
const WRITER_PRIVATE_KEY_SECRET = 'AERIS_WRITER_PRIVATE_KEY';
const WRITER_ENVIRONMENT = 'writer';
const WRITER_BRANCH_PREFIX = 'agent/';
const WRITER_TIMEOUTS = Object.freeze({
  github_api_total_seconds: 30,
  github_response_headers_seconds: 10,
  github_response_body_seconds: 15,
  publish_job_minutes: 15,
});

const WRITER_PERMISSION_GRANTS = {
  metadata: 'read',
  contents: 'write',
  pull_requests: 'write',
};
const WRITER_DENIED_PERMISSIONS = [
  'checks', 'actions', 'workflows', 'administration', 'deployments', 'environments', 'secrets',
  'members', 'packages', 'issues',
];
const WRITER_FORBIDDEN_PATHS = [
  '.github/**', '**/CODEOWNERS', '.gitmodules', '**/.git', '**/.git/**',
];
const WRITER_CAPABILITY_RESIDUALS = {
  pull_requests_write_can_review_or_merge: true,
  contents_write_not_branch_scoped: true,
  app_has_branch_protection_bypass: false,
};
const WRITER_IDENTITY_VERIFICATION = 'app_jwt_mints_installation_token_then_verify';
const WRITER_AMBIGUOUS_CREATE_RECOVERY = 'unique_attempt_marker_then_verified_close';
const WRITER_ALLOWED_OPERATIONS = [
  'create_or_update_agent_ref',
  'create_or_update_draft_pull_request',
  'compensate_close_just_created_verified_draft_pull',
];
const WRITER_DENIED_OPERATIONS = [
  'review', 'approve', 'merge', 'enable_auto_merge', 'mark_ready', 'close_pr', 'delete_branch',
];
const WRITER_REQUIRED_ACTOR_PERMISSIONS = ['admin', 'maintain', 'write'];
const WRITER_REQUIRED_COMMANDS = WRITER_COMMANDS;
const WRITER_REGISTRY_KEYS = [
  'enabled', 'enabled_variable', 'phase', 'mode', 'identity', 'app_id_variable',
  'app_slug_variable', 'private_key_secret', 'environment', 'timeouts', 'credentials', 'permissions', 'capability_residuals',
  'deterministic_client_mitigations', 'limits', 'model_variable', 'fallback_model_variable',
  'triggers', 'required_issue_labels', 'required_actor_permissions', 'required_commands',
  'allowed_branch_prefixes', 'tools', 'effects', 'denied_paths', 'handoff_to',
];
const WRITER_POLICY_KEYS = [
  'enabled', 'enabled_variable', 'branch_prefix', 'draft_pull_requests_only',
  'maximum_open_pull_requests_per_issue', 'identity', 'app_id_variable', 'app_slug_variable',
  'private_key_secret', 'environment', 'timeouts', 'credentials', 'permissions', 'capability_residuals',
  'deterministic_client_mitigations', 'limits', 'forbidden_paths', 'release_secret_access',
  'pull_request_target_checkout',
];
function requireCondition(condition, message) {
  if (!condition) throw new ContractError(message);
}

function loadYaml(repoRoot, relativePath) {
  const filePath = path.join(repoRoot, relativePath);
  return yaml.load(fs.readFileSync(filePath, 'utf8'));
}

function validateAgent(name, agent, allowedModelVariables, knownAgents) {
  requireCondition(agent && typeof agent === 'object', `${name} definition is invalid`);
  requireCondition(typeof agent.enabled === 'boolean', `${name}.enabled must be boolean`);
  requireCondition(Array.isArray(agent.handoff_to), `${name}.handoff_to must be an array`);
  for (const target of agent.handoff_to) {
    requireCondition(knownAgents.has(target), `${name} hands off to unknown agent ${target}`);
  }
  if (agent.model_variable !== null) {
    requireCondition(
      allowedModelVariables.has(agent.model_variable),
      `${name} model variable is outside the allowlist`,
    );
  }
  if (agent.fallback_model_variable !== undefined) {
    requireCondition(
      allowedModelVariables.has(agent.fallback_model_variable),
      `${name} fallback model variable is outside the allowlist`,
    );
  }
}

function validateReviewerLimits(limits) {
  requireCondition(
    limits && typeof limits === 'object' && !Array.isArray(limits),
    'reviewer limits must be an object',
  );
  const keys = Object.keys(limits);
  requireCondition(
    keys.length === REVIEWER_LIMIT_KEYS.size && keys.every((key) => REVIEWER_LIMIT_KEYS.has(key)),
    'reviewer limits must contain exactly the approved keys',
  );
  requireCondition(
    Number.isInteger(limits.maximum_input_characters) &&
      limits.maximum_input_characters >= MINIMUM_REVIEWER_INPUT_CHARACTERS &&
      limits.maximum_input_characters <= MAXIMUM_REVIEWER_INPUT_CHARACTERS,
    `reviewer maximum_input_characters must be an integer between ${MINIMUM_REVIEWER_INPUT_CHARACTERS} and ${MAXIMUM_REVIEWER_INPUT_CHARACTERS}`,
  );
  requireCondition(
    Number.isInteger(limits.maximum_patch_characters_per_file) &&
      limits.maximum_patch_characters_per_file >= MINIMUM_REVIEWER_PATCH_CHARACTERS &&
      limits.maximum_patch_characters_per_file <= MAXIMUM_REVIEWER_PATCH_CHARACTERS &&
      limits.maximum_patch_characters_per_file <= limits.maximum_input_characters,
    `reviewer maximum_patch_characters_per_file must be an integer between ${MINIMUM_REVIEWER_PATCH_CHARACTERS} and ${MAXIMUM_REVIEWER_PATCH_CHARACTERS} and not exceed maximum_input_characters`,
  );
  requireCondition(
    Number.isInteger(limits.request_timeout_seconds) &&
      limits.request_timeout_seconds >= MINIMUM_REVIEWER_REQUEST_TIMEOUT_SECONDS &&
      limits.request_timeout_seconds <= MAXIMUM_REVIEWER_REQUEST_TIMEOUT_SECONDS,
    `reviewer request_timeout_seconds must be an integer between ${MINIMUM_REVIEWER_REQUEST_TIMEOUT_SECONDS} and ${MAXIMUM_REVIEWER_REQUEST_TIMEOUT_SECONDS}`,
  );
}

function sameStringArray(actual, expected) {
  return Array.isArray(actual) &&
    actual.length === expected.length &&
    actual.every((value, index) => value === expected[index]);
}

function sameStringSet(actual, expected) {
  return Array.isArray(actual) && actual.length === expected.length &&
    actual.every((value) => typeof value === 'string') &&
    new Set(actual).size === actual.length && actual.every((value) => expected.includes(value));
}

function sameRecord(actual, expected) {
  return actual && typeof actual === 'object' && !Array.isArray(actual) &&
    sameStringSet(Object.keys(actual), Object.keys(expected)) &&
    Object.entries(expected).every(([key, value]) => actual[key] === value);
}

function sameWriterField(field, actual, expected) {
  if (field === 'permissions') {
    return sameStringSet(Object.keys(actual ?? {}), Object.keys(expected ?? {})) &&
      Object.keys(actual ?? {}).every((key) => JSON.stringify(actual[key]) === JSON.stringify(expected[key]));
  }
  return JSON.stringify(actual) === JSON.stringify(expected);
}

function validateWriterFoundation(writer, counterpart) {
  requireCondition(writer && typeof writer === 'object', 'writer foundation is missing');
  requireCondition(
    sameStringSet(Object.keys(writer), WRITER_REGISTRY_KEYS),
    'writer registry contains unapproved fields',
  );
  requireCondition(writer.enabled === false, 'writer must remain disabled during Phase 3 foundation');
  requireCondition(writer.phase === 3, 'writer phase must remain 3');
  requireCondition(writer.mode === 'draft_pull_request', 'writer must remain Draft PR only');
  requireCondition(
    writer.enabled_variable === WRITER_ENABLED_VARIABLE,
    'writer must use the independent AERIS_WRITER_ENABLED switch',
  );
  requireCondition(writer.identity === 'github_app', 'writer must use a GitHub App identity');
  requireCondition(
    writer.app_id_variable === WRITER_APP_ID_VARIABLE,
    'writer App ID must use AERIS_WRITER_APP_ID',
  );
  requireCondition(
    writer.app_slug_variable === WRITER_APP_SLUG_VARIABLE,
    'writer App slug must use AERIS_WRITER_APP_SLUG',
  );
  requireCondition(
    writer.private_key_secret === WRITER_PRIVATE_KEY_SECRET,
    'writer private key must use AERIS_WRITER_PRIVATE_KEY',
  );
  requireCondition(writer.environment === WRITER_ENVIRONMENT, 'writer must use the writer environment');
  requireCondition(
    sameRecord(writer.timeouts, WRITER_TIMEOUTS),
    'writer API and publish job timeouts changed',
  );
  requireCondition(
    sameStringArray(writer.credentials?.allowed_jobs, ['publish']) &&
      writer.credentials.github_token_write === false,
    'writer credentials must be limited to publish without GITHUB_TOKEN write access',
  );
  requireCondition(
    Object.entries(WRITER_PERMISSION_GRANTS).every(
      ([permission, level]) => writer.permissions?.[permission] === level,
    ) &&
      sameStringSet(Object.keys(writer.permissions ?? {}), [...Object.keys(WRITER_PERMISSION_GRANTS), 'denied']) &&
      sameStringArray(writer.permissions?.denied, WRITER_DENIED_PERMISSIONS),
    'writer App permissions exceed the approved minimum',
  );
  requireCondition(
    sameRecord(writer.capability_residuals, WRITER_CAPABILITY_RESIDUALS),
    'writer GitHub App capability residuals changed',
  );
  requireCondition(
    sameStringSet(Object.keys(writer.deterministic_client_mitigations ?? {}), [
      'identity_verification', 'ambiguous_create_recovery', 'allowed_operations', 'denied_operations',
    ]) &&
      writer.deterministic_client_mitigations.identity_verification === WRITER_IDENTITY_VERIFICATION &&
      writer.deterministic_client_mitigations.ambiguous_create_recovery === WRITER_AMBIGUOUS_CREATE_RECOVERY &&
      sameStringArray(writer.deterministic_client_mitigations.allowed_operations, WRITER_ALLOWED_OPERATIONS) &&
      sameStringArray(writer.deterministic_client_mitigations?.denied_operations, WRITER_DENIED_OPERATIONS),
    'writer deterministic client operations exceed the approved boundary',
  );
  requireCondition(
    sameRecord(writer.limits, WRITER_FOUNDATION_LIMITS),
    'writer limits must match the approved Phase 3 foundation values',
  );
  requireCondition(
    sameStringArray(writer.denied_paths ?? writer.forbidden_paths, WRITER_FORBIDDEN_PATHS),
    'writer forbidden paths must match the protected foundation boundary',
  );
  requireCondition(
    writer.model_variable === 'AERIS_AI_MODEL_WRITER' &&
      writer.fallback_model_variable === 'AERIS_AI_MODEL_FALLBACK' &&
      sameStringArray(writer.triggers, ['maintainer_command_implement', 'maintainer_command_retry_write']) &&
      sameStringArray(writer.required_issue_labels, ['agent-ready']) &&
      sameStringArray(writer.required_actor_permissions, WRITER_REQUIRED_ACTOR_PERMISSIONS) &&
      sameStringArray(writer.required_commands, WRITER_REQUIRED_COMMANDS) &&
      sameStringArray(writer.allowed_branch_prefixes, [WRITER_BRANCH_PREFIX]) &&
      sameStringArray(writer.tools, ['repository_read', 'isolated_shell', 'branch_write', 'draft_pull_request']) &&
      sameStringArray(writer.effects, ['create_or_update_draft_pull_request']) &&
      sameStringArray(writer.handoff_to, ['reviewer', 'tester', 'security']),
    'writer registry capabilities exceed the approved Phase 3 boundary',
  );
  if (counterpart) {
    requireCondition(
      sameStringSet(Object.keys(counterpart), WRITER_POLICY_KEYS),
      'writer policy contains unapproved fields',
    );
    requireCondition(
      counterpart.branch_prefix === WRITER_BRANCH_PREFIX &&
        counterpart.draft_pull_requests_only === true &&
        counterpart.maximum_open_pull_requests_per_issue === 1 &&
        counterpart.release_secret_access === false &&
        counterpart.pull_request_target_checkout === false,
      'writer policy capabilities exceed the approved Phase 3 boundary',
    );
    for (const field of [
      'enabled', 'enabled_variable', 'identity', 'app_id_variable', 'app_slug_variable',
      'private_key_secret', 'environment', 'timeouts', 'credentials', 'permissions',
      'capability_residuals', 'deterministic_client_mitigations', 'limits',
    ]) {
      requireCondition(
        sameWriterField(field, writer[field], counterpart[field]),
        `writer registry and policy ${field} differ`,
      );
    }
    requireCondition(
      JSON.stringify(writer.denied_paths) === JSON.stringify(counterpart.forbidden_paths),
      'writer registry and policy forbidden paths differ',
    );
  }
}

export function validateContracts(agents, policy) {
  requireCondition(agents?.version === 1, 'agent registry version must be 1');
  requireCondition(policy?.version === 1, 'automation policy version must be 1');
  requireCondition(
    agents.trusted_source?.ref === 'refs/heads/main',
    'agent registry must trust refs/heads/main',
  );
  requireCondition(
    policy.trusted_source?.ref === 'refs/heads/main',
    'automation policy must trust refs/heads/main',
  );
  requireCondition(agents.runtime?.default_enabled === false, 'agent runtime must default off');
  requireCondition(policy.kill_switch?.default_enabled === false, 'kill switch must default off');
  requireCondition(
    agents.runtime?.enabled_variable === GLOBAL_ENABLED_VARIABLE &&
      policy.kill_switch?.repository_variable === GLOBAL_ENABLED_VARIABLE,
    'registry and policy must use the fixed AERIS_AGENTS_ENABLED kill switch',
  );
  requireCondition(
    sameStringArray(policy.kill_switch?.enabled_values, GLOBAL_ENABLED_VALUES),
    'kill switch enabled values must be exactly 1 and true',
  );
  requireCondition(
    policy.limits?.maximum_concurrent_runs_per_object ===
      agents.runtime?.limits?.maximum_concurrent_runs_per_object,
    'registry and policy object concurrency limits differ',
  );
  requireCondition(
    Number.isInteger(agents.runtime?.limits?.maximum_output_tokens) &&
      agents.runtime.limits.maximum_output_tokens > 0 &&
      agents.runtime.limits.maximum_output_tokens <= MAXIMUM_MODEL_OUTPUT_TOKENS,
    `maximum_output_tokens must be an integer between 1 and ${MAXIMUM_MODEL_OUTPUT_TOKENS}`,
  );
  validateReviewerLimits(agents.runtime?.reviewer_limits);
  requireCondition(
    agents.runtime.reviewer_limits.request_timeout_seconds >= agents.runtime.api.request_timeout_seconds,
    'reviewer request timeout must not be shorter than the shared request timeout',
  );

  const expectedAgents = new Set([
    'triage',
    'planner',
    'reviewer',
    'writer',
    'tester',
    'security',
    'policy',
    'merger',
  ]);
  const knownAgents = new Set(Object.keys(agents.agents ?? {}));
  requireCondition(
    knownAgents.size === expectedAgents.size && [...expectedAgents].every((name) => knownAgents.has(name)),
    'agent registry does not contain exactly the approved roles',
  );

  const structuredOutput = agents.model_policy?.structured_output;
  requireCondition(
    Array.isArray(structuredOutput?.canary_agents) &&
      structuredOutput.canary_agents.length === 1 &&
      structuredOutput.canary_agents[0] === 'planner',
    'structured output canary must remain limited to planner',
  );
  requireCondition(
    Array.isArray(structuredOutput?.approved_model_ids) &&
      structuredOutput.approved_model_ids.length > 0 &&
      structuredOutput.approved_model_ids.every(
        (id) => typeof id === 'string' && id === id.trim() && id.length > 0 && id.length <= 200,
      ) &&
      new Set(structuredOutput.approved_model_ids).size ===
        structuredOutput.approved_model_ids.length,
    'structured output approved models must be unique bounded IDs',
  );

  const allowedModelVariables = new Set(agents.model_policy?.allowed_model_variables ?? []);
  for (const [name, agent] of Object.entries(agents.agents)) {
    validateAgent(name, agent, allowedModelVariables, knownAgents);
  }
  for (const name of ['tester', 'policy', 'merger']) {
    requireCondition(agents.agents[name].mode === 'deterministic', `${name} must be deterministic`);
    requireCondition(agents.agents[name].model_variable === null, `${name} must not select a model`);
  }
  requireCondition(
    !agents.agents.reviewer.handoff_to.includes('writer'),
    'reviewer must not authorize writer',
  );

  const retryableStatuses = agents.model_policy?.retryable_http_statuses ?? [];
  requireCondition(
    retryableStatuses.length > 0 &&
      retryableStatuses.every((status) => status === 408 || status === 429 || status >= 500),
    'retryable model statuses exceed the approved failure classes',
  );
  requireCondition(
    (agents.model_policy?.retryable_failures ?? []).every((reason) =>
      ['connect_error', 'timeout'].includes(reason),
    ),
    'retryable model failures exceed connect_error and timeout',
  );

  requireCondition(policy.commands?.prefix === '/agent', 'command prefix must remain /agent');
  requireCondition(
    sameStringArray(
      WRITER_REQUIRED_COMMANDS.map((command) => canonicalWriterCommand(command)),
      WRITER_COMMANDS,
    ),
    'writer command names do not map to the guarded command format',
  );
  requireCondition(
    Array.isArray(policy.commands?.accepted_author_associations),
    'trusted author associations are missing',
  );
  requireCondition(
    policy.authorization?.external_issue_analysis_requires_label === 'agent-analyze',
    'external Issue analysis label changed',
  );
  requireCondition(
    policy.authorization?.external_pull_request_analysis_requires_label === 'agent-analyze',
    'external PR analysis label changed',
  );
  requireCondition(policy.prompt_security?.triage_shell_access === false, 'triage shell must remain off');
  requireCondition(policy.prompt_security?.triage_network_access === false, 'model network tools must remain off');
  const writer = agents.agents?.writer;
  requireCondition(writer?.mode === 'draft_pull_request', 'writer must remain Draft PR only');
  requireCondition(
    sameStringArray(writer?.allowed_branch_prefixes, [WRITER_BRANCH_PREFIX]) &&
      policy.writer?.branch_prefix === WRITER_BRANCH_PREFIX,
    'writer branch prefix must remain agent/',
  );
  requireCondition(policy.writer?.draft_pull_requests_only === true, 'writer must create Draft PRs only');
  requireCondition(
    policy.writer?.maximum_open_pull_requests_per_issue === 1,
    'writer must allow exactly one open pull request per issue',
  );
  requireCondition(
    sameStringArray(writer.required_actor_permissions, WRITER_REQUIRED_ACTOR_PERMISSIONS) &&
      sameStringArray(writer.required_commands, WRITER_REQUIRED_COMMANDS) &&
      sameStringArray(writer.required_issue_labels, ['agent-ready']) &&
      sameStringArray(
        policy.authorization?.code_write_requires?.actor_permission,
        WRITER_REQUIRED_ACTOR_PERMISSIONS,
      ) &&
      sameStringArray(policy.authorization?.code_write_requires?.exact_commands, WRITER_REQUIRED_COMMANDS) &&
      sameStringArray(policy.authorization?.code_write_requires?.issue_labels, writer.required_issue_labels) &&
      policy.authorization?.code_write_requires?.author_association === undefined,
    'writer code-write authorization must use exact commands and live actor permissions',
  );
  validateWriterFoundation(writer, policy.writer);
  requireCondition(policy.policy_gate?.enabled === false, 'policy gate must remain disabled during Phase 2');
  requireCondition(
    Array.isArray(policy.policy_gate?.required_checks) &&
      policy.policy_gate.required_checks.length > 0 &&
      policy.policy_gate.required_checks.every(
        (context) => typeof context === 'string' && context.length > 0,
      ) &&
      new Set(policy.policy_gate.required_checks).size === policy.policy_gate.required_checks.length,
    'required checks must be a non-empty list of unique contexts',
  );

  return { agents, policy };
}

export function loadContracts(repoRoot) {
  return validateContracts(
    loadYaml(repoRoot, '.github/agents.yml'),
    loadYaml(repoRoot, '.github/automation-policy.yml'),
  );
}

export function resolveModelCandidates(agentName, agent, environment) {
  const candidates = [];
  const append = (alias, variableName) => {
    if (!variableName) return;
    const id = environment[variableName]?.trim();
    if (!id || candidates.some((candidate) => candidate.id === id)) return;
    candidates.push({ alias, id, variable: variableName });
  };

  append('role', agent.model_variable);
  append('default', 'AERIS_AI_MODEL');
  append('fallback', agent.fallback_model_variable ?? 'AERIS_AI_MODEL_FALLBACK');
  if (candidates.length === 0) {
    throw new ContractError(`no model is configured for ${agentName}`);
  }
  return candidates;
}

export function shouldUseStructuredOutput(agentName, candidates, agents) {
  const policy = agents.model_policy.structured_output;
  if (!policy.canary_agents.includes(agentName)) return false;
  const approvedModelIds = new Set(policy.approved_model_ids);
  if (!candidates.every((candidate) => approvedModelIds.has(candidate.id))) {
    throw Object.assign(
      new ContractError(`structured output model is not approved for ${agentName}`),
      { code: 'structured_output_model_not_approved' },
    );
  }
  return true;
}
