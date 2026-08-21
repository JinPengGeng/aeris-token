import assert from 'node:assert/strict';
import test from 'node:test';

import {
  AutonomyFinalizerError,
  AutonomyFinalizerGitHubClient,
  evaluateAutonomyFinalizer,
  evaluateAutonomyFinalizerPreliminary,
  finalizeAutonomyPull,
  runAutonomyFinalizer,
} from '../src/autonomy-finalizer.mjs';

const REPOSITORY = 'JinPengGeng/aeris-token';
const REPOSITORY_ID = 1316750512;
const BASE_SHA = 'a'.repeat(40);
const HEAD_SHA = 'b'.repeat(40);
const HEAD_TREE = 'c'.repeat(40);
const BASE_TREE = 'd'.repeat(40);
const DOCS_TREE = 'e'.repeat(40);
const CANARY_TREE = 'f'.repeat(40);
const ISSUE_NUMBER = 123;
const PULL_NUMBER = 17;
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
  proof_app_owner_login: 'JinPengGeng',
  proof_app_owner_type: 'User',
  installation_id: 155342531,
  proof_installation_id: 155342531,
  proof_installation_account_login: 'JinPengGeng',
  proof_installation_account_type: 'User',
  proof_repository_selection: 'selected',
  token_installation_id: 155342531,
  token_app_slug: 'aeris-writer',
  app_slug: 'aeris-writer',
});
const configValue = {
  repository: REPOSITORY,
  base_ref: 'main',
  writer_login: 'aeris-writer[bot]',
  branch_prefix: 'agent/issue-',
  maximum_files: 20,
  maximum_changes: 2000,
};
Object.defineProperty(configValue, 'writer_trust', { value: writerTrust, enumerable: false });
const config = Object.freeze(configValue);

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
    AERIS_WRITER_PROOF_APP_OWNER_LOGIN: writerTrust.proof_app_owner_login,
    AERIS_WRITER_PROOF_APP_OWNER_TYPE: writerTrust.proof_app_owner_type,
    AERIS_WRITER_INSTALLATION_ID: String(writerTrust.installation_id),
    AERIS_WRITER_PROOF_INSTALLATION_ID: String(writerTrust.proof_installation_id),
    AERIS_WRITER_PROOF_INSTALLATION_ACCOUNT_LOGIN: writerTrust.proof_installation_account_login,
    AERIS_WRITER_PROOF_INSTALLATION_ACCOUNT_TYPE: writerTrust.proof_installation_account_type,
    AERIS_WRITER_PROOF_REPOSITORY_SELECTION: writerTrust.proof_repository_selection,
    AERIS_WRITER_TOKEN_INSTALLATION_ID: String(writerTrust.token_installation_id),
    AERIS_WRITER_TOKEN_APP_SLUG: writerTrust.app_slug,
    AERIS_FINALIZER_PROOF_LEVEL: 'full',
    AERIS_FINALIZER_MUTATE: 'true',
    ...overrides,
  });
}

const apiResponse = (payload, status = 200) =>
  new Response(payload === null ? null : JSON.stringify(payload), {
    status,
    headers: { 'content-type': 'application/json' },
  });

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
    autoMergeRequest: null, labels: [], reviewThreads: [], ...overrides,
  };
}

const REQUIRED_PROTECTION_CONTEXTS = Object.freeze([
  'Rust CI / check',
  'Frontend CI / check',
  'Automation Policy / gate',
  'Autonomy Finalizer / hold',
]);

function completeConnection(nodes = []) {
  return {
    nodes,
    totalCount: nodes.length,
    pageInfo: { hasNextPage: false, endCursor: null },
  };
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
      context,
      app: { databaseId: 15368, slug: 'github-actions' },
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
    rulesets: completeConnection(),
  };
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
    check_suite: { id: suiteId },
    app: { id: 15368, slug: 'github-actions' },
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

class FakeClient {
  constructor(overrides = {}) {
    this.overrides = overrides;
    this.pullReads = 0;
    this.governanceReads = 0;
    this.protectionReads = 0;
    this.checkReads = 0;
    this.draftRestores = 0;
    this.ready = false;
    this.armed = false;
    this.hold = null;
    this.events = [];
  }

  async getWorkflowRun(runId) {
    const overridden = this.overrides.runs?.get(runId);
    if (overridden) return overridden;
    for (const name of CHECK_IDENTITIES.keys()) {
      if (CHECK_IDENTITIES.get(name).run_id === runId) {
        return workflowRun(name, runId === trigger.run_id ? this.overrides.run : {});
      }
    }
    throw new Error(`unexpected workflow run ${runId}`);
  }

