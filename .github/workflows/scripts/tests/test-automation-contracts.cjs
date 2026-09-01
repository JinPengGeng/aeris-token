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
const rustWorkflow = loadYaml('.github/workflows/rust-ci.yml');
const syncScript = read('.github/workflows/scripts/sync-upstream.sh');
const autoMergeScript = read('.github/workflows/scripts/manage-sync-automerge.sh');
const autonomyScript = read('.github/workflows/scripts/github-autonomy.sh');
const checkDispatchScript = read('.github/workflows/scripts/ensure-required-checks.sh');
const verifyCandidateScript = read('.github/workflows/scripts/verify-sync-candidate.sh');
const boundedFetchScript = read('.github/workflows/scripts/bounded-git-fetch.sh');
const prepareScript = read('.github/workflows/scripts/prepare-checkpoint-sync.sh');
const checkpointScript = read('.github/workflows/scripts/checkpoint-merge.sh');
const verifyCandidateMetadata = read(
  '.github/workflows/scripts/validate-sync-candidate-metadata.cjs',
);
const state = JSON.parse(read('.github/upstream-sync-state.json'));

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
    sync.conflicts.ai_resolution.profile === 'aeris-sync-conflict-v2' &&
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
const preflightJob = syncWorkflow.jobs.preflight;
assert(
  preflightJob,
  'sync workflow must isolate the script preflight in a dedicated job so publication starts on a clean runner',
);
const preflightSteps = preflightJob.steps;
const syncNeeds = syncWorkflow.jobs.sync.needs;
assert(
  Array.isArray(syncNeeds) ? syncNeeds.includes('preflight') : syncNeeds === 'preflight',
  'sync publication job must wait for the isolated preflight job',
);
assert(
  syncSteps.every((step) => step.name !== 'Validate checkpoint synchronization'),
  'script preflight checks must run on the preflight runner, not on the publication runner',
);
assert(
  preflightSteps.every((step) => !step.uses?.includes('create-github-app-token')),
  'preflight must not mint a Writer App token that cannot cross the job boundary',
);
assert(
  preflightJob.environment === undefined,
  'preflight must not hold the writer Environment secrets',
);
assert(
  JSON.stringify(preflightJob.permissions) === JSON.stringify({ contents: 'read' }),
  'preflight GITHUB_TOKEN must be limited to reading the trusted checkout',
);
const preflightCheckoutStep = preflightSteps.find(
  (step) => step.name === 'Check out fork default branch',
);
assert(
  preflightCheckoutStep?.with?.token === '${{ github.token }}' &&
    preflightCheckoutStep.with['persist-credentials'] === false &&
    preflightCheckoutStep.with['fetch-depth'] === 1,
  'preflight checkout must use the read-only workflow token without persisted credentials',
);
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

const publisherWorkflow = loadYaml('.github/workflows/autonomy-publisher.yml');
const publisherTokenStep = publisherWorkflow.jobs.publish.steps.find((step) => step.name === 'Mint bounded Writer App token');
assert(
  publisherTokenStep?.with['permission-checks'] === 'write' &&
    publisherTokenStep.with['permission-contents'] === 'write' &&
    publisherTokenStep.with['permission-pull-requests'] === 'write' &&
    publisherTokenStep.with['permission-administration'] === undefined,
  'candidate Publisher App token must be the only Writer token that requests checks: write',
);

