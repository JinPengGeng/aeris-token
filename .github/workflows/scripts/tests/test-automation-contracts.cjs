const fs = require('node:fs');
const path = require('node:path');
const { execFileSync } = require('node:child_process');

const repoRoot = path.resolve(__dirname, '..', '..', '..', '..');
const yamlCandidates = [
  path.join(repoRoot, '.github', 'automation', 'node_modules', 'js-yaml'),
  path.join(repoRoot, 'frontend', 'node_modules', 'js-yaml'),
];
const yamlPath = yamlCandidates.find((candidate) => fs.existsSync(candidate));
if (!yamlPath) throw new Error('js-yaml is not installed in an approved workspace');
const yaml = require(yamlPath);

const read = (relativePath) =>
  fs.readFileSync(path.join(repoRoot, relativePath), 'utf8');
const loadYaml = (relativePath) => yaml.load(read(relativePath));
const assert = (condition, message) => {
  if (!condition) throw new Error(message);
};
const sameMembers = (actual, expected) =>
  actual.length === expected.length && expected.every((value) => actual.includes(value));

const agents = loadYaml('.github/agents.yml');
const automation = loadYaml('.github/automation-policy.yml');
const sync = loadYaml('.github/upstream-sync-policy.yml');
const syncWorkflow = loadYaml('.github/workflows/sync-upstream.yml');
const syncScript = read('.github/workflows/scripts/sync-upstream.sh');
const state = JSON.parse(read('.github/upstream-sync-state.json'));

for (const contract of [agents, automation, sync]) {
  assert(contract.version === 1, 'all automation contracts must use version 1');
}

const expectedAgents = [
  'triage',
  'planner',
  'reviewer',
  'writer',
  'tester',
  'security',
  'policy',
  'merger',
];
assert(
  sameMembers(Object.keys(agents.agents), expectedAgents),
  'agent registry must contain exactly the approved roles',
);
assert(agents.runtime.default_enabled === false, 'agent runtime must default off');
assert(
  Object.entries(agents.agents).every(([name, agent]) =>
    name === 'triage' || name === 'planner' || name === 'reviewer'
      ? agent.enabled === true
      : agent.enabled === false,
  ),
  'only the triage, planner, and reviewer agents may be enabled; every other agent must default off',
);
assert(
  agents.model_policy.retryable_http_statuses.every(
    (status) => status === 408 || status === 429 || status >= 500,
  ),
  'model fallback may only cover 408, 429, and 5xx HTTP failures',
);
assert(
  sameMembers(agents.model_policy.retryable_failures, ['connect_error', 'timeout']),
  'model fallback transport reasons changed',
);

const allowedModelVariables = new Set(agents.model_policy.allowed_model_variables);
for (const [name, agent] of Object.entries(agents.agents)) {
  if (agent.model_variable !== null) {
    assert(
      allowedModelVariables.has(agent.model_variable),
      `${name} uses a model variable outside the registry allowlist`,
    );
  }
  for (const target of agent.handoff_to) {
    assert(expectedAgents.includes(target), `${name} hands off to an unknown agent`);
  }
}
for (const name of ['tester', 'policy', 'merger']) {
  assert(agents.agents[name].mode === 'deterministic', `${name} must be deterministic`);
  assert(agents.agents[name].model_variable === null, `${name} must not select a model`);
}
assert(
  agents.agents.writer.mode === 'draft_pull_request' &&
    JSON.stringify(agents.agents.writer.allowed_branch_prefixes) === JSON.stringify(['agent/']) &&
    JSON.stringify(agents.agents.writer.triggers) ===
      JSON.stringify(['maintainer_command_implement', 'maintainer_command_retry_write']) &&
    JSON.stringify(agents.agents.writer.required_actor_permissions) ===
      JSON.stringify(['admin', 'maintain', 'write']) &&
    JSON.stringify(agents.agents.writer.required_commands) === JSON.stringify(['implement', 'retry-write']),
  'writer boundary must remain Draft PR only on agent/ branches with live actor permissions',
);
assert(
  sameMembers(Object.keys(agents.agents.writer), [
    'enabled', 'enabled_variable', 'phase', 'mode', 'identity', 'app_id_variable',
    'private_key_secret', 'environment', 'credentials', 'permissions', 'capability_residuals',
    'deterministic_client_mitigations', 'limits', 'model_variable', 'fallback_model_variable',
    'triggers', 'required_issue_labels', 'required_actor_permissions', 'required_commands',
    'allowed_branch_prefixes', 'tools', 'effects', 'denied_paths', 'handoff_to',
  ]) &&
    agents.agents.writer.phase === 3 &&
    agents.agents.writer.model_variable === 'AERIS_AI_MODEL_WRITER' &&
    agents.agents.writer.fallback_model_variable === 'AERIS_AI_MODEL_FALLBACK' &&
    JSON.stringify(agents.agents.writer.required_issue_labels) === JSON.stringify(['agent-ready']) &&
    JSON.stringify(agents.agents.writer.tools) ===
      JSON.stringify(['repository_read', 'isolated_shell', 'branch_write', 'draft_pull_request']) &&
    JSON.stringify(agents.agents.writer.effects) === JSON.stringify(['create_or_update_draft_pull_request']) &&
    JSON.stringify(agents.agents.writer.handoff_to) === JSON.stringify(['reviewer', 'tester', 'security']),
  'writer registry capabilities changed',
);
assert(
  !agents.agents.reviewer.handoff_to.includes('writer'),
  'reviewer must not authorize a new writer run',
);

