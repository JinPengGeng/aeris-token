import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  AutonomyFinalizerError,
  AutonomyFinalizerGitHubClient,
  evaluateAutonomyFinalizer,
  evaluateAutonomyFinalizerPreliminary,
  finalizeAutonomyPull,
  runAutonomyFinalizer,
  validateResponseLossCanaryBinding,
} from '../src/autonomy-finalizer.mjs';
import {
  createWriterPublisherCheckRun,
  createWriterPublisherTarget,
  serializeWriterPublisherTarget,
} from '../src/autonomy-publisher-attestation.mjs';

const REPOSITORY = 'JinPengGeng/aeris-token';
const REPOSITORY_ID = 1316750512;
const BASE_SHA = 'a'.repeat(40);
const HEAD_SHA = 'b'.repeat(40);
const HEAD_TREE = 'c'.repeat(40);
const BASE_TREE = 'd'.repeat(40);
const DOCS_TREE = 'e'.repeat(40);
const CANARY_TREE = 'f'.repeat(40);
const MERGE_SHA = '9'.repeat(40);
const ISSUE_NUMBER = 123;
const PULL_NUMBER = 17;
const CANDIDATE_RUN_ID = 456;
const PUBLISHER_RUN_ID = 457;
const WRITER_BOT = Object.freeze({
  login: 'aeris-writer[bot]',
  graphqlLogin: 'aeris-writer',
  databaseId: 319277066,
  nodeId: 'BOT_writer_node',
});

const trigger = Object.freeze({ run_id: 77, run_attempt: 1 });
const trust = Object.freeze({
  repository: REPOSITORY,
  repository_id: REPOSITORY_ID,
  default_branch: 'main',
  policy_ref: 'main',
  policy_sha: BASE_SHA,
});
const writerTrust = Object.freeze({
  app_id: 4667256,
  proof_app_id: 4667256,
  proof_app_slug: 'aeris-writer',
  proof_app_node_id: 'APP_writer_node',
  proof_app_owner_login: 'JinPengGeng',
  proof_app_owner_database_id: 1234567,
  proof_app_owner_type: 'User',
  proof_app_permissions: { administration: 'read', checks: 'write', contents: 'write', metadata: 'read', pull_requests: 'write' },
  installation_id: 155342531,
  proof_installation_id: 155342531,
  proof_installation_account_login: 'JinPengGeng',
  proof_installation_account_type: 'User',
  proof_installation_permissions: { administration: 'read', checks: 'write', contents: 'write', metadata: 'read', pull_requests: 'write' },
  proof_repository_selection: 'selected',
  token_installation_id: 155342531,
  token_app_slug: 'aeris-writer',
  app_slug: 'aeris-writer',
  governance_fence_ruleset_id: 101,
  governance_fence_updated_at: '2026-08-27T18:48:17.800+08:00',
});
const configValue = {
  repository: REPOSITORY,
  base_ref: 'main',
  writer_login: WRITER_BOT.login,
  branch_prefix: 'agent/issue-',
  maximum_files: 20,
  maximum_changes: 2000,
};
Object.defineProperty(configValue, 'writer_trust', { value: writerTrust, enumerable: false });
const config = Object.freeze(configValue);
const CANDIDATE_EXECUTOR = Object.freeze({
  id: 'codex-action-v1',
  protocol: 'aeris-workspace-candidate-v1',
  kind: 'workspace_candidate',
  action_sha: '1'.repeat(40),
  tool_version: '0.148.0',
});

function candidateExecutorRegistry() {
  return {
    schema_version: 1,
    executors: [
      { id: 'openai-chat-v1', kind: 'completion', protocol: 'openai-chat-completions-v1' },
      { id: 'openai-responses-v1', kind: 'completion', protocol: 'openai-responses-v1' },
      CANDIDATE_EXECUTOR,
    ],
    routes: {
      agent_analysis: 'openai-chat-v1',
      sync_conflict_resolver: 'openai-chat-v1',
      sync_conflict_reviewer: 'openai-chat-v1',
      candidate: CANDIDATE_EXECUTOR.id,
    },
  };
}

function candidateExecutorRegistryFile(registry = candidateExecutorRegistry()) {
  const content = Buffer.from(JSON.stringify(registry), 'utf8');
  return {
    type: 'file',
    encoding: 'base64',
    size: content.length,
    content: content.toString('base64'),
  };
}

function candidateCommitMessage(executor = CANDIDATE_EXECUTOR) {
  return [
    'chore(autonomy): update issue #123',
    '',
    `Aeris-Autonomy-Executor-ID: ${executor.id}`,
    `Aeris-Autonomy-Executor-Protocol: ${executor.protocol}`,
    `Aeris-Autonomy-Executor-Action-SHA: ${executor.action_sha}`,
    `Aeris-Autonomy-Executor-Tool-Version: ${executor.tool_version}`,
    `Aeris-Autonomy-Patch: ${'c'.repeat(64)}`,
  ].join('\n');
}

function preliminaryEnvironment(overrides = {}) {
  return {
    GITHUB_REPOSITORY: REPOSITORY,
    GITHUB_REPOSITORY_ID: String(REPOSITORY_ID),
    GITHUB_TOKEN: 'actions-token',
    AERIS_TRIGGER_RUN_ID: String(trigger.run_id),
    AERIS_TRIGGER_RUN_ATTEMPT: String(trigger.run_attempt),
    AERIS_DEFAULT_BRANCH: 'main',
    AERIS_POLICY_REF: 'main',
    AERIS_POLICY_SHA: BASE_SHA,
    AERIS_WRITER_ENABLED: 'true',
    AERIS_WRITER_APP_ID: String(writerTrust.app_id),
    AERIS_WRITER_APP_SLUG: writerTrust.app_slug,
    AERIS_FINALIZER_PROOF_LEVEL: 'preliminary',
    ...overrides,
  };
}

function fullEnvironment(overrides = {}) {
  return preliminaryEnvironment({
    AERIS_WRITER_TOKEN: 'writer-token',
    AERIS_WRITER_APP_ID: String(writerTrust.app_id),
    AERIS_WRITER_PROOF_APP_ID: String(writerTrust.proof_app_id),
    AERIS_WRITER_PROOF_APP_SLUG: writerTrust.proof_app_slug,
    AERIS_WRITER_PROOF_APP_NODE_ID: writerTrust.proof_app_node_id,
    AERIS_WRITER_PROOF_APP_OWNER_LOGIN: writerTrust.proof_app_owner_login,
    AERIS_WRITER_PROOF_APP_OWNER_DATABASE_ID: String(writerTrust.proof_app_owner_database_id),
    AERIS_WRITER_PROOF_APP_OWNER_TYPE: writerTrust.proof_app_owner_type,
    AERIS_WRITER_PROOF_APP_PERMISSIONS: JSON.stringify(writerTrust.proof_app_permissions),
    AERIS_WRITER_INSTALLATION_ID: String(writerTrust.installation_id),
    AERIS_WRITER_PROOF_INSTALLATION_ID: String(writerTrust.proof_installation_id),
    AERIS_WRITER_PROOF_INSTALLATION_ACCOUNT_LOGIN: writerTrust.proof_installation_account_login,
    AERIS_WRITER_PROOF_INSTALLATION_ACCOUNT_TYPE: writerTrust.proof_installation_account_type,
    AERIS_WRITER_PROOF_INSTALLATION_PERMISSIONS: JSON.stringify(writerTrust.proof_installation_permissions),
    AERIS_WRITER_PROOF_REPOSITORY_SELECTION: writerTrust.proof_repository_selection,
    AERIS_WRITER_TOKEN_INSTALLATION_ID: String(writerTrust.token_installation_id),
    AERIS_WRITER_TOKEN_APP_SLUG: writerTrust.app_slug,
    AERIS_WRITER_GOVERNANCE_FENCE_RULESET_ID: String(writerTrust.governance_fence_ruleset_id),
    AERIS_WRITER_GOVERNANCE_FENCE_UPDATED_AT: writerTrust.governance_fence_updated_at,
    AERIS_FINALIZER_PROOF_LEVEL: 'full',
    AERIS_FINALIZER_MUTATE: 'true',
    ...overrides,
  });
}

function pull() {
  return {
    number: PULL_NUMBER,
    state: 'open',
    user: { login: config.writer_login },
    head: { ref: `agent/issue-${ISSUE_NUMBER}`, sha: HEAD_SHA, repo: { full_name: REPOSITORY } },
    base: { ref: 'main', sha: BASE_SHA, repo: { full_name: REPOSITORY } },
  };
}

function governance(overrides = {}) {
  return {
    id: 'PR_node', number: PULL_NUMBER, state: 'OPEN', isDraft: true,
    body: `<!-- aeris-autonomy-managed -->\n<!-- aeris-autonomy-task:issue:${ISSUE_NUMBER} -->`,
    headRefName: `agent/issue-${ISSUE_NUMBER}`, headRefOid: HEAD_SHA, headRepository: REPOSITORY,
    baseRefName: 'main', baseRefOid: BASE_SHA,
    authorType: 'Bot', author: WRITER_BOT.graphqlLogin,
    authorId: WRITER_BOT.nodeId, authorDatabaseId: WRITER_BOT.databaseId,
    mergeable: 'MERGEABLE', mergeStateStatus: 'DRAFT', reviewDecision: null,
    merged: false, mergedAt: null, mergedBy: null, mergeCommit: null,
    autoMergeRequest: null, labels: [], reviewThreads: [], lifecycle: [],
    ...overrides,
  };
}

