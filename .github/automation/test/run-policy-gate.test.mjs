import assert from 'node:assert/strict';
import test from 'node:test';

import { collectPolicyEvaluation, publishPolicyEvaluation } from '../src/run-policy-gate.mjs';

const sha = (character) => character.repeat(40);
const repository = 'JinPengGeng/aeris-token';

function contracts(mode = 'shadow') {
  return {
    agents: { agents: { policy: { enabled: true } } },
    policy: {
      trusted_source: { ref: 'refs/heads/main' },
      policy_gate: {
        enabled: true,
        check_name: 'Automation Policy / gate',
        mode,
        human_enable_label: 'automerge-approved',
        require_exact_head_sha: true,
        require_base_up_to_date: true,
        require_conversation_resolution: true,
        required_checks: ['Rust CI / check', 'Frontend CI / check'],
        required_check_sources: [
          { context: 'Rust CI / check', app_id: 15368, app_slug: 'github-actions' },
          { context: 'Frontend CI / check', app_id: 15368, app_slug: 'github-actions' },
        ],
        always_require_human_review: ['.github/**'],
        allowlist_paths: [],
      },
    },
  };
}

function fakeClient(overrides = {}) {
  const pull = {
    number: 37,
    state: 'open',
    draft: false,
    mergeable: true,
    labels: [],
    head: { sha: sha('a'), ref: 'feature', repo: { full_name: repository } },
    base: { sha: sha('b'), ref: 'main', repo: { full_name: repository } },
  };
  return {
    getRepository: async () => ({ id: 123, full_name: repository, default_branch: 'main' }),
    getBranchHead: async () => sha('c'),
    getPull: async () => pull,
    listPullFiles: async () => ({
      files: [{ filename: 'docs/policy.md', status: 'modified', previous_filename: null }],
      truncated: false,
    }),
    listCheckRunsForRef: async () => [
      { id: 1, name: 'Rust CI / check', head_sha: sha('a'), status: 'completed', conclusion: 'success', app: { id: 15368, slug: 'github-actions' } },
      { id: 2, name: 'Frontend CI / check', head_sha: sha('a'), status: 'completed', conclusion: 'success', app: { id: 15368, slug: 'github-actions' } },
    ],
    compare: async () => ({ base_sha: sha('b'), head_sha: sha('a'), status: 'ahead' }),
    listReviewThreads: async () => ({ unresolved: 0, truncated: false, head_sha: sha('a'), base_sha: sha('b') }),
    beginPolicyCheck: async () => ({ id: 77, html_url: 'https://github.com/JinPengGeng/aeris-token/runs/77' }),
    restorePolicyCheckInProgress: async () => ({ id: 77 }),
    completePolicyCheck: async () => ({ id: 77, html_url: 'https://github.com/JinPengGeng/aeris-token/runs/77' }),
    ...overrides,
  };
}

test('collector binds live main, pull, checks, comparison, and review threads', async () => {
  const artifact = await collectPolicyEvaluation({
    client: fakeClient(),
    contracts: contracts(),
    repository,
    repositoryId: 123,
    pullNumber: 37,
    policySha: sha('c'),
    expectedHeadSha: sha('a'),
    clock: () => new Date('2026-08-18T12:00:00Z'),
  });
  assert.equal(artifact.head_sha, sha('a'));
  assert.equal(artifact.policy_sha, sha('c'));
  assert.equal(artifact.result.verdict, 'pass');
  assert.equal(artifact.result.eligible_for_automatic_merge, false);
});

test('collector rejects stale policy checkout and review-thread snapshots', async () => {
  await assert.rejects(() => collectPolicyEvaluation({
    client: fakeClient({ getBranchHead: async () => sha('d') }),
    contracts: contracts(), repository, repositoryId: 123, pullNumber: 37, policySha: sha('c'),
  }), /checkout is stale/);

  await assert.rejects(() => collectPolicyEvaluation({
    client: fakeClient({ listReviewThreads: async () => ({ unresolved: 0, truncated: false, head_sha: sha('d'), base_sha: sha('b') }) }),
    contracts: contracts(), repository, repositoryId: 123, pullNumber: 37, policySha: sha('c'),
  }), /snapshot is stale/);
});

test('collector binds the executing checkout to the expected Policy SHA', async () => {
  await assert.rejects(() => collectPolicyEvaluation({
    client: fakeClient(),
    contracts: contracts(),
    repository,
    repositoryId: 123,
    pullNumber: 37,
    policySha: sha('c'),
    expectedPolicySha: sha('d'),
  }), /expected policy SHA/);
});

