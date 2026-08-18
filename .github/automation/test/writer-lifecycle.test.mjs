import assert from 'node:assert/strict';
import test from 'node:test';

import { evaluateWriterLifecycle, WRITER_LIFECYCLE_ACTIONS, WRITER_OWNERSHIP_MARKER } from '../src/writer-lifecycle.mjs';

const sha = (character) => character.repeat(40);
const repositoryId = 123;
const writerApp = Object.freeze({ id: 456, slug: 'aeris-writer' });
const issueNumber = 42;
const branch = { ref: 'agent/issue-42', exists: true, headSha: sha('a') };

function pull(overrides = {}) {
  return {
    number: 99,
    state: 'open',
    merged: false,
    draft: true,
    body: `Writer ownership ${WRITER_OWNERSHIP_MARKER}`,
    user: { id: 789, type: 'Bot', login: `${writerApp.slug}[bot]` },
    performed_via_github_app: { id: writerApp.id, slug: writerApp.slug },
    base: { ref: 'main', repo: { id: repositoryId } },
    head: { ref: branch.ref, repo: { id: repositoryId }, sha: branch.headSha },
    ...overrides,
  };
}

function snapshot(overrides = {}) {
  return {
    command: '/agent implement', issueNumber, repositoryId, writerApp,
    branch: { ...branch }, pullRequests: [],
    ...overrides,
  };
}

test('implement creates only on a virgin Issue branch with zero exact-head history', () => {
  const virginBranch = { ref: branch.ref, exists: false, headSha: null };
  const result = evaluateWriterLifecycle(snapshot({ branch: virginBranch }));
  assert.deepEqual(result, { action: 'create', reason: 'create_draft_pull_request', branch: branch.ref });

  assert.equal(evaluateWriterLifecycle(snapshot()).reason, 'branch_already_exists');
  assert.equal(evaluateWriterLifecycle(snapshot({ branch: virginBranch, pullRequests: [pull()] })).reason, 'orphaned_managed_branch');
});

test('retry-write updates exactly one owned Draft PR at its expected head', () => {
  const result = evaluateWriterLifecycle(snapshot({ command: '/agent retry-write', expectedHeadSha: sha('a'), pullRequests: [pull()] }));
  assert.deepEqual(result, { action: 'update', reason: 'update_managed_draft_pull_request', branch: branch.ref, prNumber: 99 });
  assert.equal(evaluateWriterLifecycle(snapshot({ command: '/agent retry-write', expectedHeadSha: sha('b'), pullRequests: [pull()] })).reason, 'expected_head_sha_mismatch');
  assert.equal(evaluateWriterLifecycle(snapshot({ command: '/agent retry-write', expectedHeadSha: null, pullRequests: [pull()] })).reason, 'invalid_expected_head_sha');
  assert.equal(evaluateWriterLifecycle(snapshot({ command: '/agent retry-write', expectedHeadSha: sha('a') })).reason, 'retry_requires_managed_draft_pull_request');
});

test('closed managed history tombstones an Issue after its branch is deleted', () => {
  const deletedBranch = { ref: branch.ref, exists: false, headSha: null };
  const closed = pull({
    state: 'closed',
    merged: false,
    head: { ref: branch.ref, repo: null, sha: sha('a') },
  });
  assert.deepEqual(evaluateWriterLifecycle(snapshot({ branch: deletedBranch, pullRequests: [closed] })), {
    action: 'noop',
    reason: 'issue_tombstoned',
  });
});

test('merged managed history tombstones an Issue after its branch is deleted', () => {
  const deletedBranch = { ref: branch.ref, exists: false, headSha: null };
  const merged = pull({
    state: 'closed',
    merged: true,
    draft: false,
    head: { ref: branch.ref, repo: null, sha: sha('a') },
  });
  assert.equal(evaluateWriterLifecycle(snapshot({ branch: deletedBranch, pullRequests: [merged] })).reason, 'issue_tombstoned');
});

