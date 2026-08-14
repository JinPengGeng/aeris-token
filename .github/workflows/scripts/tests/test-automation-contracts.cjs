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
    name === 'triage' ? agent.enabled === true : agent.enabled === false,
  ),
  'only the triage agent may be enabled; every other agent must default off',
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
    agents.agents.writer.denied_paths.includes('.github/**'),
  'writer boundary must remain Draft PR only and deny .github',
);
assert(
  !agents.agents.reviewer.handoff_to.includes('writer'),
  'reviewer must not authorize a new writer run',
);

assert(automation.kill_switch.default_enabled === false, 'kill switch must default off');
assert(automation.writer.enabled === false, 'writer policy must default off');
assert(
  automation.writer.draft_pull_requests_only === true &&
    automation.writer.forbidden_paths.includes('.github/**'),
  'writer policy boundary changed',
);
assert(automation.policy_gate.enabled === false, 'policy gate must default off');
assert(automation.policy_gate.mode === 'shadow', 'policy gate must start in shadow mode');
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
assert(
  automation.authorization.external_pull_request_analysis_requires_label === 'agent-analyze',
  'external PR analysis must require agent-analyze',
);

assert(state.schema_version === 1, 'state schema version changed');
assert(state.policy_version === sync.version, 'state policy version differs from policy');
assert(/^[0-9a-f]{40}$/.test(state.last_integrated_sha), 'checkpoint must be a full lowercase SHA');

const directlyExecutedScripts = [
  '.github/workflows/scripts/checkpoint-merge.sh',
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
