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