function mergedGovernance(overrides = {}) {
  return governance({
    state: 'MERGED', isDraft: false, mergeable: 'UNKNOWN', mergeStateStatus: 'UNKNOWN',
    baseRefOid: MERGE_SHA, merged: true, mergedAt: '2026-08-21T00:00:00Z',
    mergedBy: {
      type: 'Bot', login: WRITER_BOT.graphqlLogin,
      id: WRITER_BOT.nodeId, databaseId: WRITER_BOT.databaseId,
    },
    mergeCommit: { oid: MERGE_SHA, parentCount: 1, parents: [{ oid: BASE_SHA }] },
    lifecycle: [{ id: 91, event: 'closed' }],
    ...overrides,
  });
}

const REQUIRED_PROTECTION_CONTEXTS = Object.freeze([
  'Rust CI / check',
  'Frontend CI / check',
  'Automation Policy / gate',
]);

function completeConnection(nodes = []) {
  return { nodes, totalCount: nodes.length, pageInfo: { hasNextPage: false, endCursor: null } };
}

function protectionRule(overrides = {}) {
  return {
    pattern: 'main',
    allowsDeletions: false,
    allowsForcePushes: false,
    blocksCreations: false,
    dismissesStaleReviews: true,
    requiresStatusChecks: true,
    requiresStrictStatusChecks: true,
    isAdminEnforced: true,
    lockAllowsFetchAndMerge: false,
    lockBranch: false,
    requireLastPushApproval: false,
    requiredApprovingReviewCount: 0,
    requiredDeploymentEnvironments: [],
    requiresApprovingReviews: true,
    requiresCodeOwnerReviews: false,
    requiresCommitSignatures: false,
    requiresConversationResolution: true,
    requiresDeployments: false,
    requiresLinearHistory: true,
    restrictsPushes: false,
    restrictsReviewDismissals: false,
    bypassPullRequestAllowances: completeConnection(),
    bypassForcePushAllowances: completeConnection(),
    pushAllowances: completeConnection(),
    reviewDismissalAllowances: completeConnection(),
    requiredStatusChecks: REQUIRED_PROTECTION_CONTEXTS.map((context) => ({
      context, app: { databaseId: 15368, slug: 'github-actions' },
    })),
    ...overrides,
  };
}

function protection(overrides = {}) {
  return {
    autoMergeAllowed: true,
    mergeCommitAllowed: false,
    rebaseMergeAllowed: false,
    squashMergeAllowed: true,
    isArchived: false,
    isDisabled: false,
    isLocked: false,
    branchProtectionRules: completeConnection([protectionRule(overrides)]),
    rulesets: completeConnection([{
      id: 'RRS_fence', databaseId: 101, name: 'agent-head-fence-v1',
      enforcement: 'ACTIVE', target: 'BRANCH',
    }]),
  };
}

function writerGovernanceSnapshot() {
  return {
    governance_fence: {
      repository: { id: REPOSITORY_ID, full_name: REPOSITORY },
      direct_collaborators: {
        affiliation: 'direct', truncated: false,
        items: [{
          login: writerTrust.proof_app_owner_login,
          database_id: writerTrust.proof_app_owner_database_id,
          type: 'User', permission: 'ADMIN',
        }],
      },
      rulesets: {
        includes_parents: true, truncated: false,
        items: [{
          id: 101, name: 'agent-head-fence-v1', target: 'branch', enforcement: 'active',
          conditions: { ref_name: { include: ['refs/heads/agent/**'], exclude: [] } },
          rules: [
            { type: 'creation' }, { type: 'deletion' },
            { type: 'non_fast_forward' }, { type: 'update' },
          ],
          bypass_actors: [{ actor_id: writerTrust.app_id, actor_type: 'Integration', bypass_mode: 'always' }],
        }],
      },
    },
    secret_lane: {
      actions_permissions: { enabled: true, allowed_actions: 'selected', sha_pinning_required: true },
      workflow_permissions: {
        default_workflow_permissions: 'read', can_approve_pull_request_reviews: false,
      },
      environment: {
        name: 'writer', custom_branch_policies: true, protected_branches: false,
        can_admins_bypass_secrets_and_variables: false,
      },
      deployment_branch_policies: {
        environment_name: 'writer', truncated: false,
        items: [{ name: 'main', type: 'branch' }],
      },
    },
  };
}

function copiedWriterGovernanceSnapshot(mutator = () => {}) {
  const value = structuredClone(writerGovernanceSnapshot());
  mutator(value);
  return value;
}

const CHECK_IDENTITIES = Object.freeze(new Map([
  ['Rust CI / check', { run_id: 78, suite_id: 178, workflow_name: 'Rust CI', workflow_path: '.github/workflows/rust-ci.yml' }],
  ['Frontend CI / check', { run_id: 77, suite_id: 177, workflow_name: 'Frontend CI', workflow_path: '.github/workflows/frontend-ci.yml' }],
  ['Automation Policy / gate', { run_id: 79, suite_id: 179, workflow_name: 'Automation Policy', workflow_path: '.github/workflows/automation-policy.yml' }],
]));

function check(name, id, overrides = {}) {
  const identity = CHECK_IDENTITIES.get(name);
  const runId = overrides.run_id ?? identity.run_id;
  const suiteId = overrides.suite_id ?? identity.suite_id;
  return {
    id, name, head_sha: HEAD_SHA, status: 'completed', conclusion: 'success',
    details_url: `https://github.com/${REPOSITORY}/actions/runs/${runId}/job/${id}`,
    check_suite: { id: suiteId }, app: { id: 15368, slug: 'github-actions' },
    ...overrides,
  };
}

function workflowRun(name, overrides = {}) {
  const identity = CHECK_IDENTITIES.get(name);
  return {
    id: identity.run_id, run_attempt: 1, check_suite_id: identity.suite_id,
    event: 'pull_request', status: 'completed', conclusion: 'success',
    name: identity.workflow_name, path: identity.workflow_path, head_sha: HEAD_SHA,
    repository: { id: REPOSITORY_ID, full_name: REPOSITORY },
    head_repository: { id: REPOSITORY_ID, full_name: REPOSITORY },
    pull_requests: [{ number: PULL_NUMBER }],
    ...overrides,
  };
}

function writerPublicationAttestation(overrides = {}) {
  const source = overrides.attestation ?? {};
  const attestation = {
    schema_version: 1,
    repository: REPOSITORY,
    repository_id: REPOSITORY_ID,
    task_id: `issue:${ISSUE_NUMBER}`,
    issue_number: ISSUE_NUMBER,
    pull_number: PULL_NUMBER,
    head_ref: `agent/issue-${ISSUE_NUMBER}`,
    head_sha: HEAD_SHA,
    base_ref: 'refs/heads/main',
    base_sha: BASE_SHA,
    patch_sha256: 'c'.repeat(64),
    candidate_run_id: String(CANDIDATE_RUN_ID),
    candidate_run_attempt: 1,
    publisher_run_id: String(PUBLISHER_RUN_ID),
    publisher_run_attempt: 1,
    executor: { ...CANDIDATE_EXECUTOR, ...(source.executor ?? {}) },
    ...source,
  };
  const body = createWriterPublisherCheckRun(attestation);
  return {
    id: 4,
    ...body,
    app: { id: writerTrust.app_id, slug: writerTrust.app_slug },
    repository: { id: REPOSITORY_ID, full_name: REPOSITORY },
    ...(overrides.check ?? {}),
  };
}

function publicationWorkflowRun({ id, name, path, event, overrides = {} }) {
  return {
    id,
    run_attempt: 1,
    event,
    status: 'completed',
    conclusion: 'success',
    name,
    path,
    head_sha: BASE_SHA,
    head_branch: 'main',
    repository: { id: REPOSITORY_ID, full_name: REPOSITORY },
    head_repository: { id: REPOSITORY_ID, full_name: REPOSITORY },
    pull_requests: [],
    ...overrides,
  };
}

function publisherTriggerTarget(attestation = {}) {
  const source = {
    schema_version: 1,
    repository: REPOSITORY,
    repository_id: REPOSITORY_ID,
    task_id: `issue:${ISSUE_NUMBER}`,
    issue_number: ISSUE_NUMBER,
    pull_number: PULL_NUMBER,
    head_ref: `agent/issue-${ISSUE_NUMBER}`,
    head_sha: HEAD_SHA,
    base_ref: 'refs/heads/main',
    base_sha: BASE_SHA,
    patch_sha256: 'c'.repeat(64),
    candidate_run_id: String(CANDIDATE_RUN_ID),
    candidate_run_attempt: 1,
    publisher_run_id: String(PUBLISHER_RUN_ID),
    publisher_run_attempt: 1,
    executor: CANDIDATE_EXECUTOR,
    ...attestation,
  };
  const check = writerPublicationAttestation({ attestation: source });
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-publisher-target-'));
  const targetPath = path.join(directory, 'publisher-target.json');
  fs.writeFileSync(targetPath, serializeWriterPublisherTarget(createWriterPublisherTarget(source, check.id)));
  return {
    check,
    source,
    targetPath,
    trigger: { source: 'publisher', run_id: PUBLISHER_RUN_ID, run_attempt: 1, publisher_target_path: targetPath },
    cleanup: () => fs.rmSync(directory, { recursive: true, force: true }),
  };
}

async function evaluatePublisher(client, binding, overrides = {}) {
  return evaluateAutonomyFinalizer({ client, trigger: binding.trigger, trust, config, checkAttempts: 1, ...overrides });
}

class FakeClient {
  constructor(overrides = {}) {
    this.overrides = overrides;
    this.governanceReads = 0;
    this.protectionReads = 0;
    this.checkReads = 0;
    this.labelReads = 0;
    this.writerGovernanceReads = 0;
    this.executorRegistryReads = [];
    this.draftRestores = 0;
    this.ready = overrides.initialReady ?? false;
    this.merged = false;
    this.events = [];
    this.writerAttestation = overrides.writerAttestation === null
      ? null
      : overrides.writerAttestation ?? writerPublicationAttestation();
  }