test('publisher re-collects the live generation and emits a bounded receipt', async () => {
  let receivedFenceId = null;
  const client = fakeClient({
    beginPolicyCheck: async (_generation, _checkName, _detailsUrl, expectedFenceId) => {
      receivedFenceId = expectedFenceId;
      return { id: 77, html_url: 'https://github.com/JinPengGeng/aeris-token/runs/77' };
    },
  });
  const receipt = await publishPolicyEvaluation({
    client,
    contracts: contracts(),
    repository,
    repositoryId: 123,
    pullNumber: 37,
    policySha: sha('c'),
    expectedHeadSha: sha('a'),
    policyApp: { id: 9001, slug: 'aeris-token-policy' },
    runId: '321.1',
    detailsUrl: 'https://github.com/JinPengGeng/aeris-token/actions/runs/321',
    expectedFenceCheckRunId: 77,
    clock: () => new Date('2026-08-18T12:01:00Z'),
  });
  assert.equal(receipt.state, 'published');
  assert.equal(receipt.check_run_id, 77);
  assert.equal(receipt.conclusion, 'neutral');
  assert.equal(receivedFenceId, 77);
});

test('publisher rejects duplicate current-generation checks before any mutation', async () => {
  const mutationMethods = [];
  const currentExternalId = `aeris-policy:v1:123:37:${sha('a')}:${sha('c')}`;
  const base = fakeClient();
  const client = fakeClient({
    listCheckRunsForRef: async () => [
      ...(await base.listCheckRunsForRef()),
      { id: 77, external_id: currentExternalId, status: 'in_progress', conclusion: null },
      { id: 88, external_id: currentExternalId, status: 'in_progress', conclusion: null },
    ],
    beginPolicyCheck: async () => { mutationMethods.push('POST/PATCH'); throw new Error('must not begin'); },
    completePolicyCheck: async () => { mutationMethods.push('PATCH'); throw new Error('must not complete'); },
    restorePolicyCheckInProgress: async () => { mutationMethods.push('PATCH'); throw new Error('must not restore'); },
  });

  await assert.rejects(() => publishPolicyEvaluation({
    client,
    contracts: contracts(),
    repository,
    repositoryId: 123,
    pullNumber: 37,
    policySha: sha('c'),
    expectedHeadSha: sha('a'),
    policyApp: { id: 9001, slug: 'aeris-token-policy' },
    runId: '321.1',
    detailsUrl: 'https://github.com/JinPengGeng/aeris-token/actions/runs/321',
  }), /multiple in-progress policy checks exist for the current generation/);
  assert.deepEqual(mutationMethods, []);
});

test('publisher accepts completed history for the current generation', async () => {
  const currentExternalId = `aeris-policy:v1:123:37:${sha('a')}:${sha('c')}`;
  const base = fakeClient();
  let begins = 0;
  const client = fakeClient({
    listCheckRunsForRef: async () => [
      ...(await base.listCheckRunsForRef()),
      { id: 70, external_id: currentExternalId, status: 'completed', conclusion: 'success' },
      { id: 71, external_id: currentExternalId, status: 'completed', conclusion: 'neutral' },
    ],
    beginPolicyCheck: async () => {
      begins += 1;
      return { id: 77, html_url: 'https://github.com/JinPengGeng/aeris-token/runs/77' };
    },
  });

  const receipt = await publishPolicyEvaluation({
    client,
    contracts: contracts(),
    repository,
    repositoryId: 123,
    pullNumber: 37,
    policySha: sha('c'),
    expectedHeadSha: sha('a'),
    policyApp: { id: 9001, slug: 'aeris-token-policy' },
    runId: '321.1',
    detailsUrl: 'https://github.com/JinPengGeng/aeris-token/actions/runs/321',
  });
  assert.equal(receipt.state, 'published');
  assert.equal(begins, 1);
});

test('publisher rejects an unexpected live head before any mutation', async () => {
  const mutationMethods = [];
  const client = fakeClient({
    beginPolicyCheck: async () => { mutationMethods.push('POST/PATCH'); throw new Error('must not begin'); },
    completePolicyCheck: async () => { mutationMethods.push('PATCH'); throw new Error('must not complete'); },
    restorePolicyCheckInProgress: async () => { mutationMethods.push('PATCH'); throw new Error('must not restore'); },
  });

  await assert.rejects(() => publishPolicyEvaluation({
    client,
    contracts: contracts(),
    repository,
    repositoryId: 123,
    pullNumber: 37,
    policySha: sha('c'),
    expectedHeadSha: sha('d'),
    policyApp: { id: 9001, slug: 'aeris-token-policy' },
    runId: '321.1',
    detailsUrl: 'https://github.com/JinPengGeng/aeris-token/actions/runs/321',
  }), /pull head SHA changed/);
  assert.deepEqual(mutationMethods, []);
});

