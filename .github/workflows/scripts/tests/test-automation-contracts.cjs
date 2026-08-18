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
const frontendWorkflow = loadYaml('.github/workflows/frontend-ci.yml');
const syncScript = read('.github/workflows/scripts/sync-upstream.sh');
const autoMergeScript = read('.github/workflows/scripts/manage-sync-automerge.sh');
const autonomyScript = read('.github/workflows/scripts/github-autonomy.sh');
const checkDispatchScript = read('.github/workflows/scripts/ensure-required-checks.sh');
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
const syncSteps = syncWorkflow.jobs.sync.steps;
assert(syncWorkflow.jobs.sync.environment === 'sync', 'sync must use the dedicated sync environment');
assert(
  syncWorkflow.jobs.sync.if.includes("vars.AERIS_SYNC_APP_ENABLED == 'true'") &&
    syncWorkflow.jobs.sync.if.includes("vars.AERIS_SYNC_APP_ENABLED == '1'") &&
    syncWorkflow.jobs.sync.if.includes("vars.AERIS_AGENTS_ENABLED == 'true'") &&
    syncWorkflow.jobs.sync.if.includes("vars.AERIS_AGENTS_ENABLED == '1'"),
  'sync must remain disabled unless the bounded Sync App switch is enabled',
);
assert(
  syncWorkflow.jobs.sync.permissions.contents === 'read' &&
    syncWorkflow.jobs.sync.permissions['pull-requests'] === 'read' &&
    syncWorkflow.jobs.sync.permissions.actions === 'write' &&
    syncWorkflow.jobs.sync.permissions.checks === 'read' &&
    syncWorkflow.jobs.sync.permissions.statuses === 'read',
  'sync GITHUB_TOKEN permissions must remain read-only for repository mutation',
);
const syncTokenStep = syncSteps.find((step) => step.name === 'Mint bounded Sync App token');
assert(syncTokenStep && syncTokenStep.uses.includes('create-github-app-token@'), 'sync must mint its independent App token');
assert(
  syncTokenStep['timeout-minutes'] === 2,
  'App token mint must remain within the reserved autonomy margin',
);
assert(
  syncTokenStep.with['permission-contents'] === 'write' &&
    syncTokenStep.with['permission-pull-requests'] === 'write' &&
    syncTokenStep.with['permission-issues'] === 'write' &&
    syncTokenStep.with['permission-checks'] === 'read' &&
    syncTokenStep.with['permission-statuses'] === 'read',
  'Sync App token permissions exceed or miss the approved minimum',
);
assert(
  syncWorkflow.jobs.sync.env.AERIS_AUTONOMY_EXPIRES_AT ===
    '${{ vars.AERIS_AUTONOMY_EXPIRES_AT }}',
  'every sync phase must receive the bounded autonomy expiry',
);
assert(
  syncWorkflow.jobs.sync.env.AERIS_SYNC_APP_SLUG === '${{ vars.AERIS_SYNC_APP_SLUG }}',
  'sync must receive its explicit GitHub App slug for actor identity migration',
);
assert(
  syncWorkflow.jobs.sync.env.AERIS_AUTONOMY_MIN_REMAINING_SECONDS === 600,
  'unguarded token actions must reserve a conservative ten-minute autonomy margin',
);
const tokenStepIndex = syncSteps.indexOf(syncTokenStep);
const preMintExpiryStep = syncSteps[tokenStepIndex - 1];
assert(
  preMintExpiryStep?.name === 'Validate autonomy before token mint' &&
    typeof preMintExpiryStep.run === 'string' &&
    preMintExpiryStep.run.includes('AERIS_AUTONOMY_EXPIRES_AT') &&
    preMintExpiryStep.run.includes('AERIS_SYNC_APP_SLUG') &&
    preMintExpiryStep.run.includes(
      'now_epoch + AERIS_AUTONOMY_MIN_REMAINING_SECONDS',
    ),
  'sync must fail closed immediately before minting the App token',
);
const checkoutStep = syncSteps.find((step) => step.name === 'Check out fork default branch');
assert(checkoutStep?.with?.token === '${{ steps.sync_token.outputs.token }}', 'sync checkout must use the Sync App token');
assert(
  checkoutStep['timeout-minutes'] === 5,
  'authenticated checkout must remain within the reserved autonomy margin',
);
const checkoutStepIndex = syncSteps.indexOf(checkoutStep);
assert(
  syncSteps[checkoutStepIndex - 1]?.name === 'Validate autonomy before checkout',
  'sync must revalidate expiry immediately before checkout uses the App token',
);
assert(
  syncSteps[checkoutStepIndex - 1].run.includes(
    'now_epoch + AERIS_AUTONOMY_MIN_REMAINING_SECONDS',
  ),
  'checkout must reserve the conservative autonomy margin before using the App token',
);
const publishStep = syncSteps.find(
  (step) => step.name === 'Build and publish automation branch',
);
assert(
  publishStep?.env.GH_TOKEN === '${{ steps.sync_token.outputs.token }}',
  'sync publication must use the bounded Sync App token',
);
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
    autoMergeStep.env.SYNCED_SHA === '${{ steps.sync.outputs.synced_sha }}' &&
    autoMergeStep.env.GH_TOKEN === '${{ steps.sync_token.outputs.token }}',
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
  /aeris_require_active_autonomy_window[\s\S]*now_epoch \+ minimum_remaining_seconds >= expires_epoch/.test(
    autonomyScript,
  ) &&
    autonomyScript.includes('aeris_require_active_autonomy_window || return'),
  'GitHub wrappers must revalidate the exact UTC expiry before every invocation',
);
assert(
  !/(^|\n)\s*gh\s/.test(syncScript) &&
    !/(^|\n)\s*gh\s/.test(autoMergeScript) &&
    syncScript.includes('source "${SCRIPT_DIR}/github-autonomy.sh"') &&
    autoMergeScript.includes('source "${SCRIPT_DIR}/github-autonomy.sh"'),
  'sync and auto-merge must not bypass the expiry-guarded GitHub wrapper',
);
assert(
  syncScript.includes('SYNC_APP_BOT_LOGIN="${AERIS_SYNC_APP_SLUG}[bot]"') &&
    syncScript.includes("LEGACY_BOT_LOGIN='github-actions[bot]'") &&
    syncScript.includes(
      '.user.login == \\"${SYNC_APP_BOT_LOGIN}\\" or .user.login == \\"${LEGACY_BOT_LOGIN}\\"',
    ) &&
    syncScript.includes('is_sync_automation_login'),
  'comment and PR identity checks must accept the Sync App bot and migrate legacy Actions state',
);
assert(
  !/(^|\n)\s*git\s+(fetch|push|ls-remote)\b/.test(syncScript),
  'authenticated Git network operations must not bypass expiry revalidation',
);
const checkDispatchStep = syncSteps.find(
  (step) => step.name === 'Ensure required checks are dispatched',
);
assert(
  syncWorkflow.jobs.sync.permissions.actions === 'write' &&
    checkDispatchStep?.env.GH_TOKEN === '${{ github.token }}',
  'fallback dispatch must explicitly use a workflow job token with actions:write',
);
assert(
  checkDispatchStep?.run === 'bash .github/workflows/scripts/ensure-required-checks.sh' &&
    checkDispatchScript.includes('source "${SCRIPT_DIR}/github-autonomy.sh"') &&
    checkDispatchScript.includes('aeris_gh workflow run') &&
    !/(^|\n)\s*gh\s/.test(checkDispatchScript),
  'check discovery and dispatch must revalidate expiry before every GitHub token use',
);
const validationStep = syncSteps.find(
  (step) => step.name === 'Validate checkpoint synchronization',
);
assert(
  validationStep?.run.includes('test-github-autonomy.sh'),
  'workflow validation must exercise expiry crossing between planning and mutation',
);
assert(
  validationStep?.run.includes('test-ensure-required-checks.sh'),
  'workflow validation must exercise fallback dispatch with the explicit job token',
);
assert(
  validationStep?.run.includes('test-sync-upstream-identity.sh'),
  'workflow validation must exercise Sync App comment identity migration',
);
assert(
  frontendWorkflow.jobs.automation.steps.some(
    (step) => step.run === 'bash ../workflows/scripts/tests/test-github-autonomy.sh',
  ),
  'required CI must execute the fake-clock autonomy integration test',
);
assert(
  frontendWorkflow.jobs.automation.steps.some(
    (step) => step.run === 'bash ../workflows/scripts/tests/test-ensure-required-checks.sh',
  ),
  'required CI must execute the workflow job-token dispatch integration test',
);
assert(
  frontendWorkflow.jobs.automation.steps.some(
    (step) => step.run === 'bash ../workflows/scripts/tests/test-sync-upstream-identity.sh',
  ),
  'required CI must execute the Sync App comment identity integration test',
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