  async getWorkflowRun(runId) {
    const numericRunId = Number(runId);
    const overridden = this.overrides.runs?.get(runId) ?? this.overrides.runs?.get(numericRunId);
    if (overridden) return overridden;
    if (numericRunId === CANDIDATE_RUN_ID) {
      return publicationWorkflowRun({
        id: CANDIDATE_RUN_ID,
        name: 'Agent candidate',
        path: '.github/workflows/agent-candidate.yml',
        event: 'workflow_dispatch',
      });
    }
    if (numericRunId === PUBLISHER_RUN_ID) {
      return publicationWorkflowRun({
        id: PUBLISHER_RUN_ID,
        name: 'Autonomy Publisher',
        path: '.github/workflows/autonomy-publisher.yml',
        event: 'workflow_run',
      });
    }
    for (const name of CHECK_IDENTITIES.keys()) {
      if (CHECK_IDENTITIES.get(name).run_id === numericRunId) {
        return workflowRun(name, numericRunId === trigger.run_id ? this.overrides.run : {});
      }
    }
    throw new Error(`unexpected workflow run ${runId}`);
  }

  async getCheckSuite(suiteId) {
    return this.overrides.suites?.get(suiteId) ?? {
      id: suiteId, head_sha: HEAD_SHA, app: { id: 15368, slug: 'github-actions' },
      pull_requests: [{ number: PULL_NUMBER }],
    };
  }
  async getCheckRun(checkRunId) {
    if (this.writerAttestation?.id === checkRunId) return this.writerAttestation;
    throw new Error(`unexpected check run ${checkRunId}`);
  }

  async getRepository() { return { id: REPOSITORY_ID, full_name: REPOSITORY, default_branch: 'main' }; }
  async getInstallationRepositories() {
    return this.overrides.installationRepositories ?? {
      total_count: 1,
      repositories: [{ id: REPOSITORY_ID, full_name: REPOSITORY, owner: { login: 'JinPengGeng' } }],
    };
  }
  async getUser() {
    return this.overrides.writerBot ?? {
      login: WRITER_BOT.login, id: WRITER_BOT.databaseId, node_id: WRITER_BOT.nodeId,
      type: 'Bot', site_admin: false,
    };
  }
  async getBranchProtection() {
    this.protectionReads += 1;
    const value = typeof this.overrides.protection === 'function'
      ? this.overrides.protection(this.protectionReads, this)
      : this.overrides.protection;
    return value ?? protection();
  }
  async getWriterGovernanceSnapshot() {
    this.writerGovernanceReads += 1;
    const value = typeof this.overrides.writerGovernance === 'function'
      ? this.overrides.writerGovernance(this.writerGovernanceReads, this)
      : this.overrides.writerGovernance;
    return value ?? writerGovernanceSnapshot();
  }
  async getPull() { return pull(); }
  async getGitRef() { return { object: { sha: BASE_SHA } }; }
  async getPullFilePage() {
    return [{ filename: 'docs/automation-canary/example.md', status: 'modified', additions: 1, deletions: 0, changes: 1, patch: '@@' }];
  }
  async getPullLabelPage() {
    this.labelReads += 1;
    const value = typeof this.overrides.policyLabels === 'function'
      ? this.overrides.policyLabels(this.labelReads, this)
      : this.overrides.policyLabels;
    return value ?? [];
  }
  async getGitCommit(sha) {
    const value = typeof this.overrides.gitCommit === 'function'
      ? this.overrides.gitCommit(sha, this)
      : this.overrides.gitCommit;
    return value ?? {
      sha,
      tree: { sha: sha === HEAD_SHA ? HEAD_TREE : BASE_TREE },
      message: sha === HEAD_SHA ? candidateCommitMessage() : 'trusted base',
    };
  }
  async getRepositoryContent(filePath, ref) {
    this.executorRegistryReads.push({ filePath, ref });
    const value = typeof this.overrides.executorRegistryFile === 'function'
      ? this.overrides.executorRegistryFile(filePath, ref, this)
      : this.overrides.executorRegistryFile;
    return value ?? candidateExecutorRegistryFile();
  }
  async getGitTree(sha) {
    return new Map([
      [HEAD_TREE, { sha, truncated: false, tree: [{ path: 'docs', mode: '040000', type: 'tree', sha: DOCS_TREE }] }],
      [BASE_TREE, { sha, truncated: false, tree: [] }],
      [DOCS_TREE, { sha, truncated: false, tree: [{ path: 'automation-canary', mode: '040000', type: 'tree', sha: CANARY_TREE }] }],
      [CANARY_TREE, { sha, truncated: false, tree: [{ path: 'example.md', mode: '100644', type: 'blob', sha: '1'.repeat(40) }] }],
    ]).get(sha);
  }
  async getPullGovernance() {
    this.governanceReads += 1;
    if (this.overrides.governanceErrors?.has(this.governanceReads)) {
      throw new Error(`governance read ${this.governanceReads} failed`);
    }
    const override = typeof this.overrides.governance === 'function'
      ? this.overrides.governance(this.governanceReads, this)
      : this.overrides.governance;
    const current = this.merged
      ? mergedGovernance()
      : governance({ isDraft: !this.ready, mergeStateStatus: this.ready ? 'CLEAN' : 'DRAFT' });
    return { ...current, ...override };
  }
  async listCheckRunsForRef() {
    this.checkReads += 1;
    const value = typeof this.overrides.checks === 'function'
      ? this.overrides.checks(this.checkReads, this)
      : this.overrides.checks;
    const checks = value ?? [
      check('Rust CI / check', 1), check('Frontend CI / check', 2), check('Automation Policy / gate', 3),
    ];
    return this.overrides.withoutWriterAttestation ? checks : [...checks, this.writerAttestation];
  }
  async markPullReady(id) {
    this.events.push('ready');
    if (this.overrides.readyErrorBefore) throw this.overrides.readyErrorBefore;
    this.ready = true;
    if (this.overrides.readyErrorAfter) throw this.overrides.readyErrorAfter;
    return this.overrides.readyResult ?? { id, isDraft: false, headRefOid: HEAD_SHA };
  }
  async convertPullToDraft(id) {
    this.events.push('draft_rollback');
    this.draftRestores += 1;
    this.ready = false;
    if (this.overrides.draftErrorAfter) throw this.overrides.draftErrorAfter;
    return this.overrides.draftResult ?? { id, isDraft: true, headRefOid: HEAD_SHA };
  }
  async mergePullRequest(id, head) {
    this.events.push('merge');
    this.mergeArguments = { id, head };
    if (this.overrides.mergeErrorBefore) throw this.overrides.mergeErrorBefore;
    if (this.overrides.persistMerge !== false) this.merged = true;
    if (this.overrides.mergeErrorAfter) throw this.overrides.mergeErrorAfter;
    return this.overrides.mergeResult ?? { id, headRefOid: head, merged: true, mergeCommit: { oid: MERGE_SHA } };
  }
}

const finalize = (client, overrides = {}) => finalizeAutonomyPull({
  readClient: client,
  writerClient: client,
  trigger,
  trust,
  config,
  sleepImpl: async () => {},
  ...overrides,
});

