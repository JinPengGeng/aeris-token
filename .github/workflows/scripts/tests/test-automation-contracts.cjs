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
assert(
  automationPolicyWorkflow.jobs.gate.name === automation.policy_gate.check_name,
  'policy workflow job name must match the required policy check context',
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
  '.github/upstream-sync-state.json',
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

assert(sync.upstream.repository === state.repository, 'state repository differs from policy');
assert(sync.upstream.branch === state.branch, 'state branch differs from policy');
assert(sync.sync.state_file === '.github/upstream-sync-state.json', 'state path changed');
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
  sync.conflicts.ai_resolution.enabled === true &&
    sync.conflicts.ai_resolution.profile === 'aeris-sync-conflict-v1' &&
    sync.conflicts.ai_resolution.required_pre_conflict_verdict === 'eligible' &&
    sync.conflicts.ai_resolution.allowed_type === 'modify_modify_utf8_text' &&
    sync.conflicts.ai_resolution.allowed_mode === '100644' &&
    sync.conflicts.ai_resolution.maximum_files === 4 &&
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
const syncSteps = syncWorkflow.jobs.sync.steps;
assert(
  syncWorkflow.jobs.sync.env.AERIS_AI_MODEL_CONFLICT_RESOLVER ===
      '${{ vars.AERIS_AI_MODEL_CONFLICT_RESOLVER || vars.AERIS_AI_MODEL_WRITER }}' &&
    syncWorkflow.jobs.sync.env.AERIS_AI_MODEL_CONFLICT_REVIEWER ===
      '${{ vars.AERIS_AI_MODEL_CONFLICT_REVIEWER || vars.AERIS_AI_MODEL_REVIEWER }}',
  'trusted conflict generation must bind the configured Resolver and Reviewer model IDs',
);
assert(syncWorkflow.jobs.sync.environment === 'writer', 'sync must use the shared writer environment');
assert(
  syncWorkflow.jobs.sync.if.includes("vars.AERIS_WRITER_ENABLED == 'true'") &&
    syncWorkflow.jobs.sync.if.includes("vars.AERIS_WRITER_ENABLED == '1'") &&
    syncWorkflow.jobs.sync.if.includes("vars.AERIS_UPSTREAM_SYNC_ENABLED == 'true'") &&
    syncWorkflow.jobs.sync.if.includes("vars.AERIS_UPSTREAM_SYNC_ENABLED == '1'") &&
    syncWorkflow.jobs.sync.if.includes("vars.AERIS_AGENTS_ENABLED == 'true'") &&
    syncWorkflow.jobs.sync.if.includes("vars.AERIS_AGENTS_ENABLED == '1'"),
  'sync must remain disabled unless both the shared Writer identity and upstream-sync lane are enabled',
);
assert(
  syncWorkflow.jobs.sync.permissions.contents === 'read' &&
    syncWorkflow.jobs.sync.permissions['pull-requests'] === 'read' &&
    syncWorkflow.jobs.sync.permissions.actions === 'write' &&
    syncWorkflow.jobs.sync.permissions.issues === 'write' &&
    syncWorkflow.jobs.sync.permissions.checks === 'read' &&
    syncWorkflow.jobs.sync.permissions.statuses === undefined,
  'sync GITHUB_TOKEN permissions must be limited to issue writes, check reads, and required dispatch',
);
const syncTokenStep = syncSteps.find((step) => step.name === 'Mint bounded Writer App token');
assert(syncTokenStep && syncTokenStep.uses.includes('create-github-app-token@'), 'sync must mint a bounded Writer App token');
assert(
  syncTokenStep['timeout-minutes'] === 2,
  'App token mint must remain within the reserved autonomy margin',
);
assert(
  syncTokenStep.with['permission-contents'] === 'write' &&
    syncTokenStep.with['permission-pull-requests'] === 'write' &&
    syncTokenStep.with['permission-administration'] === 'read' &&
    syncTokenStep.with['permission-issues'] === undefined &&
    syncTokenStep.with['permission-checks'] === undefined &&
    syncTokenStep.with['permission-statuses'] === undefined,
  'Writer App token permissions exceed or miss the approved minimum',
);

const conflictArtifactSuffix = '${{ github.run_id }}-${{ github.run_attempt }}';
const expectedPermissions = (actual, expected, message) => {
  assert(
    JSON.stringify(actual) === JSON.stringify(expected),
    message,
  );
};
const findStep = (job, name) => job.steps.find((step) => step.name === name);
const assertTrustedCheckouts = (job, message) => {
  const checkouts = job.steps.filter((step) => step.uses?.startsWith('actions/checkout@'));
  assert(
    checkouts.length > 0 && checkouts.every((step) => step.with?.['persist-credentials'] === false),
    message,
  );
};
const resolveConflictJob = syncWorkflow.jobs.resolve_conflict;
const publishConflictJob = syncWorkflow.jobs.publish_conflict;
const reviewConflictJob = syncWorkflow.jobs.review_conflict;
const finalizeConflictJob = syncWorkflow.jobs.finalize_conflict;
assert(
  resolveConflictJob && publishConflictJob && reviewConflictJob && finalizeConflictJob,
  'sync conflict workflow must keep all four isolated phases',
);
for (const [jobName, job] of Object.entries(syncWorkflow.jobs)) {
  assert(
    Object.values(job.env ?? {}).every(
      (value) => typeof value !== 'string' || !value.includes('${{ runner.'),
    ),
    `${jobName} job-level env must not use the unavailable runner context`,
  );
}
expectedPermissions(
  resolveConflictJob.permissions,
  { actions: 'read', contents: 'read' },
  'Resolver job permissions changed',
);
expectedPermissions(
  publishConflictJob.permissions,
  { actions: 'read', checks: 'read', contents: 'read', issues: 'write', 'pull-requests': 'read' },
  'conflict Publisher GITHUB_TOKEN permissions changed',
);
expectedPermissions(
  reviewConflictJob.permissions,
  { actions: 'read', checks: 'read', contents: 'read', 'pull-requests': 'read' },
  'Reviewer job permissions changed',
);
expectedPermissions(
  finalizeConflictJob.permissions,
  { actions: 'write', checks: 'read', contents: 'read', 'pull-requests': 'read' },
  'conflict Finalizer GITHUB_TOKEN permissions changed',
);
assert(
  publishConflictJob.environment === 'writer' &&
    finalizeConflictJob.environment === 'writer' &&
    publishConflictJob.concurrency.group === 'aeris-writer-mutation' &&
    finalizeConflictJob.concurrency.group === 'aeris-writer-mutation' &&
    publishConflictJob.concurrency['cancel-in-progress'] === false &&
    finalizeConflictJob.concurrency['cancel-in-progress'] === false,
  'only serialized Publisher and Finalizer jobs may use the writer Environment',
);
assert(
  resolveConflictJob.environment === 'agent' && reviewConflictJob.environment === 'agent',
  'Resolver and Reviewer must obtain only the model secret from the agent Environment',
);
for (const [job, message] of [
  [resolveConflictJob, 'Resolver checkout must not persist credentials'],
  [publishConflictJob, 'conflict Publisher checkout must not persist credentials'],
  [reviewConflictJob, 'Reviewer checkout must not persist credentials'],
  [finalizeConflictJob, 'conflict Finalizer checkout must not persist credentials'],
]) {
  assertTrustedCheckouts(job, message);
}
const resolverStep = findStep(resolveConflictJob, 'Generate credentialless resolution candidate');
const reviewerStep = findStep(reviewConflictJob, 'Run independent credentialless Reviewer');
assert(
  resolverStep?.env.GITHUB_TOKEN === '' && resolverStep.env.GH_TOKEN === '' &&
    resolverStep.env.AERIS_AI_API_KEY === '${{ secrets.AERIS_AI_API_KEY }}' &&
    resolverStep.run === 'node .github/automation/src/sync-conflict-review.mjs resolve',
  'Resolver model step must be credentialless and produce only a candidate artifact',
);
assert(
  reviewerStep?.env.GITHUB_TOKEN === '' && reviewerStep.env.GH_TOKEN === '' &&
    reviewerStep.env.AERIS_AI_API_KEY === '${{ secrets.AERIS_AI_API_KEY }}' &&
    reviewerStep.run === 'node .github/automation/src/sync-conflict-review.mjs review',
  'Reviewer model step must be credentialless and independent from publication',
);
const expectedArtifacts = [
  [syncWorkflow.jobs.sync, 'Upload exact conflict bundle', `sync-conflict-bundle-${conflictArtifactSuffix}`],
  [resolveConflictJob, 'Download exact conflict bundle', `sync-conflict-bundle-${conflictArtifactSuffix}`],
  [resolveConflictJob, 'Upload exact resolution candidate', `sync-conflict-candidate-${conflictArtifactSuffix}`],
  [publishConflictJob, 'Download exact conflict bundle', `sync-conflict-bundle-${conflictArtifactSuffix}`],
  [publishConflictJob, 'Download exact resolution candidate', `sync-conflict-candidate-${conflictArtifactSuffix}`],
  [reviewConflictJob, 'Download exact conflict bundle', `sync-conflict-bundle-${conflictArtifactSuffix}`],
  [reviewConflictJob, 'Download exact resolution candidate', `sync-conflict-candidate-${conflictArtifactSuffix}`],
  [reviewConflictJob, 'Upload exact independent review', `sync-conflict-review-${conflictArtifactSuffix}`],
  [finalizeConflictJob, 'Download exact conflict bundle', `sync-conflict-bundle-${conflictArtifactSuffix}`],
  [finalizeConflictJob, 'Download exact resolution candidate', `sync-conflict-candidate-${conflictArtifactSuffix}`],
  [finalizeConflictJob, 'Download exact independent review', `sync-conflict-review-${conflictArtifactSuffix}`],
  [finalizeConflictJob, 'Upload final conflict attestation', `sync-conflict-attestation-${conflictArtifactSuffix}`],
];
for (const [job, name, artifactName] of expectedArtifacts) {
  assert(findStep(job, name)?.with?.name === artifactName, `${name} must select the exact run-attempt artifact`);
}
const publishConflictToken = findStep(publishConflictJob, 'Mint bounded Writer App token');
const finalizeConflictToken = findStep(finalizeConflictJob, 'Mint bounded Writer App token');
assert(
  publishConflictToken?.with['permission-contents'] === 'write' &&
    publishConflictToken.with['permission-pull-requests'] === 'write' &&
    publishConflictToken.with['permission-checks'] === undefined &&
    publishConflictToken.with['permission-administration'] === undefined,
  'conflict Publisher App token must keep its minimal write scope',
);
assert(
  finalizeConflictToken?.with['permission-administration'] === 'read' &&
    finalizeConflictToken.with['permission-contents'] === 'write' &&
    finalizeConflictToken.with['permission-pull-requests'] === 'write' &&
    finalizeConflictToken.with['permission-checks'] === undefined,
  'conflict Finalizer App token must include only merge and governance proof permissions',
);
const conflictMergeStep = findStep(finalizeConflictJob, 'Perform one exact server-side squash');
assert(
  conflictMergeStep?.env.GH_TOKEN === '${{ steps.sync_token.outputs.token }}' &&
    conflictMergeStep.env.AERIS_CHECKS_GH_TOKEN === '${{ github.token }}' &&
    conflictMergeStep.env.SYNCED_SHA === '${{ needs.publish_conflict.outputs.head_sha }}' &&
    conflictMergeStep.env.EXPECTED_BASE_SHA === '${{ needs.publish_conflict.outputs.base_sha }}' &&
    conflictMergeStep.env.CONFLICT_ATTESTATION_SHA === '${{ steps.attest.outputs.conflict_attestation_sha }}' &&
    (conflictMergeStep.run.match(/manage-sync-automerge\.sh/g) || []).length === 1 &&
    conflictMergeStep.run.includes('conflict_ai_review'),
  'conflict Finalizer must perform one exact attested merge helper invocation',
);
assert(
  syncWorkflow.jobs.sync.env.AERIS_AUTONOMY_EXPIRES_AT ===
    '${{ vars.AERIS_AUTONOMY_EXPIRES_AT }}',
  'every sync phase must receive the bounded autonomy expiry',
);
assert(
  syncWorkflow.jobs.sync.env.AERIS_WRITER_APP_SLUG === '${{ vars.AERIS_WRITER_APP_SLUG }}',
  'sync must receive the shared Writer App slug for actor identity migration',
);
assert(
  syncWorkflow.jobs.sync.env.AERIS_AUTONOMY_MIN_REMAINING_SECONDS === 3600,
  'unguarded token actions must stop one hour before the autonomy expiry boundary',
);
const tokenStepIndex = syncSteps.indexOf(syncTokenStep);
const preMintExpiryStep = syncSteps[tokenStepIndex - 1];
assert(
  preMintExpiryStep?.name === 'Validate autonomy before token mint' &&
    typeof preMintExpiryStep.run === 'string' &&
    preMintExpiryStep.run.includes('AERIS_AUTONOMY_EXPIRES_AT') &&
    preMintExpiryStep.run.includes('AERIS_WRITER_APP_SLUG') &&
    preMintExpiryStep.run.includes(
      'now_epoch + AERIS_AUTONOMY_MIN_REMAINING_SECONDS',
    ),
  'sync must fail closed immediately before minting the App token',
);
const checkoutStep = syncSteps.find((step) => step.name === 'Check out fork default branch');
assert(checkoutStep?.with?.token === '${{ steps.sync_token.outputs.token }}', 'sync checkout must use the Writer App token');
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
const syncValidationStep = syncSteps.find(
  (step) => step.name === 'Validate checkpoint synchronization',
);
assert(
  syncValidationStep?.run.includes(
    'bash .github/workflows/scripts/tests/test-sync-upstream-git-auth.sh',
  ),
  'sync workflow must exercise the ephemeral Writer Git credential contract before publication',
);
assert(
  publishStep?.env.GH_TOKEN === '${{ steps.sync_token.outputs.token }}' &&
    publishStep.env.AERIS_WRITER_TOKEN === '${{ steps.sync_token.outputs.token }}',
  'sync publication must use the bounded Writer App token',
);
assert(
  checkoutStep.with['persist-credentials'] === false &&
    syncScript.includes('aeris_writer_git_push() {') &&
    syncScript.includes(
      ': "${AERIS_WRITER_TOKEN:?AERIS_WRITER_TOKEN is required for Writer Git publication}"',
    ) &&
    syncScript.includes('GIT_ASKPASS="${askpass}" GIT_ASKPASS_REQUIRE=force GIT_TERMINAL_PROMPT=0') &&
    syncScript.includes(
      'aeris_git_network -c credential.helper= -c http.https://github.com/.extraheader= "$@"',
    ) &&
    /aeris_writer_git_push push \\\r?\n\s+--force-with-lease=/.test(syncScript) &&
    syncScript.includes('"https://github.com/${GITHUB_REPOSITORY}.git"') &&
    !syncScript.includes('x-access-token@') &&
    !syncScript.includes('remote set-url'),
  'sync Git publication must inject the Writer token only through ephemeral askpass with an exact lease',
);
assert(
  publishStep?.env.AERIS_ISSUES_GH_TOKEN === '${{ github.token }}' &&
    syncScript.includes(': "${AERIS_ISSUES_GH_TOKEN:?AERIS_ISSUES_GH_TOKEN is required}"') &&
    syncScript.includes('aeris_issues_gh()') &&
    syncScript.includes('issue_bot_comments() {\n  aeris_issues_gh api') &&
    syncScript.includes('pr_bot_comments() {\n  aeris_gh api') &&
    syncScript.includes('pr_comment_once() {') &&
    syncScript.includes('GH_TOKEN="${AERIS_ISSUES_GH_TOKEN}" command gh "$@"') &&
    syncScript.includes('aeris_gh api \\\n      --method PATCH') &&
    syncScript.includes('aeris_gh api --method POST \\\n      "repos/${GITHUB_REPOSITORY}/issues/${number}/comments"'),
  'sync issue inventory uses the workflow token while pending-tip mutations use the Writer token',
);
const mergeStep = syncSteps.find((step) => step.name === 'Merge synchronization PR');
const disarmCallIndex = syncScript.search(/^disarm_tracked_pr$/m);
const rebuildLoopIndex = syncScript.search(/^for attempt in 1 2 3; do$/m);
assert(mergeStep, 'sync workflow must expose the direct synchronization merge step');
assert(
  mergeStep.if.includes("steps.sync.outputs.has_changes == 'true'") &&
    mergeStep.if.includes("steps.sync.outputs.autonomous_eligible == 'true'") &&
    mergeStep.if.includes("steps.sync.outputs.policy_verdict == 'eligible'"),
  'direct merge must require a published synchronization change',
);
assert(
  mergeStep.env.PR_URL === '${{ steps.sync.outputs.pr_url }}' &&
    mergeStep.env.SYNCED_SHA === '${{ steps.sync.outputs.synced_sha }}' &&
    mergeStep.env.EXPECTED_BASE_SHA === '${{ steps.sync.outputs.expected_base_sha }}' &&
    mergeStep.env.SYNCED_SOURCE === '${{ steps.sync.outputs.synced_source }}' &&
    mergeStep.env.SYNC_POLICY_VERDICT === '${{ steps.sync.outputs.policy_verdict }}' &&
    mergeStep.env.GH_TOKEN === '${{ steps.sync_token.outputs.token }}' &&
    mergeStep.env.AERIS_CHECKS_GH_TOKEN === '${{ github.token }}',
  'direct merge must bind the published PR URL and exact head SHA',
);
assert(
  typeof mergeStep.run === 'string' &&
    mergeStep.run.includes('manage-sync-automerge.sh') &&
    mergeStep.run.includes(' merge ') &&
    !mergeStep.run.includes(' arm '),
  'direct merge must use the managed helper merge action',
);
assert(
  !autoMergeScript.includes('--auto'),
  'sync merge helper must not create persistent native auto-merge requests',
);
assert(
  autoMergeScript.includes('api --method PUT') &&
    autoMergeScript.includes('pulls/${PR_NUMBER}/merge') &&
    autoMergeScript.includes('.merged == true') &&
    autoMergeScript.includes('test("^[0-9a-fA-F]{40}$")') &&
    autoMergeScript.includes('pulls/${PR_NUMBER}') &&
    autoMergeScript.includes('merged_by.login') &&
    autoMergeScript.includes('merge_commit_sha') &&
    autoMergeScript.includes('.merge_commit_sha != .base.sha') &&
    autoMergeScript.includes('commits/${merge_commit_sha}') &&
    autoMergeScript.includes('.sha == $merge_commit_sha') &&
    autoMergeScript.includes('.parents | type == "array" and length == 1') &&
    autoMergeScript.includes('requiresStrictStatusChecks') &&
    autoMergeScript.includes('isAdminEnforced') &&
    autoMergeScript.includes('bypassPullRequestAllowances') &&
    autoMergeScript.includes('rulesets(first:100,includeParents:true,targets:[BRANCH])') &&
    autoMergeScript.includes('GH_TOKEN="${AERIS_CHECKS_GH_TOKEN:?AERIS_CHECKS_GH_TOKEN is required}" command gh') &&
    autoMergeScript.includes('set +e') &&
    autoMergeScript.includes('readback_status'),
  'direct merge must use one REST merge and prove the exact post-merge outcome',
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
  syncScript.includes('WRITER_APP_BOT_LOGIN="${AERIS_WRITER_APP_SLUG}[bot]"') &&
    syncScript.includes("LEGACY_BOT_LOGIN='github-actions[bot]'") &&
    syncScript.includes(
      '.user.login == \\"${WRITER_APP_BOT_LOGIN}\\" or .user.login == \\"${LEGACY_BOT_LOGIN}\\"',
    ) &&
    syncScript.includes('is_sync_automation_login'),
  'comment and PR identity checks must accept the Writer App bot and migrate legacy Actions state',
);
assert(
  syncScript.includes('git rev-parse HEAD') &&
    syncScript.includes('Trusted checkout HEAD no longer equals the fetched base SHA'),
  'sync must bind the trusted checkout HEAD to the fetched base before classification or publication',
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
      syncWorkflow.jobs.sync.permissions.checks === 'read' &&
      checkDispatchStep?.env.GH_TOKEN === '${{ github.token }}' &&
      checkDispatchStep?.env.PR_URL === '${{ steps.sync.outputs.pr_url }}' &&
      checkDispatchStep?.env.SYNCED_SHA === '${{ steps.sync.outputs.synced_sha }}' &&
      checkDispatchStep?.env.EXPECTED_BASE_SHA === '${{ steps.sync.outputs.expected_base_sha }}' &&
      checkDispatchStep?.['timeout-minutes'] === 35,
    'fallback dispatch and bounded required-check wait must use the exact PR and head',
  );
  assert(
    checkDispatchStep?.run === 'bash .github/workflows/scripts/ensure-required-checks.sh' &&
      checkDispatchScript.includes('source "${SCRIPT_DIR}/github-autonomy.sh"') &&
      checkDispatchScript.includes('aeris_gh workflow run') &&
      checkDispatchScript.includes('wait_for_required_checks') &&
      checkDispatchScript.includes('Automation Policy / gate') &&
      checkDispatchScript.includes('Frontend CI / check') &&
      checkDispatchScript.includes('Rust CI / check') &&
      checkDispatchScript.includes('.app.id == 15368') &&
      checkDispatchScript.includes('.app.slug == "github-actions"') &&
      checkDispatchScript.includes('.headRefOid == $head_sha') &&
      checkDispatchScript.includes('.autoMergeRequest == null') &&
      !/(^|\n)\s*gh\s/.test(checkDispatchScript),
    'check discovery, dispatch, and exact-head success wait must revalidate expiry before every GitHub token use',
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
  'workflow validation must exercise Writer App comment identity migration',
);
assert(
  validationStep?.run.includes('test-sync-upstream-alerts.sh'),
  'workflow validation must exercise Writer App sync alert identity migration',
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
    (step) => step.run === 'bash ../workflows/scripts/tests/test-sync-upstream-git-auth.sh',
  ),
  'required CI must execute the ephemeral Writer Git credential integration test',
);
assert(
  frontendWorkflow.jobs.automation.steps.some(
    (step) => step.run === 'bash ../workflows/scripts/tests/test-sync-upstream-identity.sh',
  ),
  'required CI must execute the Writer App comment identity integration test',
);
assert(
  frontendWorkflow.jobs.automation.steps.some(
    (step) => step.run === 'bash ../workflows/scripts/tests/test-sync-upstream-alerts.sh',
  ),
  'required CI must execute the Writer App alert identity integration test',
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