assert(automation.kill_switch.default_enabled === false, 'kill switch must default off');
assert(automation.writer.enabled === false, 'writer policy must default off');
assert(
  JSON.stringify(automation.authorization.code_write_requires) === JSON.stringify({
    actor_permission: ['admin', 'maintain', 'write'],
    exact_commands: ['implement', 'retry-write'],
    issue_labels: ['agent-ready'],
  }),
  'code-write authorization must require live actor permissions and exact commands',
);
assert(
  agents.agents.writer.enabled_variable === 'AERIS_WRITER_ENABLED' &&
    automation.writer.enabled_variable === 'AERIS_WRITER_ENABLED' &&
    agents.agents.writer.identity === 'github_app' &&
    automation.writer.identity === 'github_app' &&
    agents.agents.writer.app_id_variable === 'AERIS_WRITER_APP_ID' &&
    automation.writer.app_id_variable === 'AERIS_WRITER_APP_ID' &&
    agents.agents.writer.private_key_secret === 'AERIS_WRITER_PRIVATE_KEY' &&
    automation.writer.private_key_secret === 'AERIS_WRITER_PRIVATE_KEY' &&
    agents.agents.writer.environment === 'writer' &&
    automation.writer.environment === 'writer',
  'writer must use its independent disabled GitHub App identity and environment',
);
assert(
  automation.writer.draft_pull_requests_only === true &&
    automation.writer.maximum_open_pull_requests_per_issue === 1 &&
    JSON.stringify(agents.agents.writer.credentials) === JSON.stringify({
      allowed_jobs: ['publish'],
      github_token_write: false,
    }) &&
    JSON.stringify(automation.writer.credentials) === JSON.stringify(agents.agents.writer.credentials),
  'writer credentials must remain publish-only without GITHUB_TOKEN write access',
);
assert(
  sameMembers(Object.keys(automation.writer), [
    'enabled', 'enabled_variable', 'branch_prefix', 'draft_pull_requests_only',
    'maximum_open_pull_requests_per_issue', 'identity', 'app_id_variable', 'private_key_secret',
    'environment', 'credentials', 'permissions', 'capability_residuals',
    'deterministic_client_mitigations', 'limits', 'forbidden_paths', 'release_secret_access',
    'pull_request_target_checkout',
  ]) &&
    automation.writer.branch_prefix === 'agent/' &&
    automation.writer.release_secret_access === false &&
    automation.writer.pull_request_target_checkout === false,
  'writer policy capabilities changed',
);
const expectedWriterPermissions = {
  metadata: 'read',
  contents: 'write',
  pull_requests: 'write',
  denied: [
    'checks', 'actions', 'workflows', 'administration', 'deployments', 'environments', 'secrets',
    'members', 'packages', 'issues',
  ],
};
const expectedWriterCapabilityResiduals = {
  pull_requests_write_can_review_or_merge: true,
  contents_write_not_branch_scoped: true,
  app_has_branch_protection_bypass: false,
};
const expectedWriterDeterministicClientMitigations = {
  allowed_operations: ['create_or_update_agent_ref', 'create_or_update_draft_pull_request'],
  denied_operations: ['review', 'approve', 'merge', 'enable_auto_merge', 'mark_ready', 'close_pr', 'delete_branch'],
};
const expectedWriterLimits = {
  maximum_files: 50,
  maximum_patch_bytes: 65536,
  maximum_file_size_bytes: 524288,
  maximum_total_file_bytes: 2097152,
  maximum_fix_cycles: 2,
};
const expectedWriterForbiddenPaths = [
  '.github/**', '**/CODEOWNERS', '.gitmodules', '**/.git', '**/.git/**',
];
assert(
    JSON.stringify(agents.agents.writer.permissions) === JSON.stringify(expectedWriterPermissions) &&
    JSON.stringify(automation.writer.permissions) === JSON.stringify(expectedWriterPermissions) &&
    JSON.stringify(agents.agents.writer.capability_residuals) ===
      JSON.stringify(expectedWriterCapabilityResiduals) &&
    JSON.stringify(automation.writer.capability_residuals) ===
      JSON.stringify(expectedWriterCapabilityResiduals) &&
    JSON.stringify(agents.agents.writer.deterministic_client_mitigations) ===
      JSON.stringify(expectedWriterDeterministicClientMitigations) &&
    JSON.stringify(automation.writer.deterministic_client_mitigations) ===
      JSON.stringify(expectedWriterDeterministicClientMitigations) &&
    JSON.stringify(agents.agents.writer.limits) === JSON.stringify(expectedWriterLimits) &&
    JSON.stringify(automation.writer.limits) === JSON.stringify(expectedWriterLimits) &&
    JSON.stringify(agents.agents.writer.denied_paths) === JSON.stringify(expectedWriterForbiddenPaths) &&
    JSON.stringify(automation.writer.forbidden_paths) === JSON.stringify(expectedWriterForbiddenPaths),
  'writer permissions, limits, or forbidden paths changed',
);
assert(automation.policy_gate.enabled === false, 'policy gate must default off');
assert(automation.policy_gate.mode === 'shadow', 'policy gate must start in shadow mode');
const expectedPolicyRegistryKeys = [
  'enabled', 'enabled_variable', 'phase', 'mode', 'identity', 'app_id_variable',
  'app_slug_variable', 'private_key_secret', 'environment', 'credentials', 'permissions',
  'deterministic_client_mitigations', 'model_variable', 'triggers', 'tools', 'effects', 'handoff_to',
];
const expectedPolicyGateKeys = [
  'enabled', 'enabled_variable', 'identity', 'app_id_variable', 'app_slug_variable',
  'private_key_secret', 'environment', 'credentials', 'permissions',
  'deterministic_client_mitigations', 'release_secret_access', 'pull_request_target_checkout',
  'check_name', 'mode', 'allowed_modes', 'human_enable_label', 'require_exact_head_sha',
  'require_base_up_to_date', 'require_conversation_resolution', 'required_checks',
  'required_check_sources', 'always_require_human_review', 'allowlist_paths',
];
const expectedPolicyPermissions = {
  metadata: 'read',
  contents: 'read',
  pull_requests: 'read',
  checks: 'write',
  denied: [
    'actions', 'statuses', 'issues', 'workflows', 'administration', 'deployments', 'environments',
    'secrets', 'members', 'packages',
  ],
};
const expectedPolicyMitigations = {
  allowed_operations: ['read_policy_inputs', 'create_or_update_policy_check'],
  denied_operations: [
    'contents_write', 'review', 'approve', 'merge', 'enable_auto_merge', 'mark_ready', 'close_pr',
    'delete_branch',
  ],
};
const expectedPolicySources = [
  { context: 'Rust CI / check', app_id: 15368, app_slug: 'github-actions' },
  { context: 'Frontend CI / check', app_id: 15368, app_slug: 'github-actions' },
];
const expectedPolicyHumanPaths = [
  '.github/**', 'CODEOWNERS', 'apps/**', 'crates/**', 'frontend/src/**', 'Cargo.toml',
  'Cargo.lock', '**/Cargo.toml', '**/Cargo.lock', 'frontend/package.json',
  'frontend/package-lock.json', 'Dockerfile*', 'docker-compose*.yml', 'deploy.sh', 'release/**',
  'scripts/release/**', '**/auth/**', '**/database/**', '**/db/**', '**/migrations/**',
  '**/security/**',
];
const policyAgent = agents.agents.policy;
assert(
  sameMembers(Object.keys(policyAgent), expectedPolicyRegistryKeys) &&
    sameMembers(Object.keys(automation.policy_gate), expectedPolicyGateKeys),
  'policy registry or gate fields changed',
);
assert(
  policyAgent.enabled === false &&
    policyAgent.enabled === automation.policy_gate.enabled &&
    policyAgent.enabled_variable === 'AERIS_POLICY_ENABLED' &&
    policyAgent.phase === 4 &&
    policyAgent.mode === 'deterministic' &&
    policyAgent.model_variable === null &&
    policyAgent.identity === 'github_app' &&
    policyAgent.app_id_variable === 'AERIS_POLICY_APP_ID' &&
    policyAgent.app_slug_variable === 'AERIS_POLICY_APP_SLUG' &&
    policyAgent.private_key_secret === 'AERIS_POLICY_PRIVATE_KEY' &&
    policyAgent.environment === 'policy',
  'policy Agent identity or phase changed',
);
assert(
  JSON.stringify(policyAgent.credentials) === JSON.stringify({ allowed_jobs: ['publish'], github_token_write: false }) &&
    JSON.stringify(policyAgent.credentials) === JSON.stringify(automation.policy_gate.credentials) &&
    JSON.stringify(policyAgent.permissions) === JSON.stringify(expectedPolicyPermissions) &&
    JSON.stringify(automation.policy_gate.permissions) === JSON.stringify(expectedPolicyPermissions) &&
    JSON.stringify(policyAgent.deterministic_client_mitigations) === JSON.stringify(expectedPolicyMitigations) &&
    JSON.stringify(automation.policy_gate.deterministic_client_mitigations) === JSON.stringify(expectedPolicyMitigations),
  'policy credentials, permissions, or deterministic client boundary changed',
);
assert(
  JSON.stringify(policyAgent.triggers) === JSON.stringify([
    'required_checks_completed', 'policy_signal_completed', 'default_branch_changed',
    'scheduled_reconciliation', 'manual_dispatch',
  ]) &&
    JSON.stringify(policyAgent.tools) === JSON.stringify([
      'repository_read', 'pull_request_metadata_read', 'checks_read', 'checks_write',
    ]) &&
    JSON.stringify(policyAgent.effects) === JSON.stringify(['publish_policy_check']) &&
    JSON.stringify(policyAgent.handoff_to) === JSON.stringify(['merger']),
  'policy Agent capabilities changed',
);
assert(
  automation.policy_gate.release_secret_access === false &&
    automation.policy_gate.pull_request_target_checkout === false &&
    automation.policy_gate.check_name === 'Automation Policy / gate' &&
    JSON.stringify(automation.policy_gate.allowed_modes) === JSON.stringify(['shadow', 'human', 'label', 'allowlist']) &&
    automation.policy_gate.human_enable_label === 'automerge-approved' &&
    automation.policy_gate.require_exact_head_sha === true &&
    automation.policy_gate.require_base_up_to_date === true &&
    automation.policy_gate.require_conversation_resolution === true &&
    JSON.stringify(automation.policy_gate.required_checks) ===
      JSON.stringify(['Rust CI / check', 'Frontend CI / check']) &&
    JSON.stringify(automation.policy_gate.required_check_sources) === JSON.stringify(expectedPolicySources) &&
    JSON.stringify(automation.policy_gate.always_require_human_review) === JSON.stringify(expectedPolicyHumanPaths),
  'policy gate deterministic constraints changed',
);
assert(
  automation.policy_gate.allowlist_paths.length === 0,
  'automatic merge allowlist must start empty',
);
for (const immutablePath of [
  '.github/agents.yml',
  '.github/automation-policy.yml',
  '.github/automation/**',
  '.github/upstream-sync-policy.yml',
  '.github/upstream-sync-state.json',
  '.github/workflows/**',
]) {
  assert(
    automation.trusted_source.immutable_paths.includes(immutablePath),
    `trusted immutable path missing: ${immutablePath}`,
  );
}