  async getCheckSuite(suiteId) {
    return this.overrides.suites?.get(suiteId) ?? {
      id: suiteId,
      head_sha: HEAD_SHA,
      app: { id: 15368, slug: 'github-actions' },
      pull_requests: [{ number: PULL_NUMBER }],
    };
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
      login: WRITER_BOT.login,
      id: WRITER_BOT.databaseId,
      node_id: WRITER_BOT.nodeId,
      type: 'Bot',
      site_admin: false,
    };
  }
  async getBranchProtection() {
    this.protectionReads += 1;
    const override = typeof this.overrides.protection === 'function'
      ? this.overrides.protection(this.protectionReads, this)
      : this.overrides.protection;
    return override ?? protection();
  }
  async getPull() { this.pullReads += 1; return pull(); }
  async getGitRef() { return { object: { sha: BASE_SHA } }; }
  async getPullFilePage() {
    return [{ filename: 'docs/automation-canary/example.md', status: 'modified', additions: 1, deletions: 0, changes: 1, patch: '@@' }];
  }
  async getPullLabelPage() { return []; }
  async getGitCommit(sha) { return { sha, tree: { sha: sha === HEAD_SHA ? HEAD_TREE : BASE_TREE } }; }
  async getGitTree(sha) {
    const values = new Map([
      [HEAD_TREE, { sha, truncated: false, tree: [{ path: 'docs', mode: '040000', type: 'tree', sha: DOCS_TREE }] }],
      [BASE_TREE, { sha, truncated: false, tree: [] }],
      [DOCS_TREE, { sha, truncated: false, tree: [{ path: 'automation-canary', mode: '040000', type: 'tree', sha: CANARY_TREE }] }],
      [CANARY_TREE, { sha, truncated: false, tree: [{ path: 'example.md', mode: '100644', type: 'blob', sha: '1'.repeat(40) }] }],
    ]);
    return values.get(sha);
  }
  async getPullGovernance() {
    this.governanceReads += 1;
    if (this.overrides.governanceErrors?.has(this.governanceReads)) {
      throw new Error(`governance read ${this.governanceReads} failed`);
    }
    const override = typeof this.overrides.governance === 'function'
      ? this.overrides.governance(this.governanceReads, this)
      : this.overrides.governance;
    return governance({
      isDraft: !this.ready,
      mergeStateStatus: this.ready && this.hold?.status === 'in_progress' ? 'BLOCKED' : this.ready ? 'CLEAN' : 'DRAFT',
      autoMergeRequest: this.armed ? {
        enabledAt: '2026-08-20T00:00:00Z',
        mergeMethod: 'SQUASH',
        enabledBy: {
          type: 'Bot', login: WRITER_BOT.graphqlLogin,
          id: WRITER_BOT.nodeId, databaseId: WRITER_BOT.databaseId,
        },
      } : null,
      ...override,
    });
  }
  async listCheckRunsForRef() {
    this.checkReads += 1;
    const override = typeof this.overrides.checks === 'function'
      ? this.overrides.checks(this.checkReads, this)
      : this.overrides.checks;
    const checks = override ?? [
      check('Rust CI / check', 1), check('Frontend CI / check', 2), check('Automation Policy / gate', 3),
    ];
    return this.hold === null ? checks : [...checks, this.hold];
  }
  async createHoldCheck(headSha, externalId) {
    this.events.push('hold_pending');
    if (this.overrides.holdCreateError) throw this.overrides.holdCreateError;
    this.hold = {
      id: 91, name: 'Autonomy Finalizer / hold', head_sha: headSha, external_id: externalId,
      status: 'in_progress', conclusion: null,
      app: { id: 15368, slug: 'github-actions' }, pull_requests: [{ number: PULL_NUMBER }],
    };
    return this.overrides.holdCreateResult ?? this.hold;
  }
  async getCheckRun(id) {
    if (this.overrides.holdReadError) throw this.overrides.holdReadError;
    if (this.hold?.id !== id) throw new Error(`unexpected check run ${id}`);
    return this.overrides.holdReadResult ?? this.hold;
  }
  async completeHoldCheck(id, externalId) {
    this.events.push('hold_success');
    if (this.overrides.holdCompleteError) throw this.overrides.holdCompleteError;
    if (this.hold?.id === id && this.hold?.external_id === externalId) {
      this.hold = { ...this.hold, status: 'completed', conclusion: 'success' };
    }
    return this.overrides.holdCompleteResult ?? this.hold;
  }
  async markPullReady(id) {
    this.events.push('ready');
    this.ready = true;
    if (this.overrides.readyError) throw this.overrides.readyError;
    return this.overrides.readyResult ?? { id, isDraft: false, headRefOid: HEAD_SHA };
  }
  async convertPullToDraft(id) {
    this.events.push('draft_rollback');
    this.draftRestores += 1;
    this.ready = false;
    this.armed = false;
    if (this.overrides.draftError) throw this.overrides.draftError;
    return { id, isDraft: true, headRefOid: HEAD_SHA };
  }
  async enableAutoMerge(id, head) {
    this.events.push('auto_merge');
    if (this.overrides.armErrorBefore) throw this.overrides.armErrorBefore;
    this.armed = true;
    if (this.overrides.armErrorAfter) throw this.overrides.armErrorAfter;
    return {
      id,
      headRefOid: head,
      autoMergeRequest: {
        enabledAt: '2026-08-20T00:00:00Z',
        mergeMethod: 'SQUASH',
        enabledBy: {
          __typename: 'Bot', login: WRITER_BOT.graphqlLogin,
          id: WRITER_BOT.nodeId, databaseId: WRITER_BOT.databaseId,
        },
      },
    };
  }
}

test('eligible Writer canary requires all three successful GitHub Actions checks', async () => {
  const result = await evaluateAutonomyFinalizer({ client: new FakeClient(), trigger, trust, config, checkAttempts: 1 });
  assert.equal(result.eligible, true);
  assert.equal(result.bound.pull_number, PULL_NUMBER);
  assert.equal(result.policy.decision.classification, 'eligible');

  const wrongSource = new FakeClient({ checks: [
    check('Rust CI / check', 1), check('Frontend CI / check', 2),
    { ...check('Automation Policy / gate', 3), app: { id: 999, slug: 'other' } },
  ] });
  const blocked = await evaluateAutonomyFinalizer({ client: wrongSource, trigger, trust, config, checkAttempts: 1 });
  assert.equal(blocked.eligible, false);
  assert.match(blocked.reason, /Automation Policy \/ gate/);

  const policySuiteId = CHECK_IDENTITIES.get('Automation Policy / gate').suite_id;
  const wrongPull = new FakeClient({ suites: new Map([[policySuiteId, {
    id: policySuiteId,
    head_sha: HEAD_SHA,
    app: { id: 15368, slug: 'github-actions' },
    pull_requests: [{ number: PULL_NUMBER + 1 }],
  }]]) });
  const suiteBlocked = await evaluateAutonomyFinalizer({ client: wrongPull, trigger, trust, config, checkAttempts: 1 });
  assert.equal(suiteBlocked.eligible, false);
  assert.match(suiteBlocked.reason, /Automation Policy \/ gate/);
});

test('a newer workflow_dispatch success cannot replace the latest pull_request check result', async () => {
  const dispatchedRunId = 900;
  const dispatchedSuiteId = 901;
  const pullRequestFailure = check('Rust CI / check', 1, { conclusion: 'failure' });
  const dispatchedSuccess = check('Rust CI / check', 99, {
    run_id: dispatchedRunId,
    suite_id: dispatchedSuiteId,
  });
  const runs = new Map([[dispatchedRunId, workflowRun('Rust CI / check', {
    id: dispatchedRunId,
    check_suite_id: dispatchedSuiteId,
    event: 'workflow_dispatch',
    pull_requests: [],
  })]]);
  const client = new FakeClient({
    checks: [
      pullRequestFailure,
      dispatchedSuccess,
      check('Frontend CI / check', 2),
      check('Automation Policy / gate', 3),
    ],
    runs,
  });
  const result = await evaluateAutonomyFinalizer({ client, trigger, trust, config, checkAttempts: 1 });
  assert.equal(result.eligible, false);
  assert.match(result.reason, /Rust CI \/ check/);
});

