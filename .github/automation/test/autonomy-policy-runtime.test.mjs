import assert from 'node:assert/strict';
import test from 'node:test';

import {
  AutonomyPolicyRuntimeError,
  buildAutonomyPolicySnapshot,
  evaluateAutonomyPolicy,
} from '../src/autonomy-policy-runtime.mjs';

const HEAD_SHA = 'b'.repeat(40);
const BASE_SHA = 'a'.repeat(40);
const HEAD_TREE = 'c'.repeat(40);
const BASE_TREE = 'd'.repeat(40);
const DOCS_TREE = 'e'.repeat(40);
const CANARY_TREE = 'f'.repeat(40);

const config = Object.freeze({
  repository: 'JinPengGeng/aeris-token',
  base_ref: 'main',
  writer_login: 'aeris-writer[bot]',
  branch_prefix: 'agent/issue-',
  maximum_files: 20,
  maximum_changes: 2000,
});

const trigger = Object.freeze({ pull_number: 17, head_sha: HEAD_SHA });
const trust = Object.freeze({
  repository: config.repository,
  repository_id: 1310462380,
  default_branch: 'main',
  policy_ref: 'main',
  policy_sha: BASE_SHA,
});

function pull(overrides = {}) {
  return {
    number: 17,
    state: 'open',
    user: { login: config.writer_login },
    head: { ref: 'agent/issue-17', sha: HEAD_SHA, repo: { full_name: config.repository } },
    base: { ref: 'main', sha: BASE_SHA, repo: { full_name: config.repository } },
    ...overrides,
  };
}

function changedFile(filename, overrides = {}) {
  return {
    filename,
    status: 'modified',
    additions: 2,
    deletions: 1,
    changes: 3,
    patch: '@@ -1 +1,2 @@',
    ...overrides,
  };
}

function tree(sha, entries, overrides = {}) {
  return { sha, truncated: false, tree: entries, ...overrides };
}

function entry(path, mode, type, sha) {
  return { path, mode, type, sha };
}

function pullLabel(name, id = 1) {
  return { id, name };
}

class FakePolicyClient {
  constructor(options = {}) {
    this.options = options;
    this.pullReads = 0;
    this.pages = [];
    this.labelRound = -1;
    this.labelPages = [];
  }

  async getRepository() {
    if (this.options.repositoryError) throw this.options.repositoryError;
    return this.options.repository ?? { id: trust.repository_id, full_name: trust.repository, default_branch: 'main' };
  }

  async getPull() {
    const values = this.options.pulls ?? [pull(), pull()];
    return values[Math.min(this.pullReads++, values.length - 1)];
  }

  async getGitRef() {
    return this.options.baseRef ?? { object: { sha: BASE_SHA } };
  }

  async getPullFilePage(_number, page, perPage) {
    this.pages.push({ page, perPage });
    const pages = this.options.filePages ?? [[changedFile('docs/automation-canary/example.md')]];
    return pages[page - 1] ?? [];
  }

  async getPullLabelPage(_number, page, perPage) {
    if (this.options.labelError) throw this.options.labelError;
    if (page === 1) this.labelRound += 1;
    this.labelPages.push({ round: this.labelRound, page, perPage });
    const snapshots = this.options.labelSnapshots ?? [[[]], [[]]];
    const pages = snapshots[Math.min(this.labelRound, snapshots.length - 1)];
    return pages?.[page - 1];
  }

  async getGitCommit(sha) {
    return { sha, tree: { sha: sha === HEAD_SHA ? HEAD_TREE : BASE_TREE } };
  }

  async getGitTree(sha) {
    const values = this.options.trees ?? new Map([
      [HEAD_TREE, tree(HEAD_TREE, [entry('docs', '040000', 'tree', DOCS_TREE)])],
      [BASE_TREE, tree(BASE_TREE, [])],
      [DOCS_TREE, tree(DOCS_TREE, [entry('automation-canary', '040000', 'tree', CANARY_TREE)])],
      [CANARY_TREE, tree(CANARY_TREE, [entry('example.md', '100644', 'blob', '1'.repeat(40))])],
    ]);
    return values.get(sha);
  }

}