test('eligible Writer canary requires all three source-bound successful checks', async () => {
  const eligible = await evaluateAutonomyFinalizer({ client: new FakeClient(), trigger, trust, config, checkAttempts: 1 });
  assert.equal(eligible.eligible, true);
  for (const name of REQUIRED_PROTECTION_CONTEXTS) {
    const blocked = await evaluateAutonomyFinalizer({
      client: new FakeClient({ checks: [
        check('Rust CI / check', 1), check('Frontend CI / check', 2), check('Automation Policy / gate', 3),
      ].map((value) => value.name === name ? { ...value, conclusion: 'failure' } : value) }),
      trigger, trust, config, checkAttempts: 1,
    });
    assert.equal(blocked.eligible, false);
    assert.match(blocked.reason, new RegExp(name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
  }
});

test('Finalizer revalidates candidate executor provenance from the exact base before every Writer transition', async () => {
  const client = new FakeClient();
  const result = await finalize(client);
  assert.equal(result.action, 'merged');
  assert.ok(client.executorRegistryReads.length >= 3);
  assert.deepEqual(
    client.executorRegistryReads,
    client.executorRegistryReads.map(() => ({
      filePath: '.github/ai-executors.json',
      ref: BASE_SHA,
    })),
  );
});

test('a Writer-authored head with copied trailers and green required checks cannot reach Writer mutation without attestation', async () => {
  const client = new FakeClient({ writerAttestation: null });
  const result = await finalize(client);
  assert.equal(result.action, 'skipped');
  assert.equal(result.reason, 'writer_attestation_missing');
  assert.deepEqual(client.events, []);
});

test('a closed or reopened Writer pull is permanently tombstoned before every Writer mutation', async (t) => {
  for (const lifecycle of [
    [{ id: 71, event: 'closed' }],
    [{ id: 71, event: 'closed' }, { id: 72, event: 'reopened' }],
  ]) {
    await t.test(lifecycle.at(-1).event, async () => {
      const client = new FakeClient({ governance: { lifecycle } });
      await assert.rejects(() => finalize(client), /tombstoned by close or reopen history/);
      assert.deepEqual(client.events, []);
    });
  }
});

test('a lifecycle tombstone after Ready does not grant Draft rollback authority', async () => {
  const client = new FakeClient({
    governance: (read) => read >= 3 ? { lifecycle: [{ id: 71, event: 'closed' }] } : {},
  });
  await assert.rejects(() => finalize(client), AutonomyFinalizerError);
  assert.deepEqual(client.events, ['ready']);
});

test('merge outcome accepts only the Writer terminal close and rejects lifecycle history injected after the final stable read', async (t) => {
  await t.test('one terminal close produced by the Writer merge is accepted', async () => {
    const client = new FakeClient();
    const result = await finalize(client);
    assert.equal(result.action, 'merged');
    assert.deepEqual(client.events, ['ready', 'merge']);
  });

  await t.test('close/reopen history appearing between final stable read and outcome readback is fail closed', async () => {
    const client = new FakeClient({
      governance: (_read, current) => current.merged
        ? { lifecycle: [{ id: 91, event: 'closed' }, { id: 92, event: 'reopened' }, { id: 93, event: 'closed' }] }
        : {},
    });
    await assert.rejects(() => finalize(client), /independent merge outcome is ambiguous/);
    assert.deepEqual(client.events, ['ready', 'merge']);
  });
});

test('Writer attestation requires one exact Writer App binding before mutation', async (t) => {
  const requiredChecks = [
    check('Rust CI / check', 1), check('Frontend CI / check', 2), check('Automation Policy / gate', 3),
  ];
  const cases = [
    ['wrong App is ignored', new FakeClient({
      writerAttestation: writerPublicationAttestation({ check: { app: { id: 999, slug: 'other-app' } } }),
    }), 'skipped'],
    ['wrong base payload', new FakeClient({
      writerAttestation: writerPublicationAttestation({ attestation: { base_sha: 'd'.repeat(40) } }),
    }), 'rejected'],
    ['duplicate Writer App proof', new FakeClient({
      checks: [...requiredChecks, writerPublicationAttestation({ check: { id: 5 } })],
      writerAttestation: writerPublicationAttestation(),
    }), 'rejected'],
    ['source workflow head mismatch', new FakeClient({
      runs: new Map([[CANDIDATE_RUN_ID, publicationWorkflowRun({
        id: CANDIDATE_RUN_ID,
        name: 'Agent candidate',
        path: '.github/workflows/agent-candidate.yml',
        event: 'workflow_dispatch',
        overrides: { head_sha: HEAD_SHA },
      })]]),
    }), 'rejected'],
  ];
  for (const [name, client, outcome] of cases) {
    await t.test(name, async () => {
      if (outcome === 'skipped') {
        const result = await finalize(client);
        assert.equal(result.action, 'skipped');
      } else {
        await assert.rejects(() => finalize(client), AutonomyFinalizerError);
      }
      assert.deepEqual(client.events, []);
    });
  }
});

test('candidate executor provenance fails closed before Writer mutation', async (t) => {
  const commitFor = (message) => (sha) => ({
    sha,
    tree: { sha: sha === HEAD_SHA ? HEAD_TREE : BASE_TREE },
    message: sha === HEAD_SHA ? message : 'trusted base',
  });
  const mismatched = (field, value) => candidateCommitMessage({ ...CANDIDATE_EXECUTOR, [field]: value });
  const cases = [
    ['missing trailers', { gitCommit: commitFor('chore(autonomy): update issue #123') }],
    ['duplicate trailer', { gitCommit: commitFor(`${candidateCommitMessage()}\nAeris-Autonomy-Executor-ID: ${CANDIDATE_EXECUTOR.id}`) }],
    ['invalid trailer', { gitCommit: commitFor(candidateCommitMessage({ ...CANDIDATE_EXECUTOR, id: 'not valid' })) }],
    ['id mismatch', { gitCommit: commitFor(mismatched('id', 'other-action-v1')) }],
    ['protocol mismatch', { gitCommit: commitFor(mismatched('protocol', 'other-workspace-candidate-v1')) }],
    ['action SHA mismatch', { gitCommit: commitFor(mismatched('action_sha', '2'.repeat(40))) }],
    ['tool version mismatch', { gitCommit: commitFor(mismatched('tool_version', '0.149.0')) }],
    ['wrong commit identity', (() => {
      let headReads = 0;
      return { gitCommit: (sha) => {
        if (sha === HEAD_SHA) headReads += 1;
        return {
          sha: sha === HEAD_SHA && headReads > 1 ? '2'.repeat(40) : sha,
          tree: { sha: sha === HEAD_SHA ? HEAD_TREE : BASE_TREE },
          message: candidateCommitMessage(),
        };
      } };
    })()],
    ['non-file registry response', { executorRegistryFile: { type: 'dir', encoding: 'base64', size: 1, content: 'eA==' } }],
    ['invalid Base64 registry response', { executorRegistryFile: { type: 'file', encoding: 'base64', size: 1, content: '%' } }],
    ['invalid candidate route', { executorRegistryFile: candidateExecutorRegistryFile({
      ...candidateExecutorRegistry(),
      routes: { ...candidateExecutorRegistry().routes, candidate: 'openai-chat-v1' },
    }) }],
  ];
  for (const [name, overrides] of cases) {
    await t.test(name, async () => {
      const client = new FakeClient(overrides);
      await assert.rejects(() => finalize(client), AutonomyFinalizerError);
      assert.deepEqual(client.events, []);
    });
  }
});

test('workflow_dispatch success cannot impersonate a pull_request check', async () => {
  const runId = 900;
  const suiteId = 901;
  const client = new FakeClient({
    checks: [
      check('Rust CI / check', 1, { conclusion: 'failure' }),
      check('Rust CI / check', 99, { run_id: runId, suite_id: suiteId }),
      check('Frontend CI / check', 2), check('Automation Policy / gate', 3),
    ],
    runs: new Map([[runId, workflowRun('Rust CI / check', {
      id: runId, check_suite_id: suiteId, event: 'workflow_dispatch', pull_requests: [],
    })]]),
  });
  const result = await evaluateAutonomyFinalizer({ client, trigger, trust, config, checkAttempts: 1 });
  assert.equal(result.eligible, false);
  assert.match(result.reason, /Rust CI \/ check/);
});

test('conflict, unresolved discussion, manual label, and blocking review fail closed', async () => {
  for (const value of [
    { mergeable: 'CONFLICTING' },
    { reviewThreads: [{ id: 'thread', isResolved: false }] },
    { labels: ['autonomy-manual'] },
    { reviewDecision: 'CHANGES_REQUESTED' },
  ]) {
    await assert.rejects(
      () => evaluateAutonomyFinalizer({ client: new FakeClient({ governance: value }), trigger, trust, config, checkAttempts: 1 }),
      AutonomyFinalizerError,
    );
  }
});

test('preliminary proof is read-only and cannot enter mutation mode', async () => {
  const client = new FakeClient();
  client.getBranchProtection = async () => { throw new Error('must not read protection'); };
  client.getWriterGovernanceSnapshot = async () => { throw new Error('must not read Writer governance'); };
  const result = await evaluateAutonomyFinalizerPreliminary({ client, trigger, trust, config, checkAttempts: 1 });
  assert.equal(result.eligible, true);
  assert.equal(result.proof_level, 'preliminary');
  await assert.rejects(
    () => runAutonomyFinalizer(preliminaryEnvironment({ AERIS_FINALIZER_MUTATE: 'true' }), { readClient: client }),
    /mutation mode requires full proof/,
  );
});

test('Publisher completion re-evaluates an exact Writer target only after all source-bound checks are green', async () => {
  const binding = publisherTriggerTarget();
  try {
    const incomplete = new FakeClient({
      writerAttestation: binding.check,
      checks: [check('Rust CI / check', 1), check('Frontend CI / check', 2)],
    });
    const blocked = await evaluatePublisher(incomplete, binding);
    assert.equal(blocked.eligible, false);
    assert.match(blocked.reason, /Automation Policy \/ gate/);
    assert.deepEqual(incomplete.events, []);

    const eligible = await evaluatePublisher(new FakeClient({ writerAttestation: binding.check }), binding);
    assert.equal(eligible.eligible, true);
    assert.equal(eligible.bound.pull_number, PULL_NUMBER);
    assert.equal(eligible.bound.head_sha, HEAD_SHA);
  } finally {
    binding.cleanup();
  }
});

test('required-check and Publisher completion ordering both stay fail-closed until the missing proof arrives', async () => {
  const requiredFirst = new FakeClient({ writerAttestation: writerPublicationAttestation({ check: { status: 'in_progress', conclusion: null } }) });
  const waitingPublisher = await evaluateAutonomyFinalizer({ client: requiredFirst, trigger, trust, config, checkAttempts: 1 });
  assert.equal(waitingPublisher.eligible, false);
  assert.equal(waitingPublisher.reason, 'writer_attestation_not_successful');
  assert.deepEqual(requiredFirst.events, []);

  const binding = publisherTriggerTarget();
  try {
    let complete = false;
    const publisherFirst = new FakeClient({
      writerAttestation: binding.check,
      checks: () => complete
        ? [check('Rust CI / check', 1), check('Frontend CI / check', 2), check('Automation Policy / gate', 3)]
        : [check('Rust CI / check', 1), check('Frontend CI / check', 2)],
    });
    assert.equal((await evaluatePublisher(publisherFirst, binding)).eligible, false);
    complete = true;
    assert.equal((await evaluatePublisher(publisherFirst, binding)).eligible, true);
    assert.deepEqual(publisherFirst.events, []);
  } finally {
    binding.cleanup();
  }
});

test('Publisher completion rejects source, target, and attestation drift before any Writer mutation', async () => {
  const sourceDrifts = [
    { name: 'name', value: { name: 'other' } },
    { name: 'path', value: { path: '.github/workflows/other.yml' } },
    { name: 'event', value: { event: 'pull_request' } },
    { name: 'status', value: { status: 'in_progress' } },
    { name: 'branch', value: { head_branch: 'agent/issue-123' } },
    { name: 'head', value: { head_sha: HEAD_SHA } },
    { name: 'repository', value: { repository: { id: 1, full_name: REPOSITORY } } },
    { name: 'attempt', value: { run_attempt: 2 } },
  ];
  for (const drift of sourceDrifts) {
    const binding = publisherTriggerTarget();
    try {
      const client = new FakeClient({ writerAttestation: binding.check, runs: new Map([[PUBLISHER_RUN_ID,
        publicationWorkflowRun({ id: PUBLISHER_RUN_ID, name: 'Autonomy Publisher', path: '.github/workflows/autonomy-publisher.yml', event: 'workflow_run', overrides: drift.value }),
      ]]) });
      await assert.rejects(() => evaluatePublisher(client, binding), /./, drift.name);
      assert.deepEqual(client.events, []);
    } finally { binding.cleanup(); }
  }

  const invalidTarget = ['missing', 'noncanonical', 'oversize'];
  for (const kind of invalidTarget) {
    const binding = publisherTriggerTarget();
    try {
      if (kind === 'missing') fs.rmSync(binding.targetPath);
      if (kind === 'noncanonical') fs.appendFileSync(binding.targetPath, ' ');
      if (kind === 'oversize') fs.writeFileSync(binding.targetPath, 'x'.repeat(4097));
      const client = new FakeClient({ writerAttestation: binding.check });
      await assert.rejects(() => evaluatePublisher(client, binding), /./, kind);
      assert.deepEqual(client.events, []);
    } finally { binding.cleanup(); }
  }

  for (const attestation of [
    { pull_number: PULL_NUMBER + 1 }, { head_sha: 'e'.repeat(40) }, { base_sha: 'f'.repeat(40) },
    { patch_sha256: 'd'.repeat(64) }, { candidate_run_attempt: 2 }, { publisher_run_attempt: 2 },
  ]) {
    const binding = publisherTriggerTarget(attestation);
    try {
      const client = new FakeClient({ writerAttestation: binding.check });
      await assert.rejects(() => evaluatePublisher(client, binding), /./);
      assert.deepEqual(client.events, []);
    } finally { binding.cleanup(); }
  }

  const binding = publisherTriggerTarget();
  try {
    const duplicate = writerPublicationAttestation();
    const client = new FakeClient({ writerAttestation: binding.check, checks: [
      check('Rust CI / check', 1), check('Frontend CI / check', 2), check('Automation Policy / gate', 3), duplicate,
    ] });
    await assert.rejects(() => evaluatePublisher(client, binding), /ambiguous Writer attestations/);
    assert.deepEqual(client.events, []);
  } finally { binding.cleanup(); }
});

test('full proof requires exactly three source-bound business contexts and squash-only merges', async () => {
  const invalid = [
    protection({ requiredStatusChecks: [
      ...protectionRule().requiredStatusChecks,
      { context: 'Autonomy Finalizer / hold', app: { databaseId: 15368, slug: 'github-actions' } },
    ] }),
    protection({ requiredStatusChecks: protectionRule().requiredStatusChecks.slice(0, 2) }),
    protection({ requiredStatusChecks: protectionRule().requiredStatusChecks.map((value, index) => (
      index === 0 ? { ...value, app: { databaseId: 999, slug: 'github-actions' } } : value
    )) }),
    { ...protection(), mergeCommitAllowed: true },
    { ...protection(), rebaseMergeAllowed: true },
    { ...protection(), squashMergeAllowed: false },
    protection({ requiresStrictStatusChecks: false }),
    protection({ bypassPullRequestAllowances: completeConnection([{ id: 'allowance' }]) }),
    { ...protection(), rulesets: completeConnection([{ id: 'r', enforcement: 'ACTIVE', target: 'BRANCH' }]) },
  ];
  for (const value of invalid) {
    await assert.rejects(
      () => evaluateAutonomyFinalizer({ client: new FakeClient({ protection: value }), trigger, trust, config, checkAttempts: 1 }),
      AutonomyFinalizerError,
    );
  }
});

test('Writer App JWT, installation scope, and live Bot identity are proven before mutation', async () => {
  const cases = [
    { writerTrust: { ...writerTrust, proof_app_id: writerTrust.app_id + 1 }, error: /JWT proof/ },
    { writerTrust: { ...writerTrust, proof_app_node_id: '' }, error: /App node id/ },
    { writerTrust: { ...writerTrust, proof_app_owner_login: 'other' }, error: /repository owner/ },
    { writerTrust: { ...writerTrust, proof_app_owner_database_id: 0 }, error: /owner database id/ },
    { writerTrust: { ...writerTrust, proof_app_permissions: { administration: 'write', contents: 'write', metadata: 'read', pull_requests: 'write' } }, error: /permissions/ },
    { writerTrust: { ...writerTrust, proof_installation_permissions: { administration: 'read', contents: 'write', metadata: 'read', pull_requests: 'read' } }, error: /permissions/ },
    { writerTrust: { ...writerTrust, proof_installation_permissions: undefined }, error: /permissions/ },
    { writerTrust: { ...writerTrust, token_installation_id: writerTrust.installation_id + 1 }, error: /installation/ },
    { writerTrust: { ...writerTrust, token_app_slug: 'other' }, error: /configured App/ },
  ];
  for (const value of cases) {
    const client = new FakeClient();
    await assert.rejects(() => finalize(client, { writerTrust: value.writerTrust }), value.error);
    assert.deepEqual(client.events, []);
  }
  const scoped = new FakeClient({ installationRepositories: { total_count: 2, repositories: [] } });
  await assert.rejects(() => finalize(scoped), /repository scope is not exact/);
  assert.deepEqual(scoped.events, []);
  const human = new FakeClient({ writerBot: { login: WRITER_BOT.login, id: 7, node_id: 'U', type: 'User', site_admin: false } });
  await assert.rejects(() => finalize(human), /Writer Bot REST identity is invalid/);
  assert.deepEqual(human.events, []);
});

test('Writer governance proof rejects owner, rule, Environment, Actions, and pagination drift before mutation', async (t) => {
  const cases = [
    ['owner identity', (value) => { value.governance_fence.direct_collaborators.items[0].database_id += 1; }],
    ['ruleset bypass', (value) => { value.governance_fence.rulesets.items[0].bypass_actors[0].actor_id += 1; }],
    ['Environment policy', (value) => { value.secret_lane.environment.custom_branch_policies = false; }],
    ['Actions permissions', (value) => { value.secret_lane.actions_permissions.sha_pinning_required = false; }],
    ['collaborator pagination', (value) => { value.governance_fence.direct_collaborators.truncated = true; }],
    ['ruleset pagination', (value) => { value.governance_fence.rulesets.truncated = true; }],
    ['Environment branch pagination', (value) => { value.secret_lane.deployment_branch_policies.truncated = true; }],
  ];
  for (const [name, mutate] of cases) {
    await t.test(name, async () => {
      const client = new FakeClient({ writerGovernance: copiedWriterGovernanceSnapshot(mutate) });
      await assert.rejects(() => finalize(client), /governance|ruleset|collaborator|Environment|Actions|pagination/i);
      assert.deepEqual(client.events, []);
    });
  }
});

test('Writer governance proof requires GraphQL and REST to identify the same active fence', async (t) => {
  const cases = [
    ['missing GraphQL fence', { ...protection(), rulesets: completeConnection() }],
    ['different GraphQL fence id', {
      ...protection(),
      rulesets: completeConnection([{
        id: 'RRS_other_fence', databaseId: 202, name: 'agent-head-fence-v1',
        enforcement: 'ACTIVE', target: 'BRANCH',
      }]),
    }],
  ];
  for (const [name, value] of cases) {
    await t.test(name, async () => {
      const client = new FakeClient({ protection: value });
      await assert.rejects(() => finalize(client), /classic and REST governance fence identities do not match/);
      assert.deepEqual(client.events, []);
    });
  }
});

test('Writer governance must remain identical from the first full proof through the final pre-merge proof', async () => {
  const client = new FakeClient({
    writerGovernance: (read) => copiedWriterGovernanceSnapshot((value) => {
      value.governance_fence.rulesets.items.push({
        id: 202,
        name: read === 1 ? 'inactive-observer-a' : 'inactive-observer-b',
        target: 'branch',
        enforcement: 'evaluate',
      });
    }),
  });
  await assert.rejects(() => finalize(client), /Writer governance drifted before merge mutation/);
  assert.deepEqual(client.events, ['ready', 'draft_rollback']);
  assert.equal(client.writerGovernanceReads, 2);
});

test('concrete Writer governance reader uses a pinned owner baseline when bypass actors are redacted', async () => {
  const api = new AutonomyFinalizerGitHubClient({
    token: 'writer-token', repository: REPOSITORY,
    fetchImpl: async () => { throw new Error('unexpected network access'); },
  });
  let detailReads = 0;
  api.getRepository = async () => ({ id: REPOSITORY_ID, full_name: REPOSITORY });
  api.listDirectCollaborators = async () => ({
    truncated: false,
    items: [{
      login: writerTrust.proof_app_owner_login,
      id: writerTrust.proof_app_owner_database_id,
      type: 'User',
      permissions: { admin: true, maintain: true, push: true, triage: true, pull: true },
    }],
  });
  api.listRepositoryRulesetsIncludingParents = async () => ({
    truncated: false,
    items: [{ id: 101, name: 'agent-head-fence-v1', target: 'branch', enforcement: 'active' }],
  });
  const liveRules = [
    { type: 'creation' }, {
      type: 'update',
      parameters: { update_allows_fetch_and_merge: false },
    },
    { type: 'deletion' }, { type: 'non_fast_forward' },
  ];
  let rules = structuredClone(liveRules);
  let bypassActors = [{
    actor_id: writerTrust.app_id,
    actor_type: 'Integration',
    bypass_mode: 'always',
  }];
  let updatedAt = writerTrust.governance_fence_updated_at;
  let currentUserCanBypass = 'always';
  api.getRepositoryRuleset = async () => {
    detailReads += 1;
    const detail = {
      id: 101, name: 'agent-head-fence-v1', target: 'branch', enforcement: 'active',
      conditions: { ref_name: { include: ['refs/heads/agent/**'], exclude: [] } },
      rules: structuredClone(rules),
      updated_at: updatedAt,
      current_user_can_bypass: currentUserCanBypass,
    };
    if (bypassActors !== undefined) detail.bypass_actors = structuredClone(bypassActors);
    return detail;
  };
  api.getActionsPermissions = async () => ({
    enabled: true, allowed_actions: 'selected', sha_pinning_required: true,
  });
  api.getDefaultWorkflowPermissions = async () => ({
    default_workflow_permissions: 'read', can_approve_pull_request_reviews: false,
  });
  api.getWriterEnvironment = async () => ({
    name: 'writer', can_admins_bypass: false,
    deployment_branch_policy: { custom_branch_policies: true, protected_branches: false },
  });
  api.listWriterDeploymentBranchPolicies = async () => ({
    truncated: false, items: [{ id: 1, name: 'main', type: 'branch' }],
  });

  assert.deepEqual(await api.getWriterGovernanceSnapshot(writerTrust), writerGovernanceSnapshot());
  assert.equal(detailReads, 2);
  for (const mutate of [
    (value) => { delete value[1].parameters; },
    (value) => { value[1].parameters.update_allows_fetch_and_merge = true; },
    (value) => { value[1].parameters.unexpected = false; },
    (value) => { value[0].parameters = { update_allows_fetch_and_merge: false }; },
  ]) {
    rules = structuredClone(liveRules);
    bypassActors = [{ actor_id: writerTrust.app_id, actor_type: 'Integration', bypass_mode: 'always' }];
    updatedAt = writerTrust.governance_fence_updated_at;
    currentUserCanBypass = 'always';
    mutate(rules);
    await assert.rejects(() => api.getWriterGovernanceSnapshot(writerTrust), AutonomyFinalizerError);
  }
  rules = structuredClone(liveRules);
  bypassActors = undefined;
  updatedAt = writerTrust.governance_fence_updated_at;
  currentUserCanBypass = 'always';
  assert.deepEqual(await api.getWriterGovernanceSnapshot(writerTrust), writerGovernanceSnapshot());

  updatedAt = '2026-08-27T10:48:17.800Z';
  assert.deepEqual(await api.getWriterGovernanceSnapshot(writerTrust), writerGovernanceSnapshot());

  for (const [name, mutate, message] of [
    ['null', () => { bypassActors = null; }, /bypass actors are invalid \(shape=null\)/],
    ['object', () => { bypassActors = { opaque: 'must-not-log' }; }, /bypass actors are invalid \(shape=non-array:object\)/],
    ['oversized array', () => { bypassActors = Array.from({ length: 33 }, () => ({ opaque: 'must-not-log' })); }, /bypass actors are invalid \(shape=array:length=33\)/],
    ['updated_at drift', () => { updatedAt = '2026-08-27T18:48:18.800+08:00'; }, /updated_at does not match/],
    ['bypass capability drift', () => { currentUserCanBypass = 'pull_requests_only'; }, /cannot always bypass/],
  ]) {
    rules = structuredClone(liveRules);
    bypassActors = undefined;
    updatedAt = writerTrust.governance_fence_updated_at;
    currentUserCanBypass = 'always';
    mutate();
    await assert.rejects(() => api.getWriterGovernanceSnapshot(writerTrust), (error) => {
      assert.ok(error instanceof AutonomyFinalizerError, `${name} must remain fail-closed`);
      assert.match(error.message, message);
      assert.doesNotMatch(error.message, /must-not-log/);
      return true;
    });
  }
});

test('concrete Writer governance reader requires two identical complete snapshots', async () => {
  const api = new AutonomyFinalizerGitHubClient({
    token: 'writer-token', repository: REPOSITORY,
    fetchImpl: async () => { throw new Error('unexpected network access'); },
  });
  let reads = 0;
  api.readWriterGovernanceSnapshotOnce = async () => {
    reads += 1;
    return copiedWriterGovernanceSnapshot((value) => {
      if (reads === 2) value.governance_fence.rulesets.items[0].name = 'drifted-fence';
    });
  };
  await assert.rejects(() => api.getWriterGovernanceSnapshot(writerTrust), /drifted between complete reads/);
  assert.equal(reads, 2);
});

test('Finalizer makes one exact-head squash merge after fresh pre-ready and pre-merge proofs', async () => {
  const client = new FakeClient();
  const result = await finalize(client);
  assert.deepEqual(result, {
    action: 'merged', pull_number: PULL_NUMBER, head_sha: HEAD_SHA, merge_commit_sha: MERGE_SHA,
  });
  assert.deepEqual(client.mergeArguments, { id: 'PR_node', head: HEAD_SHA });
  assert.deepEqual(client.events, ['ready', 'merge']);
  assert.equal(client.protectionReads, 3);
  assert.equal(client.writerGovernanceReads, 2);
  assert.ok(client.governanceReads >= 5);
  assert.equal(client.draftRestores, 0);
});

test('already-ready pull skips the ready mutation but still receives a fresh full proof', async () => {
  const client = new FakeClient({ initialReady: true });
  const result = await finalize(client);
  assert.equal(result.action, 'merged');
  assert.deepEqual(client.events, ['merge']);
  assert.equal(client.protectionReads, 2);
  assert.equal(client.writerGovernanceReads, 2);
});

test('lost merge response counts as success only after independent exact outcome proof', async () => {
  const client = new FakeClient({ mergeErrorAfter: new Error('response lost') });
  const result = await finalize(client);
  assert.equal(result.action, 'merged');
  assert.equal(result.merge_commit_sha, MERGE_SHA);
  assert.equal(client.draftRestores, 0);
});

test('response-loss canary binding is absent by default and rejects malformed bindings', () => {
  const expected = { pull_number: PULL_NUMBER, head_sha: HEAD_SHA, base_sha: BASE_SHA };
  assert.equal(validateResponseLossCanaryBinding(undefined, expected), false);
  assert.equal(validateResponseLossCanaryBinding('', expected), false);
  const valid = { version: 1, fault: 'drop_merge_response_after_success', pull_number: PULL_NUMBER, head_sha: HEAD_SHA, base_sha: BASE_SHA };
  assert.equal(validateResponseLossCanaryBinding(JSON.stringify(valid), expected), true);
  for (const value of [
    'not-json',
    JSON.stringify({ ...valid, extra: true }),
    JSON.stringify({ ...valid, version: 2 }),
    JSON.stringify({ ...valid, fault: 'other' }),
    JSON.stringify({ ...valid, head_sha: 'not-a-sha' }),
  ]) {
    assert.throws(() => validateResponseLossCanaryBinding(value, expected), AutonomyFinalizerError);
  }
});

test('response-loss canary for a different PR is dormant but same-PR head or base drift fails closed', () => {
  const expected = { pull_number: PULL_NUMBER, head_sha: HEAD_SHA, base_sha: BASE_SHA };
  const valid = { version: 1, fault: 'drop_merge_response_after_success', pull_number: PULL_NUMBER, head_sha: HEAD_SHA, base_sha: BASE_SHA };
  assert.equal(validateResponseLossCanaryBinding(JSON.stringify({
    ...valid, pull_number: PULL_NUMBER + 1, head_sha: '8'.repeat(40), base_sha: '7'.repeat(40),
  }), expected), false);
  assert.throws(
    () => validateResponseLossCanaryBinding(JSON.stringify({ ...valid, head_sha: '8'.repeat(40) }), expected),
    /exact eligibility snapshot/,
  );
  assert.throws(
    () => validateResponseLossCanaryBinding(JSON.stringify({ ...valid, base_sha: '7'.repeat(40) }), expected),
    /exact eligibility snapshot/,
  );
});

test('exact response-loss canary drops only the successful merge response and accepts exact independent merge proof', async () => {
  const client = new FakeClient();
  const result = await finalize(client, {
    responseLossCanaryBinding: JSON.stringify({
      version: 1, fault: 'drop_merge_response_after_success', pull_number: PULL_NUMBER,
      head_sha: HEAD_SHA, base_sha: BASE_SHA,
    }),
  });
  assert.equal(result.action, 'merged');
  assert.equal(result.canary_marker, 'response_loss_after_merge_response');
  assert.equal(client.events.filter((event) => event === 'merge').length, 1);
});

test('response-loss canary fails closed on open or ambiguous readback and never attempts a second merge', async (t) => {
  const binding = JSON.stringify({
    version: 1, fault: 'drop_merge_response_after_success', pull_number: PULL_NUMBER,
    head_sha: HEAD_SHA, base_sha: BASE_SHA,
  });
  for (const [name, client] of [
    ['open', new FakeClient({ persistMerge: false })],
    ['ambiguous', new FakeClient({ governance: (_read, state) => state.merged ? { mergedBy: null } : {} })],
  ]) {
    await t.test(name, async () => {
      await assert.rejects(() => finalize(client, { responseLossCanaryBinding: binding }));
      assert.equal(client.events.filter((event) => event === 'merge').length, 1);
    });
  }
});

test('same-PR head or base canary drift fails before any Writer mutation', async (t) => {
  for (const [name, drift] of [
    ['head', { head_sha: '8'.repeat(40) }],
    ['base', { base_sha: '7'.repeat(40) }],
  ]) {
    await t.test(name, async () => {
      const client = new FakeClient();
      await assert.rejects(() => finalize(client, {
        responseLossCanaryBinding: JSON.stringify({
          version: 1, fault: 'drop_merge_response_after_success', pull_number: PULL_NUMBER,
          head_sha: HEAD_SHA, base_sha: BASE_SHA, ...drift,
        }),
      }), /exact eligibility snapshot/);
      assert.deepEqual(client.events, []);
    });
  }
});

test('response-loss canary bound to another PR leaves ordinary finalization unchanged', async () => {
  const client = new FakeClient();
  const result = await finalize(client, {
    responseLossCanaryBinding: JSON.stringify({
      version: 1, fault: 'drop_merge_response_after_success', pull_number: PULL_NUMBER + 1,
      head_sha: '8'.repeat(40), base_sha: '7'.repeat(40),
    }),
  });
  assert.deepEqual(result, {
    action: 'merged', pull_number: PULL_NUMBER, head_sha: HEAD_SHA, merge_commit_sha: MERGE_SHA,
  });
  assert.equal(client.events.filter((event) => event === 'merge').length, 1);
});

test('merge mutation payload is not trusted', async () => {
  const client = new FakeClient({
    mergeResult: { id: 'wrong', headRefOid: '0'.repeat(40), merged: false, mergeCommit: null },
  });
  const result = await finalize(client);
  assert.equal(result.action, 'merged');
  assert.equal(result.merge_commit_sha, MERGE_SHA);
});

test('definite failed merge restores Draft and leaves no persistent authorization', async () => {
  const client = new FakeClient({ mergeErrorBefore: new Error('merge rejected') });
  await assert.rejects(() => finalize(client), /merge rejected/);
  assert.deepEqual(client.events, ['ready', 'merge', 'draft_rollback']);
  assert.equal(client.ready, false);
  assert.equal(client.merged, false);
  assert.equal(client.draftRestores, 1);
});

test('untrusted success response without a persisted merge restores Draft', async () => {
  const client = new FakeClient({ persistMerge: false });
  await assert.rejects(() => finalize(client), /did not persist the exact squash merge/);
  assert.deepEqual(client.events, ['ready', 'merge', 'draft_rollback']);
  assert.equal(client.ready, false);
});

test('lost Draft rollback response is accepted only after independent Draft proof', async () => {
  const client = new FakeClient({
    mergeErrorBefore: new Error('merge rejected'),
    draftErrorAfter: new Error('Draft response lost'),
  });
  await assert.rejects(() => finalize(client), /merge rejected/);
  assert.equal(client.ready, false);
  assert.equal(client.draftRestores, 1);
});

test('lost ready response continues only after independent exact ready proof', async () => {
  const client = new FakeClient({ readyErrorAfter: new Error('ready response lost') });
  const result = await finalize(client);
  assert.equal(result.action, 'merged');
  assert.deepEqual(client.events, ['ready', 'merge']);
});

test('lost ready response does not grant Draft rollback authority after merge failure', async () => {
  const client = new FakeClient({
    readyErrorAfter: new Error('ready response lost'),
    mergeErrorBefore: new Error('merge rejected'),
  });
  await assert.rejects(() => finalize(client), /merge rejected/);
  assert.deepEqual(client.events, ['ready', 'merge']);
  assert.equal(client.ready, true);
  assert.equal(client.draftRestores, 0);
});

test('definite rejected ready mutation leaves the original Draft unchanged', async () => {
  const client = new FakeClient({ readyErrorBefore: new Error('ready rejected') });
  await assert.rejects(() => finalize(client), /ready rejected/);
  assert.deepEqual(client.events, ['ready']);
  assert.equal(client.ready, false);
  assert.equal(client.draftRestores, 0);
});

test('preexisting auto-merge fails closed before every Writer mutation', async () => {
  const client = new FakeClient({ governance: { autoMergeRequest: {
    enabledAt: '2026-08-20T00:00:00Z', mergeMethod: 'SQUASH',
    enabledBy: { type: 'Bot', login: WRITER_BOT.graphqlLogin, id: WRITER_BOT.nodeId, databaseId: WRITER_BOT.databaseId },
  } } });
  await assert.rejects(() => finalize(client), /preexisting auto-merge request/);
  assert.deepEqual(client.events, []);
});

test('wrong merger, merged head, base parent, or merge topology is ambiguous and never rolled back', async (t) => {
  const cases = [
    ['merger', { mergedBy: { type: 'User', login: 'person', id: 'U', databaseId: 7 } }],
    ['head', { headRefOid: '8'.repeat(40) }],
    ['base', { mergeCommit: { oid: MERGE_SHA, parentCount: 1, parents: [{ oid: '8'.repeat(40) }] } }],
    ['method', { mergeCommit: { oid: MERGE_SHA, parentCount: 2, parents: [{ oid: BASE_SHA }, { oid: HEAD_SHA }] } }],
  ];
  for (const [name, drift] of cases) {
    await t.test(name, async () => {
      const client = new FakeClient({ governance: (_read, state) => state.merged ? drift : {} });
      await assert.rejects(() => finalize(client), /outcome is ambiguous/);
      assert.equal(client.events.filter((event) => event === 'merge').length, 1);
      assert.equal(client.draftRestores, 0);
    });
  }
});

test('ambiguous outcome read never guesses or attempts Draft rollback', async () => {
  const client = new FakeClient({ governanceErrors: new Set([6]), mergeErrorAfter: new Error('response lost') });
  await assert.rejects(() => finalize(client), /independent merge outcome is ambiguous/);
  assert.equal(client.merged, true);
  assert.equal(client.draftRestores, 0);
});

test('an already-ready pull remains Ready after a definite merge failure', async () => {
  const client = new FakeClient({ initialReady: true, mergeErrorBefore: new Error('merge rejected') });
  await assert.rejects(() => finalize(client), /merge rejected/);
  assert.deepEqual(client.events, ['merge']);
  assert.equal(client.ready, true);
  assert.equal(client.draftRestores, 0);
});

test('governance, protection, and check drift before Ready cause zero mutation', async (t) => {
  const cases = [
    ['governance', new FakeClient({ governance: (read) => read === 2 ? { labels: ['autonomy-manual'] } : {} })],
    ['protection', new FakeClient({ protection: (read) => read === 2
      ? protection({ bypassPullRequestAllowances: completeConnection([{ id: 'allowance' }]) })
      : protection() })],
    ['checks', new FakeClient({ checks: (read) => [
      check('Rust CI / check', 1, read === 2 ? { conclusion: 'failure' } : {}),
      check('Frontend CI / check', 2), check('Automation Policy / gate', 3),
    ] })],
  ];
  for (const [name, client] of cases) {
    await t.test(name, async () => {
      await assert.rejects(() => finalize(client));
      assert.deepEqual(client.events, []);
    });
  }
});

test('governance, protection, and check drift immediately before merge roll back Ready with zero merge mutation', async (t) => {
  const cases = [
    ['governance', new FakeClient({ governance: (read) => read === 4 ? { labels: ['autonomy-manual'] } : {} })],
    ['protection', new FakeClient({ protection: (read) => read === 3
      ? protection({ bypassPullRequestAllowances: completeConnection([{ id: 'allowance' }]) })
      : protection() })],
    ['checks', new FakeClient({ checks: (read) => [
      check('Rust CI / check', 1, read === 3 ? { conclusion: 'failure' } : {}),
      check('Frontend CI / check', 2), check('Automation Policy / gate', 3),
    ] })],
    ['base', new FakeClient({ governance: (read) => read === 4 ? { baseRefOid: '8'.repeat(40) } : {} })],
  ];
  for (const [name, client] of cases) {
    await t.test(name, async () => {
      await assert.rejects(() => finalize(client));
      assert.equal(client.events.includes('merge'), false);
      assert.equal(client.draftRestores, 1);
      assert.equal(client.ready, false);
    });
  }
});

test('pre-merge Policy denial independently confirms exact Ready before rollback', async () => {
  const client = new FakeClient({
    policyLabels: (read) => read >= 5 ? [{ id: 1, name: 'autonomy-manual' }] : [],
  });
  await assert.rejects(() => finalize(client), /pre-merge verification failed: policy_deny/);
  assert.deepEqual(client.events, ['ready', 'draft_rollback']);
  assert.equal(client.events.includes('merge'), false);
  assert.equal(client.ready, false);
});

test('head drift after Ready is ambiguous and causes zero merge mutation', async () => {
  const client = new FakeClient({ governance: (read) => read === 3 ? { headRefOid: '8'.repeat(40) } : {} });
  await assert.rejects(() => finalize(client), /ready-for-review state is ambiguous/);
  assert.equal(client.events.includes('merge'), false);
  assert.equal(client.draftRestores, 0);
});

test('base drift in the final stable governance read is ambiguous and never rolls back', async () => {
  const client = new FakeClient({ governance: (read) => read >= 5 ? { baseRefOid: '8'.repeat(40) } : {} });
  await assert.rejects(() => finalize(client), /base drifted/);
  assert.equal(client.events.includes('merge'), false);
  assert.equal(client.draftRestores, 0);
});

test('governance drift in the final stable read is caught before merge and rolls back this run Ready', async () => {
  const client = new FakeClient({ governance: (read) => read === 5 ? { labels: ['safe-new-label'] } : {} });
  await assert.rejects(() => finalize(client), /governance drifted immediately before merge mutation/);
  assert.deepEqual(client.events, ['ready', 'draft_rollback']);
  assert.equal(client.draftRestores, 1);
});

test('Draft rollback re-reads exact governance immediately before its Writer mutation', async () => {
  const client = new FakeClient({
    mergeErrorBefore: new Error('merge rejected'),
    governance: (read) => read === 7 ? { baseRefOid: '8'.repeat(40) } : {},
  });
  await assert.rejects(() => finalize(client), /Draft rollback/);
  assert.equal(client.events.includes('draft_rollback'), false);
  assert.equal(client.ready, true);
});

function graphqlPull(overrides = {}) {
  return {
    id: 'PR_node', number: PULL_NUMBER, state: 'OPEN', isDraft: true,
    body: `<!-- aeris-autonomy-managed -->\n<!-- aeris-autonomy-task:issue:${ISSUE_NUMBER} -->`,
    headRefName: `agent/issue-${ISSUE_NUMBER}`, headRefOid: HEAD_SHA,
    baseRefName: 'main', baseRefOid: BASE_SHA,
    headRepository: { nameWithOwner: REPOSITORY },
    author: {
      __typename: 'Bot', login: WRITER_BOT.graphqlLogin,
      id: WRITER_BOT.nodeId, databaseId: WRITER_BOT.databaseId,
    },
    mergeable: 'MERGEABLE', mergeStateStatus: 'DRAFT', reviewDecision: null,
    merged: false, mergedAt: null, mergedBy: null, mergeCommit: null,
    autoMergeRequest: null,
    labels: { nodes: [], pageInfo: { hasNextPage: false } },
    reviewThreads: { nodes: [], pageInfo: { hasNextPage: false, endCursor: null } },
    ...overrides,
  };
}

function graphqlMergedPull(overrides = {}) {
  return graphqlPull({
    state: 'MERGED', isDraft: false, mergeable: 'UNKNOWN', mergeStateStatus: 'UNKNOWN',
    baseRefOid: MERGE_SHA, merged: true, mergedAt: '2026-08-21T00:00:00Z',
    mergedBy: {
      __typename: 'Bot', login: WRITER_BOT.graphqlLogin,
      id: WRITER_BOT.nodeId, databaseId: WRITER_BOT.databaseId,
    },
    mergeCommit: {
      oid: MERGE_SHA,
      parents: completeConnection([{ oid: BASE_SHA }]),
    },
    ...overrides,
  });
}

function graphqlClient(pulls, timelines = [[]]) {
  let index = 0;
  let timelineIndex = 0;
  return new AutonomyFinalizerGitHubClient({
    token: 'test-token', repository: REPOSITORY,
    fetchImpl: async (url) => {
      if (String(url).includes(`/issues/${PULL_NUMBER}/timeline?`)) {
        return {
          ok: true, status: 200,
          text: async () => JSON.stringify(timelines[Math.min(timelineIndex++, timelines.length - 1)]),
        };
      }
      return {
        ok: true, status: 200,
        text: async () => JSON.stringify({
          data: { repository: { pullRequest: pulls[Math.min(index++, pulls.length - 1)] } },
        }),
      };
    },
  });
}

test('GraphQL governance requires complete merge outcome and auto-merge fields', async () => {
  const invalid = [
    graphqlPull({ merged: undefined }),
    graphqlPull({ mergedAt: undefined }),
    graphqlPull({ mergedBy: undefined }),
    graphqlPull({ mergeCommit: undefined }),
    graphqlPull({ autoMergeRequest: undefined }),
    graphqlMergedPull({ mergedBy: { __typename: 'Bot', login: WRITER_BOT.graphqlLogin } }),
    graphqlMergedPull({ mergeCommit: { oid: MERGE_SHA, parents: null } }),
    graphqlMergedPull({ mergeCommit: { oid: MERGE_SHA, parents: completeConnection([{ oid: 'bad' }]) } }),
  ];
  for (const value of invalid) {
    await assert.rejects(() => graphqlClient([value]).getPullGovernance(PULL_NUMBER), AutonomyFinalizerError);
  }
});

test('GraphQL governance independently normalizes an exact merged outcome', async () => {
  const value = graphqlMergedPull();
  const snapshot = await graphqlClient([value, value]).getPullGovernance(PULL_NUMBER);
  assert.equal(snapshot.merged, true);
  assert.equal(snapshot.mergedBy.id, WRITER_BOT.nodeId);
  assert.deepEqual(snapshot.mergeCommit, {
    oid: MERGE_SHA, parentCount: 1, parents: [{ oid: BASE_SHA }],
  });
});

test('GraphQL governance rejects drift between complete reads', async () => {
  await assert.rejects(
    () => graphqlClient([graphqlPull(), graphqlPull({ labels: {
      nodes: [{ name: 'changed' }], pageInfo: { hasNextPage: false },
    } })]).getPullGovernance(PULL_NUMBER),
    /drifted between complete reads/,
  );
});

test('GraphQL governance preserves a complete lifecycle tombstone and rejects lifecycle drift', async () => {
  const lifecycle = [{ id: 71, event: 'closed' }, { id: 72, event: 'reopened' }];
  const snapshot = await graphqlClient([graphqlPull(), graphqlPull()], [lifecycle, lifecycle]).getPullGovernance(PULL_NUMBER);
  assert.deepEqual(snapshot.lifecycle, [{ id: '71', event: 'closed' }, { id: '72', event: 'reopened' }]);
  await assert.rejects(
    () => graphqlClient([graphqlPull(), graphqlPull()], [[], lifecycle]).getPullGovernance(PULL_NUMBER),
    /governance drifted between complete reads/,
  );
  await assert.rejects(
    () => graphqlClient([graphqlPull()], [[{ event: 'closed' }]]).getPullGovernance(PULL_NUMBER),
    /lifecycle event identity is invalid/,
  );
});

test('GraphQL governance rejects review-thread pagination duplicates', async () => {
  const first = graphqlPull({ reviewThreads: {
    nodes: [{ id: 'thread', isResolved: true }], pageInfo: { hasNextPage: true, endCursor: 'next' },
  } });
  const second = graphqlPull({ reviewThreads: {
    nodes: [{ id: 'thread', isResolved: true }], pageInfo: { hasNextPage: false, endCursor: 'next' },
  } });
  await assert.rejects(() => graphqlClient([first, second]).getPullGovernance(PULL_NUMBER), /contains duplicates/);
});

test('Finalizer reads the candidate executor registry from the exact immutable base SHA', async () => {
  let request;
  const client = new AutonomyFinalizerGitHubClient({
    token: 'read-token', repository: REPOSITORY,
    fetchImpl: async (url, options) => {
      request = { url, options };
      return { ok: true, status: 200, text: async () => JSON.stringify(candidateExecutorRegistryFile()) };
    },
  });
  await client.getRepositoryContent('.github/ai-executors.json', BASE_SHA);
  assert.equal(
    request.url,
    `https://api.github.com/repos/${REPOSITORY}/contents/.github/ai-executors.json?ref=${BASE_SHA}`,
  );
  assert.equal(request.options.method, 'GET');
  assert.throws(
    () => client.getRepositoryContent('.github/automation/src/engine.mjs', BASE_SHA),
    AutonomyFinalizerError,
  );
});

test('mergePullRequest hard-codes SQUASH and binds expectedHeadOid', async () => {
  let request;
  const client = new AutonomyFinalizerGitHubClient({
    token: 'writer-token', repository: REPOSITORY,
    fetchImpl: async (_url, options) => {
      request = JSON.parse(options.body);
      return { ok: true, status: 200, text: async () => JSON.stringify({
        data: { mergePullRequest: { pullRequest: { id: 'PR_node', headRefOid: HEAD_SHA, merged: true, mergeCommit: { oid: MERGE_SHA } } } },
      }) };
    },
  });
  await client.mergePullRequest('PR_node', HEAD_SHA);
  assert.match(request.query, /mergePullRequest\(input: \{ pullRequestId: \$id, mergeMethod: SQUASH, expectedHeadOid: \$head \}\)/);
  assert.deepEqual(request.variables, { id: 'PR_node', head: HEAD_SHA });
  assert.doesNotMatch(request.query, /enablePullRequestAutoMerge|disablePullRequestAutoMerge/);
});

test('mutating CLI routes all protection proofs and merge mutation through Writer', async () => {
  const readClient = new FakeClient();
  const writerClient = new FakeClient();
  const ready = writerClient.markPullReady.bind(writerClient);
  writerClient.markPullReady = async (...args) => {
    const result = await ready(...args);
    readClient.ready = writerClient.ready;
    return result;
  };
  const merge = writerClient.mergePullRequest.bind(writerClient);
  writerClient.mergePullRequest = async (...args) => {
    const result = await merge(...args);
    readClient.merged = writerClient.merged;
    return result;
  };
  readClient.getBranchProtection = async () => { throw new Error('Actions token must not read protection'); };
  const result = await runAutonomyFinalizer(fullEnvironment(), { readClient, writerClient, sleepImpl: async () => {} });
  assert.equal(result.action, 'merged');
  assert.equal(readClient.protectionReads, 0);
  assert.equal(writerClient.protectionReads, 3);
  assert.deepEqual(writerClient.events, ['ready', 'merge']);
});

test('full proof failure occurs before every Writer mutation', async () => {
  const readClient = new FakeClient();
  const writerClient = new FakeClient();
  writerClient.getBranchProtection = async () => { throw new Error('administration read denied'); };
  await assert.rejects(
    () => runAutonomyFinalizer(fullEnvironment(), { readClient, writerClient, sleepImpl: async () => {} }),
    /administration read denied/,
  );
  assert.deepEqual(readClient.events, []);
  assert.deepEqual(writerClient.events, []);
});