test('conflict, unresolved discussion, manual label, or blocking review fails closed', async () => {
  for (const value of [
    { mergeable: 'CONFLICTING' },
    { reviewThreads: [{ isResolved: false }] },
    { labels: ['autonomy-manual'] },
    { reviewDecision: 'CHANGES_REQUESTED' },
  ]) {
    await assert.rejects(
      () => evaluateAutonomyFinalizer({ client: new FakeClient({ governance: value }), trigger, trust, config, checkAttempts: 1 }),
      AutonomyFinalizerError,
    );
  }
});

test('Finalizer binds the managed branch to the Issue marker, not the pull request number', async () => {
  const result = await evaluateAutonomyFinalizer({ client: new FakeClient(), trigger, trust, config, checkAttempts: 1 });
  assert.equal(result.bound.pull_number, PULL_NUMBER);
  assert.equal(result.governance.headRefName, `agent/issue-${ISSUE_NUMBER}`);

  for (const value of [
    { headRefName: `agent/issue-${PULL_NUMBER}` },
    { body: `<!-- aeris-autonomy-managed -->\n<!-- aeris-autonomy-task:issue:${PULL_NUMBER} -->` },
  ]) {
    await assert.rejects(
      () => evaluateAutonomyFinalizer({ client: new FakeClient({ governance: value }), trigger, trust, config, checkAttempts: 1 }),
      AutonomyFinalizerError,
    );
  }
});

test('Finalizer marks ready, revalidates, and requests native auto-merge at the exact head', async () => {
  const client = new FakeClient();
  const result = await finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} });
  assert.deepEqual(result, { action: 'armed', pull_number: PULL_NUMBER, head_sha: HEAD_SHA });
  assert.equal(client.ready, true);
  assert.equal(client.armed, true);
  assert.deepEqual(client.events, ['hold_pending', 'ready', 'auto_merge', 'hold_success']);
  assert.ok(client.pullReads >= 4);
  assert.ok(client.protectionReads >= 3);
});

test('Finalizer does not mark a draft ready when branch protection drifts after hold acquisition', async () => {
  const client = new FakeClient({
    protection: (_read, state) => state.hold === null
      ? protection()
      : protection({ bypassPullRequestAllowances: completeConnection([{ id: 'allowance-1' }]) }),
  });
  await assert.rejects(
    () => finalizeAutonomyPull({
      readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {},
    }),
    /bypassPullRequestAllowances are not empty/,
  );
  assert.equal(client.events.filter((event) => event === 'ready').length, 0);
  assert.equal(client.hold.status, 'in_progress');
  assert.equal(client.events.includes('hold_success'), false);
});

test('Finalizer does not mark a draft ready when a required business check drifts after hold acquisition', async () => {
  const client = new FakeClient({
    checks: (_read, state) => [
      check('Rust CI / check', 1, state.hold === null ? {} : { conclusion: 'failure' }),
      check('Frontend CI / check', 2),
      check('Automation Policy / gate', 3),
    ],
  });
  await assert.rejects(
    () => finalizeAutonomyPull({
      readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {},
    }),
    /pre-ready verification failed: required_checks_not_successful:Rust CI \/ check/,
  );
  assert.equal(client.events.filter((event) => event === 'ready').length, 0);
  assert.equal(client.hold.status, 'in_progress');
  assert.equal(client.events.includes('hold_success'), false);
});

test('preliminary Finalizer proof never reads branch protection', async () => {
  const client = new FakeClient();
  client.getBranchProtection = async () => {
    throw new Error('Actions token must not read protected repository governance');
  };
  const direct = await evaluateAutonomyFinalizerPreliminary({
    client, trigger, trust, config, checkAttempts: 1,
  });
  assert.equal(direct.eligible, true);
  assert.equal(direct.proof_level, 'preliminary');
  assert.equal(direct.protection, null);
  assert.equal(client.protectionReads, 0);

  const cli = await runAutonomyFinalizer(preliminaryEnvironment(), {
    readClient: client,
    sleepImpl: async () => {},
  });
  assert.equal(cli.eligible, true);
  assert.equal(cli.proof_level, 'preliminary');
  assert.equal(client.protectionReads, 0);
});

test('preliminary proof cannot enter mutation mode', async () => {
  await assert.rejects(
    () => runAutonomyFinalizer(preliminaryEnvironment({ AERIS_FINALIZER_MUTATE: 'true' }), {
      readClient: new FakeClient(),
    }),
    /mutation mode requires full proof/,
  );
});

