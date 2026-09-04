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
  fs.readFileSync(path.join(repoRoot, relativePath), 'utf8').replace(/\r\n/g, '\n');
const loadYaml = (relativePath) => yaml.load(read(relativePath));
const assert = (condition, message) => {
  if (!condition) throw new Error(message);
};
const sameMembers = (actual, expected) =>
  actual.length === expected.length && expected.every((value) => actual.includes(value));

const agents = loadYaml('.github/agents.yml');
const automation = loadYaml('.github/automation-policy.yml');
const automationPolicyWorkflow = loadYaml('.github/workflows/automation-policy.yml');
const sync = loadYaml('.github/upstream-sync-policy.yml');
const frontendWorkflow = loadYaml('.github/workflows/frontend-ci.yml');
const rustWorkflow = loadYaml('.github/workflows/rust-ci.yml');
const boundedFetchScript = read('.github/workflows/scripts/bounded-git-fetch.sh');
const minimalSyncWorkflow = loadYaml('.github/workflows/sync-upstream-minimal.yml');
const minimalSyncWorkflowSource = read('.github/workflows/sync-upstream-minimal.yml');
const minimalSyncScript = read('.github/workflows/scripts/sync-upstream-minimal.sh');

for (const [name, workflow, context] of [
  ['frontend', frontendWorkflow, 'Frontend CI / check'],
  ['rust', rustWorkflow, 'Rust CI / check'],
]) {
  const check = workflow.jobs.check;
  const publisher = workflow.jobs.publish_dispatch_status;
  assert(check, `${name} CI must retain its required check job`);
  assert(
    JSON.stringify(workflow.permissions) === JSON.stringify({ contents: 'read' }),
    `${name} CI must define a workflow-wide read-only permission boundary`,
  );
  assert(
    JSON.stringify(check.permissions) === JSON.stringify({ contents: 'read' }),
    `${name} CI required check must explicitly receive only contents: read`,
  );
  assert(
    String(check.if).trim() === '${{ always() }}',
    `${name} CI required check must run after failed or skipped prerequisites`,
  );
  assert(publisher, `${name} CI must publish workflow_dispatch statuses in a separate job`);
  assert(
    JSON.stringify(publisher.permissions) ===
      JSON.stringify({ contents: 'read', statuses: 'write' }),
    `${name} CI dispatch publisher must explicitly receive only contents: read and statuses: write`,
  );
  for (const [jobName, job] of Object.entries(workflow.jobs)) {
    if (jobName === 'publish_dispatch_status') continue;
    const effectivePermissions = job.permissions ?? workflow.permissions;
    assert(
      JSON.stringify(effectivePermissions) === JSON.stringify({ contents: 'read' }),
      `${name} CI job ${jobName} must have effective contents: read-only permissions`,
    );
  }
  const statusWriters = Object.entries(workflow.jobs)
    .filter(([, job]) => job.permissions?.statuses === 'write')
    .map(([jobName]) => jobName);
  assert(
    sameMembers(statusWriters, ['publish_dispatch_status']),
    `${name} CI must grant statuses: write only to its dispatch publisher`,
  );
  assert(
    String(publisher.if).includes("github.event_name == 'workflow_dispatch'"),
    `${name} CI status publisher must run only for workflow_dispatch`,
  );
  assert(publisher.needs === 'check', `${name} CI status publisher must wait for the required check`);
  const publishStep = publisher.steps.find((step) => step.name.startsWith('Publish '));
  assert(publishStep, `${name} CI dispatch publisher must contain a status publish step`);
  assert(
    publishStep.run.includes('CHECK_RESULT') &&
      publishStep.run.includes('result=failure') &&
      publishStep.run.includes(`-f context=\"${context}\"`),
    `${name} CI dispatch publisher must report the check conclusion for ${context}`,
  );
}

