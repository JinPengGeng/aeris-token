import assert from 'node:assert/strict';
import test from 'node:test';

import {
  AutonomyFinalizerError,
  AutonomyFinalizerGitHubClient,
  evaluateAutonomyFinalizer,
  finalizeAutonomyPull,
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

const trigger = Object.freeze({ run_id: 77, run_attempt: 1 });
const trust = Object.freeze({
  repository: REPOSITORY,
  repository_id: REPOSITORY_ID,
  default_branch: 'main',
  policy_ref: 'main',
  policy_sha: BASE_SHA,
});
const config = Object.freeze({
  repository: REPOSITORY,
  base_ref: 'main',
  writer_login: 'aeris-writer[bot]',
  branch_prefix: 'agent/issue-',
  maximum_files: 20,
  maximum_changes: 2000,
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
    baseRefName: 'main', baseRefOid: BASE_SHA, author: config.writer_login,
    mergeable: 'MERGEABLE', mergeStateStatus: 'DRAFT', reviewDecision: null,
    autoMergeRequest: null, labels: [], reviewThreads: [], ...overrides,
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
    this.draftRestores = 0;
    this.ready = false;
    this.armed = false;
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
    const override = typeof this.overrides.governance === 'function'
      ? this.overrides.governance(this.governanceReads, this)
      : this.overrides.governance;
    return governance({
      isDraft: !this.ready,
      mergeStateStatus: this.ready ? 'CLEAN' : 'DRAFT',
      autoMergeRequest: this.armed ? { enabledAt: '2026-08-20T00:00:00Z', mergeMethod: 'SQUASH' } : null,
      ...override,
    });
  }
  async listCheckRunsForRef() {
    return this.overrides.checks ?? [
      check('Rust CI / check', 1), check('Frontend CI / check', 2), check('Automation Policy / gate', 3),
    ];
  }
  async markPullReady(id) {
    this.ready = true;
    if (this.overrides.readyError) throw this.overrides.readyError;
    return this.overrides.readyResult ?? { id, isDraft: false, headRefOid: HEAD_SHA };
  }
  async convertPullToDraft(id) {
    this.draftRestores += 1;
    this.ready = false;
    this.armed = false;
    if (this.overrides.draftError) throw this.overrides.draftError;
    return { id, isDraft: true, headRefOid: HEAD_SHA };
  }
  async enableAutoMerge(id, head) {
    this.armed = true;
    return { id, headRefOid: head, autoMergeRequest: { enabledAt: '2026-08-20T00:00:00Z', mergeMethod: 'SQUASH' } };
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
  assert.ok(client.pullReads >= 4);
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
    headRepository: { nameWithOwner: REPOSITORY }, author: { login: config.writer_login },
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
  ];
  for (const pullValue of invalid) {
    await assert.rejects(
      () => graphqlClient([pullValue]).getPullGovernance(PULL_NUMBER),
      AutonomyFinalizerError,
    );
  }
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
});