test('Finalizer requires strict source-bound hold branch protection before eligibility', async () => {
  const requiredContexts = REQUIRED_PROTECTION_CONTEXTS;
  const validRule = protectionRule;
  const proof = (rule) => ({
    ...protection(),
    branchProtectionRules: completeConnection(rule === null ? [] : [rule]),
    rulesets: completeConnection(),
  });
  const invalid = [
    proof(null),
    { ...proof(validRule()), branchProtectionRules: { ...completeConnection([validRule()]), totalCount: 2 } },
    { ...proof(validRule()), rulesets: { ...completeConnection(), pageInfo: { hasNextPage: true, endCursor: 'next' } } },
    { ...proof(validRule()), autoMergeAllowed: false },
    { ...proof(validRule()), mergeCommitAllowed: true },
    { ...proof(validRule()), rebaseMergeAllowed: true },
    { ...proof(validRule()), squashMergeAllowed: false },
    { ...proof(validRule()), isArchived: true },
    { ...proof(validRule()), isDisabled: true },
    { ...proof(validRule()), isLocked: true },
    proof(validRule({ requiresStrictStatusChecks: false })),
    proof(validRule({ isAdminEnforced: false })),
    proof(validRule({ requiresConversationResolution: false })),
    proof(validRule({ allowsDeletions: true })),
    proof(validRule({ allowsForcePushes: true })),
    proof(validRule({ blocksCreations: true })),
    proof(validRule({ dismissesStaleReviews: false })),
    proof(validRule({ lockAllowsFetchAndMerge: true })),
    proof(validRule({ lockBranch: true })),
    proof(validRule({ requireLastPushApproval: true })),
    proof(validRule({ requiredApprovingReviewCount: 1 })),
    proof(validRule({ requiredDeploymentEnvironments: ['production'] })),
    proof(validRule({ requiresApprovingReviews: false })),
    proof(validRule({ requiresCodeOwnerReviews: true })),
    proof(validRule({ requiresCommitSignatures: true })),
    proof(validRule({ requiresDeployments: true })),
    proof(validRule({ requiresLinearHistory: false })),
    proof(validRule({ restrictsPushes: true })),
    proof(validRule({ restrictsReviewDismissals: true })),
    proof(validRule({ bypassPullRequestAllowances: completeConnection([{ id: 'allowance-1' }]) })),
    proof(validRule({ bypassPullRequestAllowances: { ...completeConnection(), pageInfo: { hasNextPage: true, endCursor: 'next' } } })),
    proof(validRule({ bypassForcePushAllowances: completeConnection([{ id: 'allowance-1' }]) })),
    proof(validRule({ pushAllowances: completeConnection([{ id: 'allowance-1' }]) })),
    proof(validRule({ reviewDismissalAllowances: completeConnection([{ id: 'allowance-1' }]) })),
    {
      ...proof(validRule()),
      rulesets: completeConnection([{ id: 'ruleset-1', enforcement: 'ACTIVE', target: 'BRANCH' }]),
    },
    proof(validRule({
        requiredStatusChecks: [
          ...requiredContexts.map((context) => ({
            context, app: { databaseId: 15368, slug: 'github-actions' },
          })),
          { context: 'Unexpected / bypass', app: { databaseId: 15368, slug: 'github-actions' } },
        ],
      })),
    proof(validRule({
        requiredStatusChecks: requiredContexts.map((context) => ({
          context,
          app: { databaseId: context === 'Autonomy Finalizer / hold' ? 999 : 15368, slug: 'github-actions' },
        })),
      })),
    {
      ...protection(),
      branchProtectionRules: completeConnection([
        validRule(),
        validRule({ pattern: '*' }),
      ]),
    },
  ];
  for (const protection of invalid) {
    await assert.rejects(
      () => evaluateAutonomyFinalizer({
        client: new FakeClient({ protection }), trigger, trust, config, checkAttempts: 1,
      }),
      AutonomyFinalizerError,
    );
  }
});

test('Finalizer binds Writer token scope and trusted action outputs before mutation', async () => {
  const cases = [
    {
      client: new FakeClient(),
      writerTrust: { ...writerTrust, app_id: writerTrust.app_id + 1 },
      error: /Writer App JWT proof does not match the trusted App identity/,
    },
    {
      client: new FakeClient(),
      writerTrust: { ...writerTrust, proof_app_id: writerTrust.app_id + 1 },
      error: /Writer App JWT proof does not match the trusted App identity/,
    },
    {
      client: new FakeClient(),
      writerTrust: { ...writerTrust, proof_app_id: undefined },
      error: /Writer proof App id must be a positive integer/,
    },
    {
      client: new FakeClient(),
      writerTrust: { ...writerTrust, proof_app_owner_login: 'other-owner' },
      error: /does not match the repository owner/,
    },
    {
      client: new FakeClient(),
      writerTrust: { ...writerTrust, proof_app_owner_type: undefined },
      error: /Writer proof App owner type is invalid/,
    },
    {
      client: new FakeClient(),
      writerTrust: { ...writerTrust, proof_installation_id: writerTrust.installation_id + 1 },
      error: /Writer App JWT proof does not match the trusted installation/,
    },
    {
      client: new FakeClient(),
      writerTrust: { ...writerTrust, proof_installation_account_login: 'other-owner' },
      error: /does not match the repository installation/,
    },
    {
      client: new FakeClient(),
      writerTrust: { ...writerTrust, proof_repository_selection: 'all' },
      error: /does not match the repository installation/,
    },
    {
      client: new FakeClient({ installationRepositories: { total_count: 2, repositories: [] } }),
      writerTrust,
      error: /repository scope is not exact/,
    },
    {
      client: new FakeClient({
        installationRepositories: {
          total_count: 1,
          repositories: [{ id: 999, full_name: REPOSITORY, owner: { login: 'JinPengGeng' } }],
        },
      }),
      writerTrust,
      error: /repository identity is invalid/,
    },
    {
      client: new FakeClient(),
      writerTrust: { ...writerTrust, app_slug: 'wrong-writer' },
      error: /does not match policy/,
    },
    {
      client: new FakeClient(),
      writerTrust: { ...writerTrust, app_id: 0 },
      error: /trusted App id must be a positive integer/,
    },
    {
      client: new FakeClient(),
      writerTrust: { ...writerTrust, installation_id: 0 },
      error: /trusted installation id must be a positive integer/,
    },
    {
      client: new FakeClient(),
      writerTrust: { ...writerTrust, token_installation_id: 155342532 },
      error: /does not match the configured installation/,
    },
    {
      client: new FakeClient(),
      writerTrust: { ...writerTrust, token_app_slug: 'other-writer' },
      error: /Writer token App does not match the configured App/,
    },
    {
      client: new FakeClient({ writerBot: { login: 'person', id: 7, node_id: 'U_7', type: 'User', site_admin: false } }),
      writerTrust,
      error: /Writer Bot REST identity is invalid/,
    },
    {
      client: new FakeClient({
        writerBot: {
          login: WRITER_BOT.login,
          id: WRITER_BOT.databaseId + 1,
          node_id: 'BOT_other',
          type: 'Bot',
          site_admin: false,
        },
      }),
      writerTrust,
      error: /author does not match the live Writer Bot/,
    },
  ];
  for (const value of cases) {
    await assert.rejects(
      () => finalizeAutonomyPull({
        readClient: value.client,
        writerClient: value.client,
        trigger,
        trust,
        config,
        writerTrust: value.writerTrust,
        sleepImpl: async () => {},
      }),
      value.error,
    );
    assert.deepEqual(value.client.events, []);
  }
});