assert(sync.upstream.repository === state.repository, 'state repository differs from policy');
assert(sync.upstream.branch === state.branch, 'state branch differs from policy');
assert(sync.sync.state_file === '.github/upstream-sync-state.json', 'state path changed');
assert(sync.sync.pull_request_limit === 1, 'sync must allow only one managed PR');
assert(sync.sync.fail_closed === true, 'sync must fail closed');
assert(sync.matching.default === 'review_required', 'unknown paths must require review');
assert(
  JSON.stringify(sync.matching.precedence) ===
    JSON.stringify(['fork_owned', 'review_required', 'generated', 'upstream_owned']),
  'path ownership precedence changed',
);
assert(
  sync.matching.enforced_fork_owned_subset === 'exact_or_directory_recursive',
  'runtime fork-owned matcher contract changed',
);
for (const protectedPath of [
  '.github/upstream-sync-state.json',
  '.github/upstream-sync-policy.yml',
  '.github/workflows/**',
]) {
  assert(sync.fork_owned.includes(protectedPath), `fork-owned path missing: ${protectedPath}`);
}
assert(
  Object.values(sync.checkpoint).every((value) => value === true),
  'all checkpoint safety checks must remain enabled',
);
assert(sync.conflicts.overwrite_unknown_tip === false, 'unknown sync tips must not be overwritten');
assert(sync.conflicts.create_or_update_alert === true, 'sync failures must create alerts');
const syncSteps = syncWorkflow.jobs.sync.steps;
const autoMergeStep = syncSteps.find((step) => step.name === 'Enable native auto-merge');
const disarmCallIndex = syncScript.search(/^disarm_tracked_pr$/m);
const rebuildLoopIndex = syncScript.search(/^for attempt in 1 2 3; do$/m);
assert(autoMergeStep, 'sync workflow must expose the native auto-merge step');
assert(
  autoMergeStep.if === "steps.sync.outputs.has_changes == 'true'",
  'native auto-merge must require a published synchronization change',
);
assert(
  autoMergeStep.env.PR_URL === '${{ steps.sync.outputs.pr_url }}' &&
    autoMergeStep.env.SYNCED_SHA === '${{ steps.sync.outputs.synced_sha }}',
  'native auto-merge must bind the published PR URL and exact head SHA',
);
assert(
  typeof autoMergeStep.run === 'string' &&
    autoMergeStep.run.includes('manage-sync-automerge.sh') &&
    autoMergeStep.run.includes(' arm '),
  'native auto-merge must use the managed helper arm action',
);
assert(
  disarmCallIndex >= 0 && rebuildLoopIndex >= 0 && disarmCallIndex < rebuildLoopIndex,
  'sync must disarm a stale auto-merge before rebuilding its fixed branch',
);
assert(
  automation.authorization.external_pull_request_analysis_requires_label === 'agent-analyze',
  'external PR analysis must require agent-analyze',
);