const finalizerWorkflow = loadYaml('.github/workflows/autonomy-finalizer.yml');
const finalizerTokenStep = finalizerWorkflow.jobs.finalize.steps.find((step) => step.name === 'Mint bounded Writer App token');
assert(
  finalizerTokenStep?.with['permission-checks'] === undefined &&
    finalizerTokenStep.with['permission-administration'] === 'read' &&
    finalizerTokenStep.with['permission-contents'] === 'write' &&
    finalizerTokenStep.with['permission-pull-requests'] === 'write',
  'candidate Finalizer App token must not request checks: write',
);
const finalizerAttestationStep = finalizerWorkflow.jobs.finalize.steps.find(
  (step) => step.name === 'Attest Writer App and installation identity',
);
const finalizerMergeStep = finalizerWorkflow.jobs.finalize.steps.find(
  (step) => step.name === 'Directly squash merge exact eligible pull request',
);
assert(
  finalizerAttestationStep?.env.AERIS_WRITER_APP_NODE_ID === '${{ vars.AERIS_WRITER_APP_NODE_ID }}' &&
    finalizerAttestationStep.env.AERIS_WRITER_APP_OWNER_DATABASE_ID ===
      '${{ vars.AERIS_WRITER_APP_OWNER_DATABASE_ID }}' &&
    finalizerMergeStep?.env.AERIS_WRITER_PROOF_APP_ID ===
      '${{ steps.writer_app_attestation.outputs.app_id }}' &&
    finalizerMergeStep.env.AERIS_WRITER_PROOF_APP_SLUG ===
      '${{ steps.writer_app_attestation.outputs.app_slug }}' &&
    finalizerMergeStep.env.AERIS_WRITER_PROOF_APP_NODE_ID ===
      '${{ steps.writer_app_attestation.outputs.app_node_id }}' &&
    finalizerMergeStep.env.AERIS_WRITER_PROOF_APP_OWNER_DATABASE_ID ===
      '${{ steps.writer_app_attestation.outputs.app_owner_database_id }}',
  'candidate Finalizer must bind live Writer App and owner identity into full proof',
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
  [preflightJob, 'preflight checkout must not persist credentials'],
  [resolveConflictJob, 'Resolver checkout must not persist credentials'],
  [publishConflictJob, 'conflict Publisher checkout must not persist credentials'],
  [reviewConflictJob, 'Reviewer checkout must not persist credentials'],
  [finalizeConflictJob, 'conflict Finalizer checkout must not persist credentials'],
]) {
  assertTrustedCheckouts(job, message);
}
const resolverStep = findStep(resolveConflictJob, 'Generate credentialless resolution candidate');
const reviewerStep = findStep(reviewConflictJob, 'Run independent credentialless Reviewer');
const collectReviewStep = findStep(reviewConflictJob, 'Collect exact published review input');
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
assert(
  collectReviewStep?.id === 'collect' &&
    reviewConflictJob.outputs.input_sha === '${{ steps.collect.outputs.conflict_review_input_sha }}' &&
    reviewConflictJob.outputs.receipt_sha === '${{ steps.review.outputs.conflict_review_receipt_sha }}',
  'Reviewer must expose exact input and receipt hashes to the Finalizer',
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
    conflictMergeStep.env.AERIS_CONFLICT_BUNDLE_PATH === '${{ runner.temp }}/aeris-sync-conflict/bundle.json' &&
    conflictMergeStep.env.AERIS_CONFLICT_CANDIDATE_PATH === '${{ runner.temp }}/aeris-sync-resolution/candidate.json' &&
    conflictMergeStep.env.AERIS_CONFLICT_REVIEW_INPUT_PATH === '${{ runner.temp }}/aeris-sync-review/input.json' &&
    conflictMergeStep.env.AERIS_CONFLICT_REVIEW_RECEIPT_PATH === '${{ runner.temp }}/aeris-sync-review/receipt.json' &&
    conflictMergeStep.env.AERIS_CONFLICT_BUNDLE_SHA === '${{ needs.publish_conflict.outputs.bundle_sha }}' &&
    conflictMergeStep.env.AERIS_CONFLICT_CANDIDATE_SHA === '${{ needs.publish_conflict.outputs.candidate_sha }}' &&
    conflictMergeStep.env.AERIS_CONFLICT_REVIEW_INPUT_SHA === '${{ needs.review_conflict.outputs.input_sha }}' &&
    conflictMergeStep.env.AERIS_CONFLICT_REVIEW_RECEIPT_SHA === '${{ needs.review_conflict.outputs.receipt_sha }}' &&
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
  checkoutStep?.with?.['fetch-depth'] === 1,
  'bootstrap checkout must not perform an unbounded full-history fetch',
);
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
const nodeSetupStep = syncSteps.find((step) => step.name === 'Set up Node.js 22');
assert(
  nodeSetupStep?.uses ===
      'actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020' &&
    nodeSetupStep?.with?.['node-version'] === '22' &&
    syncSteps.indexOf(nodeSetupStep) === checkoutStepIndex + 1,
  'sync must pin Node.js 22 immediately after the bounded checkout',
);
const publishStep = syncSteps.find(
  (step) => step.name === 'Build and publish automation branch',
);
const syncValidationStep = preflightSteps.find(
  (step) => step.name === 'Validate checkpoint synchronization',
);
assert(
  publishStep?.['timeout-minutes'] === 15,
  'sync publication must have a hard workflow-step deadline',
);
assert(
  publishStep?.env.AERIS_BOUNDED_BOOTSTRAP_SHALLOW === 'true',
  'sync must explicitly convert the one-commit bootstrap before bounded exact-ref fetches',
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
    syncScript.includes('aeris_bounded_issues_gh()') &&
    syncScript.includes('issue_bot_comments() {') &&
    syncScript.includes('pr_bot_comments() {') &&
    syncScript.includes('pr_comment_once() {') &&
    syncScript.includes('GH_TOKEN="${AERIS_ISSUES_GH_TOKEN}" command gh "$@"') &&
    syncScript.includes('GH_TOKEN="${AERIS_ISSUES_GH_TOKEN}" aeris_bounded_run_deadline') &&
    syncScript.includes('aeris_bounded_gh api \\\n      --method PATCH') &&
    syncScript.includes('aeris_bounded_gh api --method POST \\\n      "repos/${GITHUB_REPOSITORY}/issues/${number}/comments"'),
  'sync issue inventory uses the workflow token while pending-tip mutations use the bounded Writer token',
);
const verifyCandidateStep = syncSteps.find(
  (step) => step.name === 'Verify managed synchronization candidate',
);
const mergeStep = syncSteps.find((step) => step.name === 'Merge synchronization PR');
const disarmCallIndex = syncScript.search(/^disarm_tracked_pr$/m);
const rebuildLoopIndex = syncScript.search(/^for attempt in 1 2 3; do$/m);
const producerBoundIndex = syncScript.indexOf(
  'if ! aeris_enforce_change_bounds "${checkpoint_sha}" "${upstream_sha}"',
);
const producerPrepareIndex = syncScript.indexOf(
  'prepare_output="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}"',
);
assert(verifyCandidateStep, 'sync workflow must verify the published candidate tree');
assert(
  verifyCandidateStep.env.GH_TOKEN === undefined &&
    verifyCandidateStep.env.PR_URL === '${{ steps.sync.outputs.pr_url }}' &&
    verifyCandidateStep.env.SYNCED_SHA === '${{ steps.sync.outputs.synced_sha }}' &&
    verifyCandidateStep.run.includes('verify-sync-candidate.sh'),
  'candidate verification must bind the App-authored PR URL and exact published head',
);
assert(
  verifyCandidateStep['timeout-minutes'] === 10,
  'candidate verification must have a hard workflow runtime bound',
);
assert(
  verifyCandidateScript.includes('"${expected_tree}" == "${actual_tree}"') &&
    verifyCandidateScript.includes('git merge-base --is-ancestor "${checkpoint}" "${upstream_tip}"') &&
    verifyCandidateScript.includes('"${upstream_tip}" == "${upstream_current}"') &&
    !verifyCandidateScript.includes('--paginate'),
  'candidate verification must regenerate the exact tree without trusting paginated metadata',
);
assert(
  verifyCandidateScript.includes('second_parent="${parents[2]:-') &&
    verifyCandidateScript.includes('"${parent_count}" "${actual_parent}" "${second_parent}"') &&
    verifyCandidateMetadata.includes("parentCount !== '2'") &&
    verifyCandidateMetadata.includes('secondParent !== upstreamTip'),
  'candidate verification must require the advertised upstream tip as the second parent',
);
assert(
  syncScript.includes('aeris_bounded_fetch_ref') &&
    verifyCandidateScript.includes('aeris_bounded_fetch_ref') &&
    syncScript.includes('AERIS_BOUNDED_FETCH_CREDENTIALLESS=true') &&
    boundedFetchScript.includes('fetch.fsckObjects=true') &&
    boundedFetchScript.includes('fetch.unpackLimit=0') &&
    boundedFetchScript.includes('ulimit -f "${file_blocks}"') &&
    boundedFetchScript.includes('ulimit -v "${memory_kib}"') &&
    boundedFetchScript.includes('fsck --strict') &&
    boundedFetchScript.includes('--no-tags --no-recurse-submodules --refmap=') &&
    boundedFetchScript.includes('aeris_enforce_change_bounds') &&
    boundedFetchScript.includes('export GIT_NO_LAZY_FETCH=1') &&
    !boundedFetchScript.includes('>"${stage}/objects/info/alternates"'),
  'producer and verifier must share the exact-ref bounded and fsck-verified fetch path',
);
assert(
  syncScript.includes('GITHUB_API_MAX_PAGES=10') &&
    syncScript.includes('aeris_read_bounded_api_array_pages') &&
    syncScript.includes('aeris_bounded_gh') &&
    !syncScript.includes('--paginate') &&
    verifyCandidateScript.includes(
      'aeris_bounded_run "${MAX_PR_BYTES}" curl -q',
    ),
  'GitHub pagination and public metadata transport must remain page, time, memory, and file bounded',
);
const deadlineRunnerMatch = boundedFetchScript.match(
  /aeris_bounded_run_deadline\(\) \{[\s\S]*?\n\}/,
);
assert(
  deadlineRunnerMatch &&
    !deadlineRunnerMatch[0].includes('ulimit -v') &&
    deadlineRunnerMatch[0].includes('ulimit -f "${file_blocks}"') &&
    deadlineRunnerMatch[0].includes('timeout -k 5s') &&
    syncScript.includes('aeris_bounded_run_deadline "${GITHUB_API_PAGE_BYTES}" gh "$@"') &&
    syncScript.includes('aeris_bounded_run_deadline 2097152 gh api'),
  'Go-based gh calls must use the deadline runner, which keeps the timeout and file bound without a virtual-memory ceiling',
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
  prepareScript.includes('source "${BOUNDED_FETCH_HELPER}"') &&
    prepareScript.includes('bounded_tree_git diff') &&
    prepareScript.includes('bounded_tree_git read-tree') &&
    prepareScript.includes('bounded_tree_git write-tree') &&
    prepareScript.includes('bounded_tree_git commit-tree') &&
    checkpointScript.includes('source "${BOUNDED_FETCH_HELPER}"') &&
    checkpointScript.includes('bounded_tree_git merge-tree') &&
    syncScript.includes('aeris_writer_git_push push'),
  'all untrusted-tree preparation stages must use the fail-closed runner and publication must use the Writer credential channel',
);
assert(
  boundedFetchScript.includes('git rev-parse --absolute-git-dir') &&
    boundedFetchScript.includes('git rev-parse --git-common-dir') &&
    boundedFetchScript.includes('git worktree list --porcelain') &&
    boundedFetchScript.includes('repositories with linked worktrees are forbidden'),
  'bootstrap conversion must reject linked or shared worktrees before object deletion',
);
assert(
  syncScript.includes('aeris_assert_publication_refs_exact') &&
    syncScript.includes('"${base_sha}" "${upstream_sha}" "${published_sha}"') &&
    verifyCandidateScript.includes('AERIS_VERIFY_BEFORE_FINAL_FENCE_HOOK') &&
    verifyCandidateScript.includes("fail 'upstream branch drifted during verification'"),
  'producer and verifier must re-fence exact refs immediately before successful completion',
);
assert(
  syncScript.includes('git commit-tree "${prepared_tree}"') &&
    syncScript.includes('-p "${base_sha}"') &&
    syncScript.includes('-p "${upstream_sha}"') &&
    syncScript.includes('git reset --hard "${local_sha}"') &&
    !syncScript.includes('git commit "${commit_arguments[@]}"'),
  'sync commits must be dual-parent commit-tree snapshots linked to the exact upstream tip',
);
assert(
  producerBoundIndex >= 0 &&
    producerPrepareIndex >= 0 &&
    producerBoundIndex < producerPrepareIndex,
  'producer must bound the untrusted upstream tree before candidate preparation',
);
assert(
  !verifyCandidateScript.includes('GH_TOKEN') &&
    verifyCandidateScript.includes("--max-filesize \"${MAX_PR_BYTES}\"") &&
    boundedFetchScript.includes('-c credential.helper=') &&
    boundedFetchScript.includes('-c http.https://github.com/.extraheader='),
  'candidate verification reads must be credentialless and resource-bounded',
);
assert(
  verifyCandidateMetadata.includes('`${syncAppSlug}[bot]`') &&
    verifyCandidateMetadata.includes("pr.user.type !== expectedAuthorType") &&
    !verifyCandidateMetadata.includes("'github-actions[bot]'") &&
    !verifyCandidateMetadata.includes("'app/github-actions'"),
  'managed sync PR identity must be the exact configured Writer App bot without legacy fallback',
);
assert(mergeStep, 'sync workflow must expose the direct synchronization merge step');
assert(
  mergeStep.if.includes("steps.sync.outputs.has_changes == 'true'") &&
    mergeStep.if.includes("steps.verify.outputs.verified == 'true'") &&
    mergeStep.if.includes("steps.sync.outputs.autonomous_eligible == 'true'") &&
    mergeStep.if.includes("steps.sync.outputs.policy_verdict == 'eligible'"),
  'direct merge must require a verified published synchronization change',
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
    autoMergeScript.includes('-f merge_method=merge') &&
    !autoMergeScript.includes('merge_method=squash') &&
    autoMergeScript.includes('commit_title=chore: sync ') &&
    autoMergeScript.includes('Sync-Upstream-Checkpoint: %s->%s') &&
    autoMergeScript.includes('.merged == true') &&
    autoMergeScript.includes('test("^[0-9a-fA-F]{40}$")') &&
    autoMergeScript.includes('pulls/${PR_NUMBER}') &&
    autoMergeScript.includes('merged_by.login') &&
    autoMergeScript.includes('merge_commit_sha') &&
    autoMergeScript.includes('.merge_commit_sha != .base.sha') &&
    autoMergeScript.includes('commits/${merge_commit_sha}') &&
    autoMergeScript.includes('.sha == $merge_commit_sha') &&
    autoMergeScript.includes('.parents | type == "array" and length == 2 and') &&
    autoMergeScript.includes('.[0].sha == $base_sha and .[1].sha == $head_sha') &&
    autoMergeScript.includes('Sync-Upstream-Checkpoint: " + $checkpoint + "->" + $upstream_sha') &&
    autoMergeScript.includes('$repository_profile.mergeCommitAllowed == true') &&
    autoMergeScript.includes('rulesets(first:100,includeParents:true,targets:[BRANCH])') &&
    autoMergeScript.includes('REQUIRED_LINEAR_HISTORY') &&
    autoMergeScript.includes('strictRequiredStatusChecksPolicy') &&
    autoMergeScript.includes('repos/${REPOSITORY}/rulesets/21984329') &&
    autoMergeScript.includes('.actor_id == 4667256') &&
    autoMergeScript.includes('.bypass_mode == "always"') &&
    autoMergeScript.includes('GH_TOKEN="${AERIS_CHECKS_GH_TOKEN:?AERIS_CHECKS_GH_TOKEN is required}" command gh') &&
    autoMergeScript.includes('set +e') &&
    autoMergeScript.includes('readback_status'),
  'direct merge must use one REST true-merge and prove the exact dual-parent post-merge outcome',
);
assert(
  autoMergeScript.includes('repos/${REPOSITORY}/pulls/${PR_NUMBER}') &&
    autoMergeScript.includes('.auto_merge == null') &&
    autoMergeScript.includes('Sync-Upstream-Policy-Verdict') &&
    autoMergeScript.includes('.head.sha == $head_sha') &&
    autoMergeScript.includes('.base.sha == $base_sha') &&
    autoMergeScript.includes("fail 'pull request drifted before merge mutation'"),
  'direct merge must revalidate the exact managed PR snapshot before mutation',
);
assert(
  autoMergeScript.includes(
    '.parents | type == "array" and length == 2 and .[0].sha == $base_sha and .[1].sha == $upstream_sha',
  ),
  'direct merge must require the exact dual-parent sync commit shape',
);
assert(
  autoMergeScript.includes('sync_checkpoint=') &&
    autoMergeScript.includes('endswith("->" + $upstream_sha)'),
  'direct merge must extract the exact checkpoint trailer from the verified head commit',
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
    syncScript.includes('.user.login == $sync or .user.login == $legacy') &&
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
for (const [name, script] of [
  ['sync-upstream.sh', syncScript],
  ['verify-sync-candidate.sh', verifyCandidateScript],
]) {
  assert(
    !/execFileSync\(\s*['"]git['"]/.test(script),
    `${name} must not spawn an unbounded Git history read from Node`,
  );
  assert(
    !/(^|\n)\s*git\s+(show|rev-parse|rev-list|cat-file|ls-tree|verify-pack)\b/.test(script),
    `${name} history and tree reads must use bounded_tree_git`,
  );
}
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
const validationStep = preflightSteps.find(
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
  validationStep?.run.includes('test-bounded-git-fetch.sh'),
  'workflow validation must exercise bounded Git transport failure fixtures',
);
assert(
  !validationStep?.run.includes('test-verify-sync-candidate.sh'),
  'candidate verification fixtures run only in required PR CI, not in the sync job preflight',
);
assert(
  frontendWorkflow.jobs.automation.steps.some(
    (step) => step.run === 'bash ../workflows/scripts/tests/test-github-autonomy.sh',
  ),
  'required CI must execute the fake-clock autonomy integration test',
);
assert(
  frontendWorkflow.jobs.automation.steps.some(
    (step) => step.run === 'bash ../workflows/scripts/tests/test-verify-sync-candidate.sh',
  ),
  'required CI must execute candidate tamper and replay fixtures',
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