test('Finalizer adopts exact hold checks from the real REST check-runs envelope', async () => {
  const client = new FakeClient();
  const calls = [];
  const api = new AutonomyFinalizerGitHubClient({
    token: 'actions-token',
    repository: REPOSITORY,
    fetchImpl: async (url) => {
      calls.push(url);
      const checkRuns = [
        check('Rust CI / check', 1),
        check('Frontend CI / check', 2),
        check('Automation Policy / gate', 3),
        ...(client.hold === null ? [] : [client.hold]),
      ];
      return apiResponse({ total_count: checkRuns.length, check_runs: checkRuns });
    },
  });
  client.listCheckRunsForRef = (ref) => api.listCheckRunsForRef(ref);

  const result = await finalizeAutonomyPull({
    readClient: client,
    writerClient: client,
    trigger,
    trust,
    config,
    sleepImpl: async () => {},
  });

  assert.equal(result.action, 'armed');
  assert.equal(client.hold.status, 'completed');
  assert.ok(calls.length >= 2);
  assert.ok(calls.every((url) => url.includes(`/commits/${HEAD_SHA}/check-runs?filter=all&per_page=100&page=1`)));
});

test('Finalizer rejects a User with the Writer App GraphQL login', async () => {
  await assert.rejects(
    () => evaluateAutonomyFinalizer({
      client: new FakeClient({ governance: { authorType: 'User', author: 'aeris-writer' } }),
      trigger, trust, config, checkAttempts: 1,
    }),
    /author is not the Writer App/,
  );
});

test('Finalizer accepts only auto-merge enabled by the live Writer Bot', async () => {
  const human = new FakeClient({
    governance: {
      autoMergeRequest: {
        enabledAt: '2026-08-20T00:00:00Z', mergeMethod: 'SQUASH',
        enabledBy: { type: 'User', login: WRITER_BOT.graphqlLogin, id: 'U_same_login', databaseId: 8 },
      },
    },
  });
  human.ready = true;
  human.armed = true;
  await assert.rejects(
    () => finalizeAutonomyPull({ readClient: human, writerClient: human, trigger, trust, config, sleepImpl: async () => {} }),
    /auto-merge was not enabled by the Writer App/,
  );
  assert.deepEqual(human.events, []);

  const otherBot = new FakeClient({
    governance: {
      autoMergeRequest: {
        enabledAt: '2026-08-20T00:00:00Z', mergeMethod: 'SQUASH',
        enabledBy: {
          type: 'Bot', login: WRITER_BOT.graphqlLogin,
          id: 'BOT_other', databaseId: WRITER_BOT.databaseId + 1,
        },
      },
    },
  });
  otherBot.ready = true;
  otherBot.armed = true;
  await assert.rejects(
    () => finalizeAutonomyPull({ readClient: otherBot, writerClient: otherBot, trigger, trust, config, sleepImpl: async () => {} }),
    /auto-merge actor does not match the live Writer Bot/,
  );
  assert.deepEqual(otherBot.events, []);
});

test('Finalizer trusts an independent arm read, not an untrustworthy mutation response', async () => {
  const client = new FakeClient();
  const originalEnable = client.enableAutoMerge.bind(client);
  client.enableAutoMerge = async (id, head) => {
    await originalEnable(id, head);
    return { id, headRefOid: head, autoMergeRequest: null };
  };
  const result = await finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} });
  assert.equal(result.action, 'armed');
  assert.equal(client.draftRestores, 0);
  assert.equal(client.hold.status, 'completed');
});

test('Finalizer recovers when arming succeeds but its response is lost', async () => {
  const client = new FakeClient({ armErrorAfter: new Error('connection lost') });
  const result = await finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} });
  assert.equal(result.action, 'armed');
  assert.equal(client.armed, true);
  assert.equal(client.hold.status, 'completed');
  assert.equal(client.draftRestores, 0);
});

test('Finalizer refuses aggregate merge states incompatible with a pending hold', async () => {
  const client = new FakeClient({ governance: { mergeStateStatus: 'CLEAN' } });
  await assert.rejects(
    () => finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} }),
    /aggregate merge state is incompatible/,
  );
  assert.equal(client.armed, false);
  assert.equal(client.hold.status, 'in_progress');
  assert.equal(client.draftRestores, 1);
});

test('Finalizer accepts GitHub UNSTABLE while the exact required hold is pending', async () => {
  const client = new FakeClient({ governance: { mergeStateStatus: 'UNSTABLE' } });
  const result = await finalizeAutonomyPull({
    readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {},
  });
  assert.equal(result.action, 'armed');
  assert.equal(client.hold.status, 'completed');
});

test('Finalizer keeps the hold pending when aggregate merge state drifts after arming', async (t) => {
  for (const mergeStateStatus of ['CLEAN', 'BEHIND', 'DRAFT', 'HAS_HOOKS', 'UNKNOWN']) {
    await t.test(mergeStateStatus, async () => {
      const client = new FakeClient({
        governance: (_read, state) => state.armed ? { mergeStateStatus } : {},
      });
      await assert.rejects(
        () => finalizeAutonomyPull({
          readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {},
        }),
        AutonomyFinalizerError,
      );
      assert.equal(client.armed, true);
      assert.equal(client.hold.status, 'in_progress');
      assert.equal(client.events.includes('hold_success'), false);
    });
  }
});

test('Finalizer keeps the hold pending when the base commit drifts after arming', async () => {
  const client = new FakeClient({
    governance: (_read, state) => state.armed ? { baseRefOid: '9'.repeat(40) } : {},
  });
  await assert.rejects(
    () => finalizeAutonomyPull({
      readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {},
    }),
    /base drifted/,
  );
  assert.equal(client.armed, true);
  assert.equal(client.hold.status, 'in_progress');
  assert.equal(client.events.includes('hold_success'), false);
});