for (const contract of [agents, automation, sync]) {
  assert(contract.version === 1, 'all automation contracts must use version 1');
}
assert(
  JSON.stringify(sync.resource_bounds) === JSON.stringify({
    fetch_timeout_seconds: 90,
    max_received_bytes: 268435456,
    max_received_expanded_bytes: 1073741824,
    max_received_objects: 250000,
    max_import_bytes: 268435456,
    max_import_objects: 250000,
    max_process_memory_bytes: 536870912,
    max_object_bytes: 33554432,
    max_blob_bytes: 33554432,
    max_changed_blob_bytes: 268435456,
    max_changed_paths: 20000,
    max_tree_entries: 500000,
    max_diff_bytes: 33554432,
  }),
  'sync resource-bound constants must remain deterministic',
);

const expectedAgents = [
  'triage',
  'planner',
  'reviewer',
  'writer',
  'tester',
  'security',
  'policy',
];
assert(
  sameMembers(Object.keys(agents.agents), expectedAgents),
  'agent registry must contain exactly the approved roles',
);
assert(
  boundedFetchScript.includes('AERIS_FETCH_MAX_REMOTE_REF_BYTES=4096') &&
    boundedFetchScript.includes('AERIS_BOUNDED_NETWORK_MAX_FILE_BYTES') &&
    boundedFetchScript.includes('aeris-remote-ref.XXXXXX') &&
    !boundedFetchScript.includes(
      'output="$(aeris_bounded_network_git ls-remote',
    ),
  'exact-ref discovery stdout must be file-bounded before shell parsing',
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
for (const name of ['tester', 'policy']) {
  assert(agents.agents[name].mode === 'deterministic', `${name} must be deterministic`);
  assert(agents.agents[name].model_variable === null, `${name} must not select a model`);
}
assert(
  agents.agents.policy.handoff_to.length === 0,
  'policy must terminate in the deterministic gate; Finalizer owns direct merge execution',
);
assert(
  agents.agents.writer.mode === 'credentialless_candidate' &&
    agents.agents.writer.effects.includes('publish_candidate_artifact') &&
    agents.agents.writer.denied_paths.includes('.github/**'),
  'Agent writer must remain credentialless and deny .github',
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
const [policyWorkflowName, policyJobSuffix] = automation.policy_gate.check_name.split(' / ');
assert(
  automationPolicyWorkflow.name === policyWorkflowName &&
    automationPolicyWorkflow.jobs.gate.name === automation.policy_gate.check_name &&
    automationPolicyWorkflow.jobs.gate.name.endsWith(` / ${policyJobSuffix}`),
  'policy workflow name and explicit job check context must match the required policy check context',
);
assert(
  JSON.stringify(automationPolicyWorkflow.on.pull_request?.types) ===
    JSON.stringify(['opened', 'reopened', 'synchronize', 'labeled', 'unlabeled']),
  'policy workflow must remain event-driven for every policy-relevant pull request event',
);
// The dispatch lane (#181) exists for GITHUB_TOKEN-created sync PRs, which
// emit no pull_request event. It must pin the exact evaluation target so the
// gate cannot be pointed at an unrelated commit.
assert(
  sameMembers(Object.keys(automationPolicyWorkflow.on.workflow_dispatch?.inputs ?? {}), [
    'ref',
    'pull_number',
    'policy_sha',
  ]) &&
    automationPolicyWorkflow.on.workflow_dispatch.inputs.ref.required === true &&
    automationPolicyWorkflow.on.workflow_dispatch.inputs.pull_number.required === true &&
    automationPolicyWorkflow.on.workflow_dispatch.inputs.policy_sha.required === true &&
    automationPolicyWorkflow.on.workflow_dispatch.inputs.pull_number.type === 'number',
  'policy workflow dispatch must take exactly the ref, pull request number, and trusted policy SHA',
);
assert(
  automationPolicyWorkflow.jobs.gate.if === undefined,
  'policy workflow scheduling must not be disabled by the policy gate or mutation flags',
);
const policyGateSteps = automationPolicyWorkflow.jobs.gate.steps;
const policyEvaluationStep = policyGateSteps.find((step) => step.name === 'Evaluate deterministic policy');
assert(
  policyEvaluationStep && policyGateSteps[policyGateSteps.length - 1] === policyEvaluationStep,
  'policy workflow must end with its deterministic evaluation step',
);
assert(
  policyGateSteps.every((step) => step.if === undefined),
  'policy workflow steps must not be conditionally skipped for required pull request events',
);
assert(
  typeof policyEvaluationStep?.run === 'string' &&
    policyEvaluationStep.run.includes('node .github/automation/src/run-autonomy-policy.mjs') &&
    (policyEvaluationStep.run.match(/node \.github\/automation\/src\/run-autonomy-policy\.mjs/g) ?? []).length === 1 &&
    policyEvaluationStep.run.includes('exit 1') &&
    !/\bexit 0\b/.test(policyEvaluationStep.run),
  'policy workflow must execute the deterministic gate and fail closed when its runtime is unavailable',
);
assert(automation.policy_gate.mode === 'canary_allowlist', 'policy gate must remain limited to the canary allowlist');
assert(
  JSON.stringify(automation.policy_gate.allowlist_paths) === JSON.stringify(['docs/automation-canary/**/*.md']),
  'automatic merge allowlist must remain restricted to canary Markdown',
);
assert(automation.finalizer.merge_mode === 'github_direct_merge', 'finalizer must use direct GitHub merge');
assert(automation.finalizer.merge_method === 'squash', 'finalizer must use squash merge');
assert(automation.finalizer.direct_merge === true, 'finalizer must not arm native auto-merge');
assert(automation.finalizer.require_policy_classification === 'eligible', 'finalizer classification gate changed');
for (const immutablePath of [
  '.github/agents.yml',
  '.github/automation-policy.yml',
  '.github/automation/**',
  '.github/upstream-sync-policy.yml',
  '.github/workflows/**',
  'CODEOWNERS',
  'Cargo.toml',
  'Cargo.lock',
  'frontend/package.json',
  'frontend/package-lock.json',
]) {
  assert(
    automation.trusted_source.immutable_paths.includes(immutablePath),
    `trusted immutable path missing: ${immutablePath}`,
  );
}

// The checkpoint state file was retired in #179 Phase 1a and the legacy sync
// chain in Phase 1b: merge-base against the upstream tip is the sync progress
// signal, so the state_file policy key is removed here as well.
assert(sync.sync.state_file === undefined, 'the retired state_file policy key must stay removed');
assert(sync.sync.pull_request_limit === 1, 'sync must allow only one managed PR');
assert(sync.sync.fail_closed === true, 'sync must fail closed');
assert(sync.matching.default === 'review_required', 'unknown paths must require review');
assert(
  JSON.stringify(sync.matching.precedence) ===
    JSON.stringify(['sensitive', 'review_required', 'fork_owned', 'generated', 'upstream_owned']),
  'path ownership precedence changed',
);
assert(
  sync.matching.syntax === 'aeris-glob-v1' &&
    sync.matching.enforced_fork_owned_subset === 'exact_or_directory_recursive',
  'runtime path matcher contract changed',
);
assert(
  JSON.stringify(sync.sensitive) === JSON.stringify(['.gitmodules', '**/*.pem', '**/*.key', '**/*.p12']),
  'sensitive path contract changed',
);
for (const protectedPath of [
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
  sync.conflicts.ai_resolution.enabled === true &&
    sync.conflicts.ai_resolution.profile === 'aeris-sync-conflict-v2' &&
    sync.conflicts.ai_resolution.required_pre_conflict_verdict === 'eligible' &&
    sync.conflicts.ai_resolution.allowed_type === 'modify_modify_utf8_text' &&
    sync.conflicts.ai_resolution.allowed_mode === '100644' &&
    sync.conflicts.ai_resolution.maximum_files === 16 &&
    sync.conflicts.ai_resolution.maximum_bytes_per_file === 16384 &&
    sync.conflicts.ai_resolution.maximum_total_input_bytes === 65536 &&
    sync.conflicts.ai_resolution.resolver_model_variable === 'AERIS_AI_MODEL_CONFLICT_RESOLVER' &&
    sync.conflicts.ai_resolution.reviewer_model_variable === 'AERIS_AI_MODEL_CONFLICT_REVIEWER' &&
    sync.conflicts.ai_resolution.require_distinct_model_ids === true &&
    sync.conflicts.ai_resolution.require_complete_resolution === true &&
    sync.conflicts.ai_resolution.require_independent_review_pass === true &&
    sync.conflicts.ai_resolution.allow_non_conflict_edits === false &&
    sync.conflicts.ai_resolution.allow_sensitive_or_review_required_paths === false &&
    sync.conflicts.ai_resolution.allow_binary_rename_delete_mode_or_case_ambiguity === false,
  'AI conflict resolution policy must remain narrow, independent, and fail closed',
);

// --- Shared bounded Git transport (bounded-git-fetch.sh) ---
// The legacy sync producer/verifier/conflict chain was retired in #179 Phase
// 1b; the minimal loop is the surviving consumer of this helper.
assert(
  boundedFetchScript.includes('fetch.fsckObjects=true') &&
    boundedFetchScript.includes('fetch.unpackLimit=0') &&
    boundedFetchScript.includes('ulimit -f "${file_blocks}"') &&
    boundedFetchScript.includes('ulimit -v "${memory_kib}"') &&
    boundedFetchScript.includes('fsck --strict') &&
    boundedFetchScript.includes('--no-tags --no-recurse-submodules --refmap=') &&
    boundedFetchScript.includes('aeris_enforce_change_bounds') &&
    boundedFetchScript.includes('export GIT_NO_LAZY_FETCH=1') &&
    !boundedFetchScript.includes('>"${stage}/objects/info/alternates"'),
  'the shared bounded fetch helper must remain exact-ref, resource-bounded, and fsck-verified',
);

const deadlineRunnerMatch = boundedFetchScript.match(
  /aeris_bounded_run_deadline\(\) \{[\s\S]*?\n\}/,
);
assert(
  deadlineRunnerMatch &&
    !deadlineRunnerMatch[0].includes('ulimit -v') &&
    deadlineRunnerMatch[0].includes('ulimit -f "${file_blocks}"') &&
    deadlineRunnerMatch[0].includes('timeout -k 5s'),
  'the deadline runner must keep the timeout and file bound without the virtual-memory ceiling Go binaries cannot start under',
);
const fetchRefMatch = boundedFetchScript.match(
  /aeris_bounded_fetch_ref\(\) \{[\s\S]*?\n\}/,
);
assert(
  fetchRefMatch &&
    fetchRefMatch[0].includes('AERIS_FETCH_RECEIVED_EXPANDED_BYTES_TOTAL=0') &&
    fetchRefMatch[0].includes('AERIS_FETCH_IMPORT_OBJECTS_TOTAL=0') &&
    fetchRefMatch[0].indexOf('AERIS_FETCH_RECEIVED_EXPANDED_BYTES_TOTAL=0') <
      fetchRefMatch[0].indexOf('aeris_bounded_read_remote_ref'),
  'received and import budgets must reset per exact-ref fetch so consecutive full-history fetches keep a single-stage guard',
);
assert(
  boundedFetchScript.includes('--no-write-fetch-head') &&
    boundedFetchScript.includes(
      'git update-ref "${destination}" "${expected}" "${previous_ref:-${zero}}"',
    ) &&
    boundedFetchScript.includes('destination changed concurrently before publication') &&
    boundedFetchScript.includes('destination did not retain the exact validated SHA') &&
    !boundedFetchScript.includes('aeris_bounded_rollback_import') &&
    !boundedFetchScript.includes('caller-objects-before'),
  'validated objects must publish through an exact-ref CAS without shared-object rollback',
);

assert(
  boundedFetchScript.includes('git rev-parse --absolute-git-dir') &&
    boundedFetchScript.includes('git rev-parse --git-common-dir') &&
    boundedFetchScript.includes('git worktree list --porcelain') &&
    boundedFetchScript.includes('repositories with linked worktrees are forbidden'),
  'bootstrap conversion must reject linked or shared worktrees before object deletion',
);

assert(
  boundedFetchScript.includes('-c credential.helper=') &&
    boundedFetchScript.includes('-c http.https://github.com/.extraheader='),
  'bounded Git transport must strip inherited credentials and authentication headers',
);

assert(
  boundedFetchScript.includes('aeris_bounded_assert_credentialless_transport') &&
    boundedFetchScript.includes('GIT_SSH_COMMAND') &&
    boundedFetchScript.includes('SSH_ASKPASS') &&
    boundedFetchScript.includes('GIT_CONFIG_COUNT') &&
    boundedFetchScript.includes('url\\..*\\.(insteadof|pushinsteadof)') &&
    boundedFetchScript.includes('credential(\\..*)?') &&
    boundedFetchScript.includes('http\\..*'),
  'credentialless Git transport must reject inherited environment and configuration overrides',
);
assert(
  boundedFetchScript.includes('git verify-pack -v "$1" | awk') &&
    boundedFetchScript.includes("cat-file --batch-check") &&
    boundedFetchScript.includes('metadata_limit=$((current * 128 + 1))'),
  'verify-pack and cat-file aggregation must be generated inside bounded pipelines',
);

assert(
  automation.authorization.external_pull_request_analysis_requires_label === 'agent-analyze',
  'external PR analysis must require agent-analyze',
);

// --- Minimal upstream sync loop (#175; sole sync path since #179 Phase 1b) ---
// The legacy sync-upstream.yml is retired; the minimal loop is now the only
// consumer of the sync-upstream-main concurrency group, which still must not
// cancel an in-progress run.
assert(
  minimalSyncWorkflow.concurrency?.group === 'sync-upstream-main' &&
    minimalSyncWorkflow.concurrency['cancel-in-progress'] === false,
  'the minimal sync workflow must keep the sync-upstream-main mutex with cancel-in-progress: false',
);
assert(
  Array.isArray(minimalSyncWorkflow.on.schedule) &&
    minimalSyncWorkflow.on.schedule.length === 1 &&
    typeof minimalSyncWorkflow.on.schedule[0].cron === 'string' &&
    ![0, 30].includes(Number(minimalSyncWorkflow.on.schedule[0].cron.split(' ')[0])) &&
    minimalSyncWorkflow.on.workflow_dispatch !== undefined,
  'minimal sync must run daily off the round hour and support manual dispatch',
);
const minimalSyncJob = minimalSyncWorkflow.jobs.sync;
assert(
  JSON.stringify(minimalSyncJob.permissions) ===
    JSON.stringify({
      contents: 'write',
      'pull-requests': 'write',
      issues: 'write',
      actions: 'write',
    }),
  'minimal sync GITHUB_TOKEN permissions must be exactly contents/pull-requests/issues/actions write',
);
assert(
  !/secrets\.|create-github-app-token|AERIS_AI_|AERIS_WRITER_APP|environment:/.test(
    minimalSyncWorkflowSource,
  ),
  'minimal sync must use GITHUB_TOKEN only: no secrets, Writer App, AI configuration, or environments',
);
assert(
  minimalSyncJob.if.includes("vars.AERIS_UPSTREAM_SYNC_ENABLED == 'true'") &&
    minimalSyncJob.if.includes("vars.AERIS_UPSTREAM_SYNC_ENABLED == '1'") &&
    minimalSyncJob.if.includes('github.event.repository.default_branch'),
  'minimal sync must run only on the default branch with the upstream-sync lane enabled',
);
const minimalCheckout = minimalSyncJob.steps.find((step) =>
  String(step.uses).startsWith('actions/checkout@'),
);
assert(
  minimalCheckout?.with?.['persist-credentials'] === false &&
    minimalCheckout.with['fetch-depth'] === 1 &&
    minimalCheckout.with.token === '${{ github.token }}',
  'minimal sync checkout must use the workflow token without persisted credentials',
);
assert(
  minimalSyncJob.steps.some(
    (step) =>
      step.run === 'bash .github/workflows/scripts/sync-upstream-minimal.sh' &&
      step.env?.GH_TOKEN === '${{ github.token }}',
  ),
  'minimal sync must execute the loop script with the workflow job token',
);

// Every network git transfer must go through the shared bounded helper; bare
// fetch/ls-remote/push is forbidden in the loop script.
assert(
  minimalSyncScript.includes('source "${SCRIPT_DIR}/bounded-git-fetch.sh"') &&
    minimalSyncScript.includes('aeris_bounded_fetch_init "${SYNC_POLICY_FILE}"') &&
    (minimalSyncScript.match(/aeris_bounded_fetch_ref /g) ?? []).length >= 1 &&
    !/\bgit\s+(fetch|ls-remote|push)\b/.test(minimalSyncScript) &&
    minimalSyncScript.includes('bounded_git_push push'),
  'minimal sync must route all git transport through bounded-git-fetch.sh and the bounded push helper',
);
assert(
  minimalSyncScript.includes('SYNC_BRANCH="${SYNC_BRANCH:-sync/upstream}"'),
  'the minimal sync loop must publish through its fixed synchronization branch',
);

// Idempotent PR reuse: fixed head/base inventory plus an upstream-SHA marker.
assert(
  minimalSyncScript.includes('head=${REPO_OWNER}:${SYNC_BRANCH}') &&
    minimalSyncScript.includes('<!-- upstream-sync-minimal-upstream:') &&
    minimalSyncScript.includes('<!-- upstream-sync-minimal-managed -->'),
  'minimal sync PR reuse key must be the fixed head/base pair plus the upstream SHA marker',
);

// Fail-closed alerts must pass the raw error output through into the issue
// body (#172 lesson). The fourth argument is mandatory and every call site
// follows one uniform shape.
assert(
  /report_sync_alert\(\) \{[\s\S]*?\[\[ -n "\$\{raw\}" \]\]/.test(minimalSyncScript) &&
    minimalSyncScript.includes('### Raw error'),
  'minimal sync alert helper must require and publish the raw error detail',
);
const minimalAlertCalls = minimalSyncScript.match(/^[ \t]+report_sync_alert [a-z-]+ [^\n]*$/gm) ?? [];
assert(
  minimalAlertCalls.length >= 5 &&
    minimalAlertCalls.every((call) =>
      /^[ \t]+report_sync_alert [a-z-]+ "\$\{[^}]+\}" "\$\{summary_msg\}" "\$\{raw\}"$/.test(call),
    ),
  'every minimal sync alert call site must pass summary plus raw error detail',
);

// Auto-merge is armed exactly once, with the merge method, and only after the
// conflict and .github/** drift paths have already failed closed.
const minimalAutoMergeIndex = minimalSyncScript.indexOf('--auto --merge');
assert(
  minimalAutoMergeIndex > -1 &&
    minimalSyncScript.indexOf('--auto --merge') ===
      minimalSyncScript.lastIndexOf('--auto --merge') &&
    !/pr merge[^\n]*(--squash|--rebase)/.test(minimalSyncScript) &&
    minimalSyncScript.indexOf('report_sync_alert conflict ') > -1 &&
    minimalSyncScript.indexOf('report_sync_alert conflict ') < minimalAutoMergeIndex &&
    minimalSyncScript.indexOf('report_sync_alert workflow-drift ') > -1 &&
    minimalSyncScript.indexOf('report_sync_alert workflow-drift ') < minimalAutoMergeIndex &&
    minimalSyncScript.indexOf('exit 1', minimalSyncScript.indexOf('report_sync_alert conflict ')) <
      minimalAutoMergeIndex,
  'minimal sync must arm auto-merge (merge method) only on the conflict-free, drift-free path',
);

// Merge-commit discipline: true merge (ancestor connectivity), verified after
// the auto-merge lands; the workflow file itself documents the discipline.
assert(
  minimalSyncScript.includes('--no-ff') &&
    minimalSyncScript.includes('rev-list --count') &&
    minimalSyncScript.includes('"${behind}" != 0') &&
    /grep -c '\^parent '/.test(minimalSyncScript) &&
    minimalSyncScript.includes('report_sync_alert merge-discipline '),
  'minimal sync must verify behind==0 and a two-parent merge commit after the auto-merge lands',
);
assert(
  /merge commit/i.test(minimalSyncWorkflowSource) &&
    minimalSyncWorkflowSource.includes('rev-list --count'),
  'minimal sync workflow must document the merge-commit discipline in a comment',
);

// The GITHUB_TOKEN event-suppression gap must surface as an alert instead of
// an eternal auto-merge wait.
assert(
  minimalSyncScript.includes('report_sync_alert missing-required-check '),
  'minimal sync must fail closed when a required check context can never appear',
);

// All three required contexts are dispatched onto the sync branch (#181); the
// gate dispatch pins the exact PR number and the validated main tip so the
// policy evaluation re-validates them against the live API and fails closed.
assert(
  minimalSyncScript.includes('ensure_check_dispatch rust-ci.yml "Rust CI / check"') &&
    minimalSyncScript.includes('ensure_check_dispatch frontend-ci.yml "Frontend CI / check"') &&
    minimalSyncScript.includes('ensure_check_dispatch automation-policy.yml "Automation Policy / gate"') &&
    minimalSyncScript.includes('-f "ref=${SYNC_BRANCH}"') &&
    minimalSyncScript.includes('-f "pull_number=${pr_number}"') &&
    minimalSyncScript.includes('-f "policy_sha=${base_sha}"'),
  'minimal sync must dispatch all three required checks, pinning the PR number and policy SHA for the gate',
);

// The loop script follows the executable-bit convention of the other scripts.
const minimalScriptMode = execFileSync(
  'git',
  ['ls-files', '--stage', '--', '.github/workflows/scripts/sync-upstream-minimal.sh'],
  { cwd: repoRoot, encoding: 'utf8' },
).split(' ')[0];
assert(minimalScriptMode === '100755', 'sync-upstream-minimal.sh must be executable');

// gh api defaults to POST when -f/-F fields are present. A read without an
// explicit --method became a pull-request creation attempt and turned the
// alert comment lookup into a bodiless POST (HTTP 422, #180 first run), and
// the alert path died unguarded. Pin the method discipline across every
// bounded gh api call in the loop script.
const minimalLogicalLines = [];
{
  let pendingLine = '';
  for (const line of minimalSyncScript.split('\n')) {
    pendingLine = pendingLine === '' ? line : `${pendingLine} ${line.trim()}`;
    if (!pendingLine.endsWith('\\')) {
      minimalLogicalLines.push(pendingLine);
      pendingLine = '';
    } else {
      pendingLine = pendingLine.slice(0, -1);
    }
  }
  if (pendingLine !== '') minimalLogicalLines.push(pendingLine);
}
const minimalFieldedApiCalls = minimalLogicalLines.filter(
  (line) => line.includes('bounded_gh api ') && /\s-[fF]\s/.test(line),
);
assert(
  minimalFieldedApiCalls.length >= 4 &&
    minimalFieldedApiCalls.every((line) => /--method (GET|POST|PATCH|PUT|DELETE)\b/.test(line)),
  'every bounded gh api call with -f/-F fields must pin an explicit --method (gh defaults to POST)',
);
assert(
  minimalSyncScript.includes('alert comment inventory failed') &&
    minimalSyncScript.includes('alert issue create failed') &&
    minimalSyncScript.includes('alert issue inventory failed'),
  'the minimal sync alert helper must fail loudly to stderr when its own gh calls fail',
);
assert(
  frontendWorkflow.jobs.automation.steps.some(
    (step) => step.run === 'bash ../workflows/scripts/tests/test-sync-minimal-alerts.sh',
  ),
  'required CI must execute the minimal sync alert method-discipline regression test',
);
const minimalAlertTestMode = execFileSync(
  'git',
  ['ls-files', '--stage', '--', '.github/workflows/scripts/tests/test-sync-minimal-alerts.sh'],
  { cwd: repoRoot, encoding: 'utf8' },
).split(' ')[0];
assert(minimalAlertTestMode === '100755', 'test-sync-minimal-alerts.sh must be executable');

console.log(
  JSON.stringify({
    agents: expectedAgents.length,
    defaultEnabled: agents.runtime.default_enabled,
    policyMode: automation.policy_gate.mode,
  }),
);
