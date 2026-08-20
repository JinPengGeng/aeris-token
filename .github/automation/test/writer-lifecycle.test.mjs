import assert from 'node:assert/strict';
import test from 'node:test';

import {
  evaluateWriterLifecycle,
  evaluateWriterRetryLineage,
  hasCanonicalWriterOwnershipMarker,
  writerCommitMessage,
  WRITER_LIFECYCLE_ACTIONS,
  WRITER_LIFECYCLE_EPOCH,
  WRITER_OWNERSHIP_MARKER,
} from '../src/writer-lifecycle.mjs';

const sha = (character) => character.repeat(40);
const repositoryId = 123;
const writerApp = Object.freeze({ id: 456, slug: 'aeris-writer' });
const issueNumber = 42;
const branch = { ref: 'agent/issue-42', exists: true, headSha: sha('a') };

function retryCompare({ candidateSha = '1'.repeat(64), message = null, parentSha = sha('a'), headSha = sha('b') } = {}) {
  return {
    status: 'ahead',
    ahead_by: 1,
    total_commits: 1,
    base_commit: { sha: sha('a') },
    merge_base_commit: { sha: sha('a') },
    commits: [{
      sha: headSha,
      parents: [{ sha: parentSha }],
      commit: { message: message ?? writerCommitMessage({ issueNumber, fixCycle: 1, commentId: 92, candidateSha, pullMetadataSha: '2'.repeat(64) }) },
    }],
  };
}

test('retry lineage binds exact parent, cycle, comment, candidate, PR metadata, and head', () => {
  const receipt = {
    issue_number: issueNumber,
    comment_id: 91,
    lifecycle_epoch: WRITER_LIFECYCLE_EPOCH,
    fix_cycle: 0,
    candidate_sha: '0'.repeat(64),
    commit_sha: sha('a'),
  };
  const accepted = evaluateWriterRetryLineage({
    receipt,
    sourceSha: sha('b'),
    compare: retryCompare(),
    issueNumber,
    commentId: 93,
    maximumFixCycles: 2,
  });
  assert.equal(accepted.allowed, true);
  assert.equal(accepted.fixCycle, 2);
  assert.equal(accepted.commits[0].candidate_sha, '1'.repeat(64));
  assert.equal(accepted.commits[0].pull_metadata_sha, '2'.repeat(64));
  assert.throws(() => writerCommitMessage({
    issueNumber,
    fixCycle: 1,
    commentId: 92,
    candidateSha: '1'.repeat(64),
  }), /fields are invalid/);

  for (const compare of [
    retryCompare({ parentSha: sha('9') }),
    retryCompare({ message: writerCommitMessage({ issueNumber, fixCycle: 2, commentId: 92, candidateSha: '1'.repeat(64), pullMetadataSha: '2'.repeat(64) }) }),
    { ...retryCompare(), status: 'diverged' },
  ]) {
    assert.equal(evaluateWriterRetryLineage({
      receipt, sourceSha: sha('b'), compare, issueNumber, commentId: 93, maximumFixCycles: 2,
    }).reason, 'managed_pr_retry_lineage_invalid');
  }
  assert.equal(evaluateWriterRetryLineage({
    receipt, sourceSha: sha('b'), compare: retryCompare(), issueNumber, commentId: 92, maximumFixCycles: 2,
  }).reason, 'retry_comment_already_applied');
  assert.equal(evaluateWriterRetryLineage({
    receipt,
    sourceSha: sha('b'),
    compare: retryCompare({
      message: writerCommitMessage({
        issueNumber,
        fixCycle: 1,
        commentId: receipt.comment_id,
        candidateSha: '1'.repeat(64),
        pullMetadataSha: '2'.repeat(64),
      }),
    }),
    issueNumber,
    commentId: 93,
    maximumFixCycles: 2,
  }).reason, 'retry_comment_already_applied');
  assert.equal(evaluateWriterRetryLineage({
    receipt: { ...receipt, fix_cycle: 1 },
    sourceSha: sha('b'), compare: retryCompare(), issueNumber, commentId: 93, maximumFixCycles: 2,
  }).reason, 'managed_pr_retry_lineage_invalid');
});