test('Finalizer leaves an armed pull held when post-arm governance is inconclusive', async () => {
  const client = new FakeClient({ governanceErrors: new Set([5, 6]) });
  await assert.rejects(
    () => finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} }),
    /governance read 5 failed/,
  );
  assert.equal(client.armed, true);
  assert.equal(client.ready, true);
  assert.equal(client.draftRestores, 0);
  assert.equal(client.hold.status, 'in_progress');
});

test('Finalizer leaves an armed pull held when branch protection drifts after arming', async () => {
  const client = new FakeClient({
    protection: (_read, state) => state.armed
      ? protection({ bypassPullRequestAllowances: completeConnection([{ id: 'allowance-1' }]) })
      : protection(),
  });
  await assert.rejects(
    () => finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} }),
    /bypassPullRequestAllowances are not empty/,
  );
  assert.equal(client.armed, true);
  assert.equal(client.hold.status, 'in_progress');
  assert.equal(client.events.includes('hold_success'), false);
});

test('Finalizer leaves an armed pull held when a business check drifts after arming', async () => {
  const client = new FakeClient({
    checks: (_read, state) => [
      check('Rust CI / check', 1, state.armed ? { conclusion: 'failure' } : {}),
      check('Frontend CI / check', 2),
      check('Automation Policy / gate', 3),
    ],
  });
  await assert.rejects(
    () => finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} }),
    /post-arm verification failed: required_checks_not_successful:Rust CI \/ check/,
  );
  assert.equal(client.armed, true);
  assert.equal(client.hold.status, 'in_progress');
  assert.equal(client.events.includes('hold_success'), false);
});

test('Finalizer leaves an armed pull held when non-blocking governance fields drift after arming', async () => {
  const client = new FakeClient({
    governance: (read) => read >= 6 ? { labels: ['safe-but-new-label'] } : {},
  });
  await assert.rejects(
    () => finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} }),
    /governance drifted after native auto-merge request: labels/,
  );
  assert.equal(client.armed, true);
  assert.equal(client.hold.status, 'in_progress');
  assert.equal(client.events.includes('hold_success'), false);
});

test('Finalizer leaves an armed pull held when the exact hold is no longer pending at release', async () => {
  const client = new FakeClient();
  const getCheckRun = client.getCheckRun.bind(client);
  client.getCheckRun = async (id) => {
    const value = await getCheckRun(id);
    return client.armed ? { ...value, status: 'completed', conclusion: 'cancelled' } : value;
  };
  await assert.rejects(
    () => finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} }),
    /hold check is not pending/,
  );
  assert.equal(client.armed, true);
  assert.equal(client.hold.status, 'in_progress');
  assert.equal(client.events.includes('hold_success'), false);
});

test('Finalizer retries an armed pending hold without rearming', async () => {
  const client = new FakeClient({ holdCompleteError: new Error('hold response lost') });
  await assert.rejects(
    () => finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} }),
    /hold response lost/,
  );
  assert.equal(client.armed, true);
  assert.equal(client.hold.status, 'in_progress');
  client.overrides.holdCompleteError = null;
  const result = await finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} });
  assert.equal(result.action, 'already_armed');
  assert.equal(client.events.filter((event) => event === 'auto_merge').length, 1);
  assert.equal(client.hold.status, 'completed');
});

test('Finalizer confirms a hold release whose HTTP response was lost', async () => {
  const client = new FakeClient();
  const complete = client.completeHoldCheck.bind(client);
  client.completeHoldCheck = async (...args) => {
    await complete(...args);
    throw new Error('completion response lost');
  };
  const result = await finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} });
  assert.equal(result.action, 'armed');
  assert.equal(client.hold.status, 'completed');
});

test('Finalizer keeps the hold pending and restores Draft after a definite unarmed failure', async () => {
  const client = new FakeClient({ armErrorBefore: new Error('arming rejected') });
  await assert.rejects(
    () => finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} }),
    /arming rejected/,
  );
  assert.equal(client.draftRestores, 1);
  assert.equal(client.hold.status, 'in_progress');
  assert.deepEqual(client.events, ['hold_pending', 'ready', 'auto_merge', 'draft_rollback']);
});

test('Finalizer fails closed on an ambiguous exact-head hold', async () => {
  const client = new FakeClient();
  const originalChecks = client.listCheckRunsForRef.bind(client);
  client.listCheckRunsForRef = async () => [
    ...(await originalChecks()),
    {
      id: 92, name: 'Autonomy Finalizer / hold', head_sha: HEAD_SHA,
      external_id: 'foreign-generation', status: 'in_progress', conclusion: null,
      app: { id: 15368, slug: 'github-actions' }, pull_requests: [{ number: PULL_NUMBER }],
    },
  ];
  await assert.rejects(
    () => finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} }),
    /hold check/,
  );
  assert.deepEqual(client.events, []);
});

test('Finalizer restores Draft when the ready mutation response reports head drift', async () => {
  const client = new FakeClient({
    readyResult: { id: 'PR_node', isDraft: false, headRefOid: '9'.repeat(40) },
  });
  await assert.rejects(
    () => finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} }),
    /exact ready-for-review transition/,
  );
  assert.equal(client.draftRestores, 1);
  assert.equal(client.ready, false);
  assert.equal(client.armed, false);
});

test('Finalizer restores Draft when governance fails after the ready transition', async () => {
  const client = new FakeClient({
    governance: (read) => read === 3 ? { labels: ['autonomy-manual'] } : {},
  });
  await assert.rejects(
    () => finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} }),
    /manual-only label/,
  );
  assert.equal(client.draftRestores, 1);
  assert.equal(client.ready, false);
  assert.equal(client.armed, false);
});

test('Finalizer rereads governance immediately before ready without mutating drifted state', async () => {
  const client = new FakeClient({
    governance: (read) => read === 2 ? { labels: ['autonomy-manual'] } : {},
  });
  await assert.rejects(
    () => finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} }),
    /manual-only label/,
  );
  assert.equal(client.draftRestores, 0);
  assert.equal(client.ready, false);
});

test('an already armed exact pull is idempotent', async () => {
  const client = new FakeClient();
  client.ready = true;
  client.armed = true;
  const result = await finalizeAutonomyPull({ readClient: client, writerClient: client, trigger, trust, config, sleepImpl: async () => {} });
  assert.equal(result.action, 'already_armed');
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
    autoMergeRequest: null,
    labels: { nodes: [], pageInfo: { hasNextPage: false } },
    reviewThreads: { nodes: [], pageInfo: { hasNextPage: false, endCursor: null } },
    ...overrides,
  };
}