assert(state.schema_version === 1, 'state schema version changed');
assert(state.policy_version === sync.version, 'state policy version differs from policy');
assert(/^[0-9a-f]{40}$/.test(state.last_integrated_sha), 'checkpoint must be a full lowercase SHA');

const directlyExecutedScripts = [
  '.github/workflows/scripts/checkpoint-merge.sh',
  '.github/workflows/scripts/manage-sync-automerge.sh',
  '.github/workflows/scripts/prepare-checkpoint-sync.sh',
];
const trackedModes = execFileSync('git', ['ls-files', '--stage', '--', ...directlyExecutedScripts], {
  cwd: repoRoot,
  encoding: 'utf8',
});
const modesByPath = new Map(
  trackedModes
    .trim()
    .split('\n')
    .filter(Boolean)
    .map((entry) => {
      const [metadata, file] = entry.split('\t');
      return [file, metadata.split(' ')[0]];
    }),
);
for (const script of directlyExecutedScripts) {
  assert(modesByPath.get(script) === '100755', `${script} must be executable`);
}

console.log(
  JSON.stringify({
    agents: expectedAgents.length,
    defaultEnabled: agents.runtime.default_enabled,
    policyMode: automation.policy_gate.mode,
    checkpoint: state.last_integrated_sha,
  }),
);
