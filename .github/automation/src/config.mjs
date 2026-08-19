import fs from 'node:fs';
import path from 'node:path';
import yaml from 'js-yaml';

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
const POLICY_PERMISSION_GRANTS = {
  metadata: 'read',
  contents: 'read',
  pull_requests: 'read',
  checks: 'write',
};
const POLICY_DENIED_PERMISSIONS = [
  'actions', 'statuses', 'issues', 'workflows', 'administration', 'deployments', 'environments',
  'secrets', 'members', 'packages',
];
const POLICY_ALLOWED_OPERATIONS = ['read_policy_inputs', 'create_or_update_policy_check'];
const POLICY_DENIED_OPERATIONS = [
  'contents_write', 'review', 'approve', 'merge', 'enable_auto_merge', 'mark_ready', 'close_pr',
  'delete_branch',
];
const POLICY_TRIGGERS = [
  'required_checks_completed', 'policy_signal_completed', 'default_branch_changed',
  'scheduled_reconciliation', 'manual_dispatch',
];
const POLICY_HUMAN_REVIEW_PATHS = [
  '.github/**', 'CODEOWNERS', 'apps/**', 'crates/**', 'frontend/src/**', 'Cargo.toml',
  'Cargo.lock', '**/Cargo.toml', '**/Cargo.lock', 'frontend/package.json',
  'frontend/package-lock.json', 'Dockerfile*', 'docker-compose*.yml', 'deploy.sh', 'release/**',
  'scripts/release/**', '**/auth/**', '**/database/**', '**/db/**', '**/migrations/**',
  '**/security/**',
];
const POLICY_REQUIRED_CHECKS = ['Rust CI / check', 'Frontend CI / check'];
const POLICY_REQUIRED_CHECK_SOURCES = [
  { context: 'Rust CI / check', app_id: 15368, app_slug: 'github-actions' },
  { context: 'Frontend CI / check', app_id: 15368, app_slug: 'github-actions' },
];
const POLICY_REGISTRY_KEYS = [
  'enabled', 'enabled_variable', 'phase', 'mode', 'identity', 'app_id_variable',
  'app_slug_variable', 'private_key_secret', 'environment', 'credentials', 'permissions',
  'deterministic_client_mitigations', 'model_variable', 'triggers', 'tools', 'effects', 'handoff_to',
];
const POLICY_GATE_KEYS = [
  'enabled', 'enabled_variable', 'identity', 'app_id_variable', 'app_slug_variable',
  'private_key_secret', 'environment', 'credentials', 'permissions',
  'deterministic_client_mitigations', 'release_secret_access', 'pull_request_target_checkout',
  'check_name', 'mode', 'allowed_modes', 'human_enable_label', 'require_exact_head_sha',
  'require_base_up_to_date', 'require_conversation_resolution', 'required_checks',
  'required_check_sources', 'always_require_human_review', 'allowlist_paths',
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
  return Array.isArray(actual) && actual.length === expected.length &&
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

function validatePolicyFoundation(policyAgent, gate) {
  requireCondition(policyAgent && typeof policyAgent === 'object', 'policy Agent foundation is missing');
  requireCondition(gate && typeof gate === 'object', 'policy gate foundation is missing');
  requireCondition(sameStringSet(Object.keys(policyAgent), POLICY_REGISTRY_KEYS), 'policy Agent registry contains unapproved fields');
  requireCondition(sameStringSet(Object.keys(gate), POLICY_GATE_KEYS), 'policy gate contains unapproved fields');
  requireCondition(policyAgent.enabled === true && gate.enabled === true, 'policy Agent and gate must be enabled in human mode');
  requireCondition(policyAgent.phase === 4 && policyAgent.mode === 'deterministic' && policyAgent.model_variable === null, 'policy Agent must remain deterministic Phase 4');
  for (const field of ['enabled_variable', 'identity', 'app_id_variable', 'app_slug_variable', 'private_key_secret', 'environment']) {
    requireCondition(policyAgent[field] === gate[field], `policy Agent registry and gate ${field} differ`);
  }
  requireCondition(
    policyAgent.enabled_variable === 'AERIS_POLICY_ENABLED' && policyAgent.identity === 'github_app' &&
      policyAgent.app_id_variable === 'AERIS_POLICY_APP_ID' && policyAgent.app_slug_variable === 'AERIS_POLICY_APP_SLUG' &&
      policyAgent.private_key_secret === 'AERIS_POLICY_PRIVATE_KEY' && policyAgent.environment === 'policy',
    'policy Agent identity configuration changed',
  );
  for (const source of [policyAgent, gate]) {
    requireCondition(sameStringArray(source.credentials?.allowed_jobs, ['publish']) && source.credentials.github_token_write === false, 'policy credentials must be limited to publish without GITHUB_TOKEN write access');
    requireCondition(Object.entries(POLICY_PERMISSION_GRANTS).every(([name, level]) => source.permissions?.[name] === level) && sameStringSet(Object.keys(source.permissions ?? {}), [...Object.keys(POLICY_PERMISSION_GRANTS), 'denied']) && sameStringArray(source.permissions.denied, POLICY_DENIED_PERMISSIONS), 'policy App permissions exceed the approved minimum');
    requireCondition(sameStringArray(source.deterministic_client_mitigations?.allowed_operations, POLICY_ALLOWED_OPERATIONS) && sameStringArray(source.deterministic_client_mitigations?.denied_operations, POLICY_DENIED_OPERATIONS), 'policy deterministic client operations exceed the approved boundary');
  }
  requireCondition(sameStringArray(policyAgent.triggers, POLICY_TRIGGERS) && sameStringArray(policyAgent.tools, ['repository_read', 'pull_request_metadata_read', 'checks_read', 'checks_write']) && sameStringArray(policyAgent.effects, ['publish_policy_check']) && sameStringArray(policyAgent.handoff_to, ['merger']), 'policy Agent capabilities exceed the approved Phase 4 boundary');
  requireCondition(gate.release_secret_access === false && gate.pull_request_target_checkout === false, 'policy gate cannot access release secrets or checkout pull request code');
  requireCondition(gate.check_name === 'Automation Policy / gate' && gate.mode === 'human' && sameStringArray(gate.allowed_modes, ['shadow', 'human', 'label', 'allowlist']) && gate.require_exact_head_sha === true && gate.require_base_up_to_date === true && gate.require_conversation_resolution === true, 'policy gate core constraints changed');
  requireCondition(sameStringArray(gate.required_checks, POLICY_REQUIRED_CHECKS) && JSON.stringify(gate.required_check_sources) === JSON.stringify(POLICY_REQUIRED_CHECK_SOURCES), 'policy gate required check identities changed');
  requireCondition(!gate.required_checks.includes(gate.check_name), 'policy gate cannot require its own check');
  requireCondition(sameStringArray(gate.always_require_human_review, POLICY_HUMAN_REVIEW_PATHS), 'policy gate human-review paths changed');
  requireCondition(Array.isArray(gate.allowlist_paths) && gate.allowlist_paths.length === 0, 'human-mode automatic merge allowlist must remain empty');
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
    policy.kill_switch?.repository_variable === agents.runtime?.enabled_variable,
    'registry and policy kill switch variables differ',
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
  requireCondition(policy.writer?.enabled === false, 'writer must remain disabled during Phase 2');
  validatePolicyFoundation(agents.agents.policy, policy.policy_gate);
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