function graphqlClient(pulls) {
  let index = 0;
  return new AutonomyFinalizerGitHubClient({
    token: 'test-token',
    repository: REPOSITORY,
    fetchImpl: async () => ({
      ok: true,
      status: 200,
      text: async () => JSON.stringify({
        data: { repository: { pullRequest: pulls[Math.min(index++, pulls.length - 1)] } },
      }),
    }),
  });
}

test('GraphQL governance rejects missing labels, threads, pageInfo, node fields, and auto-merge fields', async () => {
  const invalid = [
    graphqlPull({ labels: null }),
    graphqlPull({ labels: { nodes: null, pageInfo: { hasNextPage: false } } }),
    graphqlPull({ labels: { nodes: [{}], pageInfo: { hasNextPage: false } } }),
    graphqlPull({ labels: { nodes: [], pageInfo: null } }),
    graphqlPull({ reviewThreads: null }),
    graphqlPull({ reviewThreads: { nodes: null, pageInfo: { hasNextPage: false, endCursor: null } } }),
    graphqlPull({ reviewThreads: { nodes: [], pageInfo: null } }),
    graphqlPull({ reviewThreads: { nodes: [], pageInfo: { hasNextPage: true, endCursor: null } } }),
    graphqlPull({ autoMergeRequest: undefined }),
    graphqlPull({ autoMergeRequest: { enabledAt: '2026-08-20T00:00:00Z' } }),
    graphqlPull({ autoMergeRequest: {
      enabledAt: '2026-08-20T00:00:00Z', mergeMethod: 'SQUASH', enabledBy: null,
    } }),
    graphqlPull({ autoMergeRequest: {
      enabledAt: '2026-08-20T00:00:00Z', mergeMethod: 'SQUASH',
      enabledBy: { __typename: 'Bot', login: WRITER_BOT.graphqlLogin, id: WRITER_BOT.nodeId },
    } }),
  ];
  for (const pullValue of invalid) {
    await assert.rejects(
      () => graphqlClient([pullValue]).getPullGovernance(PULL_NUMBER),
      AutonomyFinalizerError,
    );
  }
});

test('GraphQL governance requires the Bot author type as well as the Writer slug', async () => {
  const invalid = graphqlPull({
    author: { __typename: 'User', login: WRITER_BOT.graphqlLogin, id: 'U_same_login', databaseId: 7 },
  });
  const snapshot = await graphqlClient([invalid, invalid]).getPullGovernance(PULL_NUMBER);
  const client = new FakeClient({ governance: snapshot });
  await assert.rejects(
    () => evaluateAutonomyFinalizer({ client, trigger, trust, config, checkAttempts: 1 }),
    /author is not the Writer App/,
  );
});

test('GraphQL governance rejects drift between complete reads', async () => {
  const initial = graphqlPull();
  const changed = graphqlPull({
    labels: { nodes: [{ name: 'autonomy-manual' }], pageInfo: { hasNextPage: false } },
  });
  await assert.rejects(
    () => graphqlClient([initial, changed]).getPullGovernance(PULL_NUMBER),
    /drifted between complete reads/,
  );
});

test('GraphQL governance rejects pagination duplicates and non-thread field drift', async () => {
  const first = graphqlPull({
    reviewThreads: { nodes: [{ id: 'thread-1', isResolved: true }], pageInfo: { hasNextPage: true, endCursor: 'cursor-1' } },
  });
  const duplicate = graphqlPull({
    reviewThreads: { nodes: [{ id: 'thread-1', isResolved: true }], pageInfo: { hasNextPage: false, endCursor: 'cursor-1' } },
  });
  await assert.rejects(
    () => graphqlClient([first, duplicate]).getPullGovernance(PULL_NUMBER),
    /contains duplicates/,
  );

  const drifted = graphqlPull({
    labels: { nodes: [{ name: 'changed' }], pageInfo: { hasNextPage: false } },
    reviewThreads: { nodes: [], pageInfo: { hasNextPage: false, endCursor: 'cursor-1' } },
  });
  await assert.rejects(
    () => graphqlClient([first, drifted]).getPullGovernance(PULL_NUMBER),
    /drifted during pagination/,
  );
});

test('GraphQL governance accepts two identical complete strict snapshots', async () => {
  const pullValue = graphqlPull({
    labels: { nodes: [{ name: 'safe-label' }], pageInfo: { hasNextPage: false } },
    reviewThreads: { nodes: [{ id: 'thread-1', isResolved: true }], pageInfo: { hasNextPage: false, endCursor: 'cursor-1' } },
  });
  const snapshot = await graphqlClient([pullValue, pullValue]).getPullGovernance(PULL_NUMBER);
  assert.deepEqual(snapshot.labels, ['safe-label']);
  assert.deepEqual(snapshot.reviewThreads, [{ id: 'thread-1', isResolved: true }]);
  assert.equal(snapshot.authorId, WRITER_BOT.nodeId);
  assert.equal(snapshot.authorDatabaseId, WRITER_BOT.databaseId);
});