test('builds a complete snapshot from bounded pages and exact commit trees', async () => {
  const trees = new Map([
    [HEAD_TREE, tree(HEAD_TREE, [entry('docs', '040000', 'tree', DOCS_TREE)])],
    [BASE_TREE, tree(BASE_TREE, [])],
    [DOCS_TREE, tree(DOCS_TREE, [entry('automation-canary', '040000', 'tree', CANARY_TREE)])],
    [CANARY_TREE, tree(CANARY_TREE, [
      entry('first.md', '100644', 'blob', '1'.repeat(40)),
      entry('second.md', '100644', 'blob', '2'.repeat(40)),
      entry('third.md', '100644', 'blob', '3'.repeat(40)),
    ])],
  ]);
  const client = new FakePolicyClient({
    trees,
    filePages: [
      [changedFile('docs/automation-canary/first.md'), changedFile('docs/automation-canary/second.md')],
      [changedFile('docs/automation-canary/third.md')],
    ],
  });
  const snapshot = await buildAutonomyPolicySnapshot({
    client, trigger, trust, config, limits: { maximumFiles: 4, pageSize: 2 },
  });

  assert.deepEqual(client.pages, [{ page: 1, perPage: 2 }, { page: 2, perPage: 2 }]);
  assert.deepEqual(snapshot, {
    repository: config.repository,
    base: { ref: 'main', sha: BASE_SHA },
    head: { ref: 'agent/issue-17', sha: HEAD_SHA },
    source: { author: config.writer_login, branch: 'agent/issue-17', repository: config.repository },
    labels: [],
    labels_truncated: false,
    files: [
      { filename: 'docs/automation-canary/first.md', status: 'modified', additions: 2, deletions: 1, changes: 3, mode: '100644', binary: false },
      { filename: 'docs/automation-canary/second.md', status: 'modified', additions: 2, deletions: 1, changes: 3, mode: '100644', binary: false },
      { filename: 'docs/automation-canary/third.md', status: 'modified', additions: 2, deletions: 1, changes: 3, mode: '100644', binary: false },
    ],
    truncated: false,
  });
  assert.ok(Object.isFrozen(snapshot));
  assert.ok(Object.isFrozen(snapshot.files));
});

test('rejects excess pagination instead of classifying an incomplete file list', async () => {
  const client = new FakePolicyClient({
    filePages: [
      [changedFile('docs/automation-canary/example.md')],
      [changedFile('docs/automation-canary/other.md')],
    ],
  });
  await assert.rejects(
    () => buildAutonomyPolicySnapshot({ client, trigger, trust, config, limits: { maximumFiles: 1, pageSize: 1 } }),
    (error) => error instanceof AutonomyPolicyRuntimeError && /exceed the policy snapshot limit/.test(error.message),
  );
});

test('builds a complete bounded label snapshot and rechecks it after file reads', async () => {
  const labels = [pullLabel('autonomy-manual', 2), pullLabel('needs-review', 1)];
  const client = new FakePolicyClient({
    labelSnapshots: [
      [[labels[0], labels[1]], []],
      [[labels[0], labels[1]], []],
    ],
  });
  const snapshot = await buildAutonomyPolicySnapshot({
    client, trigger, trust, config, limits: { maximumLabels: 4, labelPageSize: 2 },
  });
  assert.deepEqual(snapshot.labels, [pullLabel('needs-review', 1), pullLabel('autonomy-manual', 2)]);
  assert.equal(snapshot.labels_truncated, false);
  assert.deepEqual(client.labelPages, [
    { round: 0, page: 1, perPage: 2 }, { round: 0, page: 2, perPage: 2 },
    { round: 1, page: 1, perPage: 2 }, { round: 1, page: 2, perPage: 2 },
  ]);
});

test('rejects incomplete, malformed, duplicate, and drifted label snapshots', async () => {
  await assert.rejects(
    () => buildAutonomyPolicySnapshot({
      client: new FakePolicyClient({ labelError: new Error('label endpoint unavailable') }), trigger, trust, config,
    }),
    /label endpoint unavailable/,
  );
  await assert.rejects(
    () => buildAutonomyPolicySnapshot({
      client: new FakePolicyClient({ labelSnapshots: [[[pullLabel('one')], [pullLabel('two', 2)]]] }),
      trigger, trust, config, limits: { maximumLabels: 1, labelPageSize: 1 },
    }),
    /pull labels exceed the policy snapshot limit/,
  );
  await assert.rejects(
    () => buildAutonomyPolicySnapshot({
      client: new FakePolicyClient({ labelSnapshots: [[[{ id: 1 }]]] }), trigger, trust, config,
    }),
    /pull labels\[0\]\.name is invalid/,
  );
  await assert.rejects(
    () => buildAutonomyPolicySnapshot({
      client: new FakePolicyClient({ labelSnapshots: [[[pullLabel('one'), pullLabel('ONE', 2)]]] }), trigger, trust, config,
    }),
    /duplicate name/,
  );
  await assert.rejects(
    () => buildAutonomyPolicySnapshot({
      client: new FakePolicyClient({ labelSnapshots: [[[pullLabel('one')]], [[]]] }), trigger, trust, config,
    }),
    /labels drifted while the policy snapshot was built/,
  );
  await assert.rejects(
    () => buildAutonomyPolicySnapshot({
      client: new FakePolicyClient({ labelSnapshots: [[null]] }), trigger, trust, config,
    }),
    /pull labels response is invalid/,
  );
});

test('rejects trigger mismatch, base drift, and PR drift', async () => {
  await assert.rejects(
    () => buildAutonomyPolicySnapshot({ client: new FakePolicyClient(), trigger: { ...trigger, head_sha: '9'.repeat(40) }, trust, config }),
    /head SHA does not match the trigger/,
  );
  await assert.rejects(
    () => buildAutonomyPolicySnapshot({ client: new FakePolicyClient({ baseRef: { object: { sha: '8'.repeat(40) } } }), trigger, trust, config }),
    /not identical/,
  );
  await assert.rejects(
    () => buildAutonomyPolicySnapshot({
      client: new FakePolicyClient({ pulls: [pull(), pull({ head: { ...pull().head, sha: '7'.repeat(40) } })] }),
      trigger, trust, config,
    }),
    /drifted while the policy snapshot was built/,
  );
});