function pull(overrides = {}) {
  return {
    number: 99,
    state: 'open',
    merged: false,
    draft: true,
    body: `Writer ownership\n\n${WRITER_OWNERSHIP_MARKER}`,
    user: { id: 789, type: 'Bot', login: `${writerApp.slug}[bot]` },
    performed_via_github_app: { id: writerApp.id, slug: writerApp.slug },
    writer_lifecycle: { epoch: WRITER_LIFECYCLE_EPOCH, tombstoned: false },
    base: { ref: 'main', repo: { id: repositoryId } },
    head: { ref: branch.ref, repo: { id: repositoryId }, sha: branch.headSha },
    ...overrides,
  };
}

test('Writer ownership marker must be unique and in the canonical final position', () => {
  assert.equal(hasCanonicalWriterOwnershipMarker(WRITER_OWNERSHIP_MARKER), true);
  assert.equal(hasCanonicalWriterOwnershipMarker(`Details\n\n${WRITER_OWNERSHIP_MARKER}`), true);
  for (const body of [
    `Quoted ${WRITER_OWNERSHIP_MARKER}`,
    `> ${WRITER_OWNERSHIP_MARKER}\n\nDetails`,
    `${WRITER_OWNERSHIP_MARKER}\ntrailing content`,
    `Details\n${WRITER_OWNERSHIP_MARKER}`,
    `Details\n\n${WRITER_OWNERSHIP_MARKER}\n`,
    `${WRITER_OWNERSHIP_MARKER}\n\n${WRITER_OWNERSHIP_MARKER}`,
  ]) {
    assert.equal(hasCanonicalWriterOwnershipMarker(body), false, body);
    const result = evaluateWriterLifecycle({
      command: '/agent retry-write',
      issueNumber,
      writerApp,
      repositoryId,
      branch,
      pullRequests: [pull({ body })],
    });
    assert.deepEqual(result, { action: 'noop', reason: 'managed_pr_marker_missing' });
  }
});

function snapshot(overrides = {}) {
  return {
    command: '/agent implement', issueNumber, repositoryId, writerApp,
    baseSha: sha('a'), sourceSha: sha('a'),
    branch: { ...branch }, pullRequests: [],
    ...overrides,
  };
}

test('implement creates only on a virgin Issue branch with zero exact-head history', () => {
  const virginBranch = { ref: branch.ref, exists: false, headSha: null };
  const result = evaluateWriterLifecycle(snapshot({ branch: virginBranch }));
  assert.deepEqual(result, { action: 'create', reason: 'create_draft_pull_request', branch: branch.ref, sourceSha: sha('a') });

  assert.equal(evaluateWriterLifecycle(snapshot()).reason, 'branch_already_exists');
  assert.equal(evaluateWriterLifecycle(snapshot({ branch: virginBranch, pullRequests: [pull()] })).reason, 'orphaned_managed_branch');
});

test('retry-write updates exactly one owned Draft PR at its expected head', () => {
  const result = evaluateWriterLifecycle(snapshot({ command: '/agent retry-write', expectedHeadSha: sha('a'), pullRequests: [pull()] }));
  assert.deepEqual(result, { action: 'update', reason: 'update_managed_draft_pull_request', branch: branch.ref, prNumber: 99, sourceSha: sha('a') });
  assert.equal(evaluateWriterLifecycle(snapshot({ command: '/agent retry-write', expectedHeadSha: sha('b'), sourceSha: sha('b'), pullRequests: [pull()] })).reason, 'expected_head_sha_mismatch');
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

test('a reopened managed Draft PR remains irreversibly tombstoned by its lifecycle attestation', () => {
  const reopened = pull({
    state: 'open',
    merged: false,
    draft: true,
    writer_lifecycle: { epoch: WRITER_LIFECYCLE_EPOCH, tombstoned: true },
  });
  assert.equal(evaluateWriterLifecycle(snapshot({
    command: '/agent retry-write',
    expectedHeadSha: sha('a'),
    pullRequests: [reopened],
  })).reason, 'issue_tombstoned');
});

test('missing, malformed, or future lifecycle attestations fail closed', () => {
  for (const writerLifecycle of [undefined, null, {}, { epoch: 1, tombstoned: false }, { epoch: 0, tombstoned: 'false' }]) {
    assert.equal(evaluateWriterLifecycle(snapshot({ pullRequests: [pull({ writer_lifecycle: writerLifecycle })] })).reason,
      'managed_pr_lifecycle_attestation_invalid');
  }
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