test('raw GitHub REST Writer identity requires Bot user and validates optional App evidence', () => {
  const retry = (candidate) => evaluateWriterLifecycle(snapshot({
    command: '/agent retry-write',
    expectedHeadSha: sha('a'),
    pullRequests: [candidate],
  }));

  assert.equal(retry(pull({ performed_via_github_app: null })).action, 'update');
  assert.equal(retry(pull({ user: null })).reason, 'managed_pr_writer_identity_invalid');
  assert.equal(retry(pull({
    author: { type: 'App', id: writerApp.id },
    user: { type: 'User', login: 'attacker' },
    performed_via_github_app: null,
  })).reason, 'managed_pr_writer_identity_invalid');
  assert.equal(retry(pull({
    user: { type: 'Bot', login: `${writerApp.slug}[bot]` },
    performed_via_github_app: { id: 999, slug: 'other-app' },
  })).reason, 'managed_pr_writer_identity_conflict');
});

test('refuses malformed PR numbers and merged state', () => {
  for (const [candidate, reason] of [
    [pull({ number: 0 }), 'managed_pr_number_invalid'],
    [pull({ number: 1.5 }), 'managed_pr_number_invalid'],
    [pull({ merged: undefined }), 'managed_pr_merged_invalid'],
    [pull({ merged: 'false' }), 'managed_pr_merged_invalid'],
    [pull({ state: 'open', merged: true }), 'managed_pr_open_merged_invalid'],
    [pull({ state: 'unknown', merged: false }), 'managed_pr_state_invalid'],
  ]) {
    assert.equal(evaluateWriterLifecycle(snapshot({ pullRequests: [candidate] })).reason, reason);
  }
});

test('refuses manual transitions, ambiguous history, and head drift', () => {
  assert.equal(evaluateWriterLifecycle(snapshot({ pullRequests: [pull({ draft: false })] })).reason, 'managed_pr_not_draft');
  assert.equal(evaluateWriterLifecycle(snapshot({ pullRequests: [pull(), pull({ number: 100 })] })).reason, 'multiple_managed_pull_requests');
  assert.equal(evaluateWriterLifecycle(snapshot({ pullRequests: [pull(), pull()] })).reason, 'duplicate_managed_pull_request');
  assert.equal(evaluateWriterLifecycle(snapshot({
    command: '/agent retry-write',
    expectedHeadSha: sha('a'),
    pullRequests: [pull({ head: { ref: branch.ref, repo: { id: repositoryId }, sha: sha('b') } })],
  })).reason, 'managed_pr_branch_head_drift');
});

test('refuses every repository, branch, marker, and identity violation', () => {
  const cases = [
    [pull({ base: { ref: 'release', repo: { id: repositoryId } } }), 'managed_pr_base_not_main'],
    [pull({ base: { ref: 'main', repo: { id: 321 } } }), 'managed_pr_base_repository_invalid'],
    [pull({ head: { ref: branch.ref, repo: { id: 321 }, sha: sha('a') } }), 'managed_pr_head_repository_invalid'],
    [pull({ head: { ref: 'agent/issue-7', repo: { id: repositoryId }, sha: sha('a') } }), 'managed_pr_head_branch_invalid'],
    [pull({ user: { type: 'User', login: 'aeris-writer[bot]' }, performed_via_github_app: null }), 'managed_pr_writer_identity_invalid'],
    [pull({ body: 'manually edited' }), 'managed_pr_marker_missing'],
  ];
  for (const [candidate, reason] of cases) {
    assert.equal(evaluateWriterLifecycle(snapshot({ pullRequests: [candidate] })).reason, reason);
  }
});

test('malformed inputs fail closed and actions never include destructive operations', () => {
  for (const overrides of [
    { command: '/agent merge' }, { issueNumber: 0 }, { repositoryId: 0 },
    { writerApp: { id: 0, slug: 'aeris-writer' } }, { writerApp: { id: 456 } },
    { branch: null }, { branch: { ...branch, ref: 'agent/issue-43' } },
    { pullRequests: {} }, { branch: { ...branch, headSha: 'bad' } },
  ]) assert.equal(evaluateWriterLifecycle(snapshot(overrides)).action, 'noop');
  assert.deepEqual(WRITER_LIFECYCLE_ACTIONS, ['create', 'update', 'noop']);
});