test('requires policy code at the exact current default-branch SHA', async () => {
  await assert.rejects(
    () => buildAutonomyPolicySnapshot({ client: new FakePolicyClient(), trigger, trust: { ...trust, policy_ref: 'release/v1' }, config }),
    /not sourced from the default branch/,
  );
  await assert.rejects(
    () => buildAutonomyPolicySnapshot({ client: new FakePolicyClient(), trigger, trust: { ...trust, policy_sha: '6'.repeat(40) }, config }),
    /not identical/,
  );
  await assert.rejects(
    () => buildAutonomyPolicySnapshot({
      client: new FakePolicyClient({ repository: { id: 999, full_name: trust.repository, default_branch: 'main' } }),
      trigger, trust, config,
    }),
    /identity does not match/,
  );
});

test('uses exact tree modes and treats missing patches as binary', async () => {
  const trees = new Map([
    [HEAD_TREE, tree(HEAD_TREE, [entry('docs', '040000', 'tree', DOCS_TREE)])],
    [BASE_TREE, tree(BASE_TREE, [])],
    [DOCS_TREE, tree(DOCS_TREE, [entry('automation-canary', '040000', 'tree', CANARY_TREE)])],
    [CANARY_TREE, tree(CANARY_TREE, [entry('example.md', '120000', 'blob', '1'.repeat(40))])],
  ]);
  const snapshot = await buildAutonomyPolicySnapshot({
    client: new FakePolicyClient({ trees, filePages: [[changedFile('docs/automation-canary/example.md', { patch: undefined })]] }),
    trigger, trust, config,
  });
  assert.equal(snapshot.files[0].mode, '120000');
  assert.equal(snapshot.files[0].binary, true);

  trees.set(CANARY_TREE, tree(CANARY_TREE, [], { truncated: true }));
  await assert.rejects(
    () => buildAutonomyPolicySnapshot({
      client: new FakePolicyClient({ trees }), trigger, trust, config,
    }),
    /tree response is truncated/,
  );
});

test('successful evaluation returns the classifier result without publishing a custom check', async () => {
  const client = new FakePolicyClient();
  const result = await evaluateAutonomyPolicy({ client, trigger, trust, config });
  assert.deepEqual(result.decision, { classification: 'eligible', reasons: [] });
  assert.equal(result.snapshot.head.sha, HEAD_SHA);
  assert.equal(client.pullReads, 2);
});

test('adding and removing a manual-hold label changes the managed policy decision', async () => {
  const held = await evaluateAutonomyPolicy({
    client: new FakePolicyClient({
      labelSnapshots: [[[pullLabel('autonomy-manual')]], [[pullLabel('autonomy-manual')]]],
    }),
    trigger, trust, config,
  });
  assert.deepEqual(held.decision, { classification: 'deny', reasons: ['deny_autonomy_manual_label'] });

  const released = await evaluateAutonomyPolicy({ client: new FakePolicyClient(), trigger, trust, config });
  assert.deepEqual(released.decision, { classification: 'eligible', reasons: [] });
});

test('an unmanaged human pull is manual without paginating or reading candidate trees', async () => {
  const human = pull({
    user: { login: 'maintainer' },
    head: { ref: 'feature/human', sha: HEAD_SHA, repo: { full_name: config.repository } },
  });
  const client = new FakePolicyClient({ pulls: [human, human] });
  const result = await evaluateAutonomyPolicy({ client, trigger, trust, config });
  assert.deepEqual(result.decision, { classification: 'manual', reasons: ['manual_unmanaged_branch'] });
  assert.deepEqual(client.pages, []);
  assert.deepEqual(result.snapshot.files, []);
});

test('an unmanaged human pull is denied only by the global do-not-merge label', async () => {
  const human = pull({
    user: { login: 'maintainer' },
    head: { ref: 'feature/human', sha: HEAD_SHA, repo: { full_name: config.repository } },
  });
  const client = new FakePolicyClient({
    pulls: [human, human],
    labelSnapshots: [[[pullLabel('do-not-merge')]], [[pullLabel('do-not-merge')]]],
  });
  const result = await evaluateAutonomyPolicy({ client, trigger, trust, config });
  assert.deepEqual(result.decision, {
    classification: 'deny', reasons: ['deny_do_not_merge_label', 'manual_unmanaged_branch'],
  });
});

test('an external contributor pull is manual even when its branch resembles a managed branch', async () => {
  const external = pull({
    user: { login: 'contributor' },
    head: { ref: 'agent/issue-17', sha: HEAD_SHA, repo: { full_name: 'outside/fork' } },
  });
  const client = new FakePolicyClient({ pulls: [external, external] });
  const result = await evaluateAutonomyPolicy({ client, trigger, trust, config });
  assert.deepEqual(result.decision, { classification: 'manual', reasons: ['manual_external_head_repository'] });
  assert.deepEqual(client.pages, []);
});