test('publisher requires an expected head before reading or mutating live state', async () => {
  let calls = 0;
  const client = new Proxy({}, {
    get() {
      calls += 1;
      throw new Error('client must not be accessed');
    },
  });

  await assert.rejects(() => publishPolicyEvaluation({
    client,
    contracts: contracts(),
    repository,
    repositoryId: 123,
    pullNumber: 37,
    policySha: sha('c'),
    policyApp: { id: 9001, slug: 'aeris-token-policy' },
    runId: '321.1',
    detailsUrl: 'https://github.com/JinPengGeng/aeris-token/actions/runs/321',
  }), /expected PR head SHA is required/);
  assert.equal(calls, 0);
});

test('publisher restores an in-progress fence when completion postconditions fail', async () => {
  let begins = 0;
  let restores = 0;
  let completions = 0;
  const client = fakeClient({
    beginPolicyCheck: async () => { begins += 1; return { id: 77 }; },
    restorePolicyCheckInProgress: async () => { restores += 1; return { id: 77 }; },
    completePolicyCheck: async () => { completions += 1; throw new Error('persisted check reread failed'); },
  });
  await assert.rejects(() => publishPolicyEvaluation({
    client,
    contracts: contracts(),
    repository,
    repositoryId: 123,
    pullNumber: 37,
    policySha: sha('c'),
    expectedHeadSha: sha('a'),
    policyApp: { id: 9001, slug: 'aeris-token-policy' },
    runId: '321.1',
    detailsUrl: 'https://github.com/JinPengGeng/aeris-token/actions/runs/321',
  }), /persisted check reread failed/);
  assert.equal(begins, 1);
  assert.equal(restores, 1);
  assert.equal(completions, 1);
});

test('publisher leaves the check in progress when policy inputs do not stabilize', async () => {
  let begins = 0;
  let restores = 0;
  let completions = 0;
  let checkReads = 0;
  const base = fakeClient();
  const client = fakeClient({
    listCheckRunsForRef: async () => {
      checkReads += 1;
      return (await base.listCheckRunsForRef()).map((check) => ({ ...check, id: check.id + checkReads * 10 }));
    },
    beginPolicyCheck: async () => { begins += 1; return { id: 77 }; },
    restorePolicyCheckInProgress: async () => { restores += 1; return { id: 77 }; },
    completePolicyCheck: async () => { completions += 1; return { id: 77 }; },
  });
  await assert.rejects(() => publishPolicyEvaluation({
    client,
    contracts: contracts(),
    repository,
    repositoryId: 123,
    pullNumber: 37,
    policySha: sha('c'),
    expectedHeadSha: sha('a'),
    policyApp: { id: 9001, slug: 'aeris-token-policy' },
    runId: '321.1',
    detailsUrl: 'https://github.com/JinPengGeng/aeris-token/actions/runs/321',
  }), /stable snapshot/);
  assert.equal(begins, 1);
  assert.equal(restores, 1);
  assert.equal(completions, 0);
});

test('publisher fences a moved head before completing the check', async () => {
  const stable = fakeClient();
  let reads = 0;
  let begins = 0;
  let restores = 0;
  let completions = 0;
  const moved = fakeClient({
    getPull: async () => {
      reads += 1;
      const pull = await stable.getPull();
      return reads === 1 ? pull : {
        ...pull,
        head: { sha: sha('d'), ref: 'feature', repo: { full_name: repository } },
      };
    },
    beginPolicyCheck: async () => { begins += 1; return { id: 77 }; },
    restorePolicyCheckInProgress: async () => { restores += 1; return { id: 77 }; },
    completePolicyCheck: async () => { completions += 1; return { id: 77 }; },
  });
  await assert.rejects(() => publishPolicyEvaluation({
    client: moved,
    contracts: contracts(),
    repository,
    repositoryId: 123,
    pullNumber: 37,
    policySha: sha('c'),
    expectedHeadSha: sha('a'),
    policyApp: { id: 9001, slug: 'aeris-token-policy' },
    runId: '321.1',
    detailsUrl: 'https://github.com/JinPengGeng/aeris-token/actions/runs/321',
  }), /head SHA changed/);
  assert.equal(begins, 1);
  assert.equal(restores, 1);
  assert.equal(completions, 0);
});