test('GraphQL branch protection reads source-bound required check descriptions', async () => {
  const proof = protection();
  let requestBody;
  const client = new AutonomyFinalizerGitHubClient({
    token: 'test-token',
    repository: REPOSITORY,
    fetchImpl: async (_url, options) => {
      requestBody = JSON.parse(options.body);
      return {
        ok: true,
        status: 200,
        text: async () => JSON.stringify({ data: { repository: proof } }),
      };
    },
  });
  assert.deepEqual(await client.getBranchProtection(), proof);
  assert.match(requestBody.query, /requiredStatusChecks\s*\{\s*context\s+app\s*\{\s*databaseId\s+slug/);
  assert.match(requestBody.query, /isAdminEnforced/);
  assert.match(requestBody.query, /requiresConversationResolution/);
  assert.match(requestBody.query, /autoMergeAllowed/);
  assert.match(requestBody.query, /mergeCommitAllowed/);
  assert.match(requestBody.query, /rebaseMergeAllowed/);
  assert.match(requestBody.query, /squashMergeAllowed/);
  assert.match(requestBody.query, /requiredApprovingReviewCount/);
  assert.match(requestBody.query, /requiredDeploymentEnvironments/);
  assert.match(requestBody.query, /requiresLinearHistory/);
  assert.match(requestBody.query, /pushAllowances\(first: 100\)/);
  assert.match(requestBody.query, /reviewDismissalAllowances\(first: 100\)/);
  assert.match(requestBody.query, /bypassPullRequestAllowances\(first: 100\)\s*\{\s*totalCount/);
  assert.match(requestBody.query, /bypassForcePushAllowances\(first: 100\)\s*\{\s*totalCount/);
  assert.match(requestBody.query, /rulesets\(first: 100, includeParents: true, targets: \[BRANCH\]\)/);
  assert.match(requestBody.query, /nodes\s*\{\s*id databaseId name enforcement target\s*\}/);
  assert.match(requestBody.query, /totalCount pageInfo\s*\{\s*hasNextPage endCursor\s*\}/);
});

test('Writer token proof reads only its repository scope and live Bot identity', async () => {
  const calls = [];
  const client = new AutonomyFinalizerGitHubClient({
    token: 'writer-token', repository: REPOSITORY,
    fetchImpl: async (url, options) => {
      calls.push({ url, options });
      const body = url.includes('/installation/repositories') ? {
        total_count: 1,
        repositories: [{ id: REPOSITORY_ID, full_name: REPOSITORY, owner: { login: 'JinPengGeng' } }],
      } : {
        login: WRITER_BOT.login,
        id: WRITER_BOT.databaseId,
        node_id: WRITER_BOT.nodeId,
        type: 'Bot',
        site_admin: false,
      };
      return {
        ok: true,
        status: 200,
        text: async () => JSON.stringify(body),
      };
    },
  });
  await client.getInstallationRepositories();
  await client.getUser(WRITER_BOT.login);
  assert.equal(calls.length, 2);
  assert.match(calls[0].url, /\/installation\/repositories\?per_page=100&page=1$/);
  assert.equal(calls[0].options.method, 'GET');
  assert.match(calls[1].url, /\/users\/aeris-writer%5Bbot%5D$/);
  assert.equal(calls[1].options.method, 'GET');
});

test('mutating CLI routes protection proof through the Writer client', async () => {
  const readClient = new FakeClient();
  const writerClient = new FakeClient();
  const originalMarkReady = writerClient.markPullReady.bind(writerClient);
  writerClient.markPullReady = async (...args) => {
    const result = await originalMarkReady(...args);
    readClient.ready = writerClient.ready;
    return result;
  };
  const originalEnableAutoMerge = writerClient.enableAutoMerge.bind(writerClient);
  writerClient.enableAutoMerge = async (...args) => {
    const result = await originalEnableAutoMerge(...args);
    readClient.armed = writerClient.armed;
    return result;
  };
  readClient.getBranchProtection = async () => {
    throw new Error('Actions token must not read protected repository governance');
  };
  const result = await runAutonomyFinalizer(fullEnvironment(), {
    readClient,
    writerClient,
    sleepImpl: async () => {},
  });
  assert.equal(result.action, 'armed');
  assert.ok(writerClient.protectionReads >= 3);
  assert.equal(readClient.protectionReads, 0);
});

test('full proof failure occurs before every Writer and hold mutation', async () => {
  const readClient = new FakeClient();
  const writerClient = new FakeClient();
  writerClient.getBranchProtection = async () => {
    writerClient.protectionReads += 1;
    throw new Error('Writer administration read denied');
  };
  await assert.rejects(
    () => runAutonomyFinalizer(fullEnvironment(), {
      readClient,
      writerClient,
      sleepImpl: async () => {},
    }),
    /Writer administration read denied/,
  );
  assert.equal(writerClient.protectionReads, 1);
  assert.equal(readClient.protectionReads, 0);
  assert.equal(readClient.hold, null);
  assert.deepEqual(readClient.events, []);
  assert.deepEqual(writerClient.events, []);
});

test('hold check REST methods bind creation and completion to one exact check run', async () => {
  const calls = [];
  const responseCheck = {
    id: 91, name: 'Autonomy Finalizer / hold', head_sha: HEAD_SHA,
    external_id: `aeris-finalizer-hold:v1:${REPOSITORY_ID}:${PULL_NUMBER}:${HEAD_SHA}`,
    status: 'in_progress', conclusion: null,
    app: { id: 15368, slug: 'github-actions' }, pull_requests: [{ number: PULL_NUMBER }],
  };
  const client = new AutonomyFinalizerGitHubClient({
    token: 'actions-token', repository: REPOSITORY,
    fetchImpl: async (url, options) => {
      calls.push({ url, options, body: options.body ? JSON.parse(options.body) : null });
      const value = options.method === 'PATCH'
        ? { ...responseCheck, status: 'completed', conclusion: 'success' }
        : responseCheck;
      return { ok: true, status: 200, text: async () => JSON.stringify(value) };
    },
  });
  await client.createHoldCheck(HEAD_SHA, responseCheck.external_id);
  await client.getCheckRun(91);
  await client.completeHoldCheck(91, responseCheck.external_id);

  assert.match(calls[0].url, /\/repos\/JinPengGeng\/aeris-token\/check-runs$/);
  assert.deepEqual(
    { name: calls[0].body.name, head_sha: calls[0].body.head_sha, external_id: calls[0].body.external_id, status: calls[0].body.status },
    { name: 'Autonomy Finalizer / hold', head_sha: HEAD_SHA, external_id: responseCheck.external_id, status: 'in_progress' },
  );
  assert.match(calls[1].url, /\/check-runs\/91$/);
  assert.equal(calls[1].options.method, 'GET');
  assert.equal(calls[2].options.method, 'PATCH');
  assert.equal(calls[2].body.external_id, responseCheck.external_id);
  assert.equal(calls[2].body.status, 'completed');
  assert.equal(calls[2].body.conclusion, 'success');
  assert.match(calls[2].body.completed_at, /^\d{4}-\d{2}-\d{2}T/);
});