test('publisher restores the same fenced check after the completed check observes a moved head', async () => {
  let restores = 0;
  let completions = 0;
  const client = fakeClient({
    listCheckRunsForRef: async () => {
      const runs = [
        { id: 1, name: 'Rust CI / check', head_sha: sha('a'), status: 'completed', conclusion: 'success', app: { id: 15368, slug: 'github-actions' } },
        { id: 2, name: 'Frontend CI / check', head_sha: sha('a'), status: 'completed', conclusion: 'success', app: { id: 15368, slug: 'github-actions' } },
      ];
      return completions > 0 ? runs.map((check) => ({ ...check, id: check.id + 10 })) : runs;
    },
    completePolicyCheck: async () => { completions += 1; return { id: 77, html_url: 'https://github.com/JinPengGeng/aeris-token/runs/77' }; },
    restorePolicyCheckInProgress: async (checkRunId, generation, checkName) => {
      restores += 1;
      assert.equal(checkRunId, 77);
      assert.equal(generation.head_sha, sha('a'));
      assert.equal(checkName, 'Automation Policy / gate');
      return { id: 77 };
    },
  });
  await assert.rejects(() => publishPolicyEvaluation({
    client,
    contracts: contracts(),
    repository,
    repositoryId: 123,
    pullNumber: 37,
    policySha: sha('c'),
    expectedHeadSha: sha('a'),
    policyApp: { id: 9001, slug: 'aeris-token-policy' },
    runId: '321.1',
    detailsUrl: 'https://github.com/JinPengGeng/aeris-token/actions/runs/321',
  }), /policy inputs changed during check completion/);
  assert.equal(completions, 1);
  assert.equal(restores, 1);
});

test('publisher restores success when a review thread becomes unresolved after completion', async () => {
  let threadReads = 0;
  let completions = 0;
  let restores = 0;
  const client = fakeClient({
    listReviewThreads: async () => {
      threadReads += 1;
      return { unresolved: threadReads >= 3 ? 1 : 0, truncated: false, head_sha: sha('a'), base_sha: sha('b') };
    },
    completePolicyCheck: async () => {
      completions += 1;
      return { id: 77, html_url: 'https://github.com/JinPengGeng/aeris-token/runs/77' };
    },
    restorePolicyCheckInProgress: async () => {
      restores += 1;
      return { id: 77 };
    },
  });
  await assert.rejects(() => publishPolicyEvaluation({
    client,
    contracts: contracts('human'),
    repository,
    repositoryId: 123,
    pullNumber: 37,
    policySha: sha('c'),
    expectedHeadSha: sha('a'),
    policyApp: { id: 9001, slug: 'aeris-token-policy' },
    runId: '321.1',
    detailsUrl: 'https://github.com/JinPengGeng/aeris-token/actions/runs/321',
  }), /policy inputs changed during check completion/);
  assert.equal(completions, 1);
  assert.equal(restores, 1);
});

test('post-success hard abort plus recovery failure is reported as an explicit platform residual', async () => {
  let repositoryReads = 0;
  let completions = 0;
  const base = fakeClient();
  const client = fakeClient({
    getRepository: async () => {
      repositoryReads += 1;
      if (repositoryReads >= 4) throw new Error('GitHub API request timed out');
      return base.getRepository();
    },
    completePolicyCheck: async () => {
      completions += 1;
      return { id: 77, html_url: 'https://github.com/JinPengGeng/aeris-token/runs/77' };
    },
    restorePolicyCheckInProgress: async () => {
      throw new Error('GitHub API request timed out during recovery');
    },
  });
  let error;
  try {
    await publishPolicyEvaluation({
      client,
      contracts: contracts('human'),
      repository,
      repositoryId: 123,
      pullNumber: 37,
      policySha: sha('c'),
      expectedHeadSha: sha('a'),
      policyApp: { id: 9001, slug: 'aeris-token-policy' },
      runId: '321.1',
      detailsUrl: 'https://github.com/JinPengGeng/aeris-token/actions/runs/321',
    });
    assert.fail('publisher unexpectedly succeeded');
  } catch (caught) {
    error = caught;
  }
  assert.equal(error instanceof AggregateError, true);
  assert.match(error.message, /could not be restored/);
  assert.equal(completions, 1);
});
