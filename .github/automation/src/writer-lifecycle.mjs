export const WRITER_LIFECYCLE_ACTIONS = Object.freeze(['create', 'update', 'noop']);
export const WRITER_OWNERSHIP_MARKER = '<!-- aeris-writer-managed -->';

const COMMANDS = new Set(['/agent implement', '/agent retry-write']);
const SHA = /^[0-9a-f]{40}$/;
const APP_SLUG = /^[a-z\d](?:[a-z\d-]{0,98}[a-z\d])?$/;

function noop(reason) {
  return { action: 'noop', reason };
}

function validPositiveNumber(value) {
  return Number.isSafeInteger(value) && value > 0;
}

function validSha(value) {
  return typeof value === 'string' && SHA.test(value);
}

function branchFor(issueNumber) {
  return `agent/issue-${issueNumber}`;
}

function sameRepository(value, repositoryId) {
  return value?.id === repositoryId;
}

function validWriterApp(writerApp) {
  return writerApp &&
    typeof writerApp === 'object' &&
    !Array.isArray(writerApp) &&
    validPositiveNumber(writerApp.id) &&
    typeof writerApp.slug === 'string' &&
    APP_SLUG.test(writerApp.slug);
}

function writerIdentityReason(pull, writerApp) {
  const user = pull.user;
  const app = pull.performed_via_github_app;
  const hasApp = app !== null && app !== undefined;
  const userMatches =
    typeof user === 'object' &&
    user !== null &&
    !Array.isArray(user) &&
    user.type === 'Bot' &&
    user.login === `${writerApp.slug}[bot]`;
  const appMatches = hasApp &&
    typeof app === 'object' &&
    !Array.isArray(app) &&
    app.id === writerApp.id &&
    app.slug === writerApp.slug;

  if (!userMatches) return 'managed_pr_writer_identity_invalid';
  if (hasApp && !appMatches) return 'managed_pr_writer_identity_conflict';
  return null;
}

export function hasCanonicalWriterOwnershipMarker(body) {
  if (typeof body !== 'string') return false;
  const first = body.indexOf(WRITER_OWNERSHIP_MARKER);
  if (first < 0 || first !== body.lastIndexOf(WRITER_OWNERSHIP_MARKER)) return false;
  return body === WRITER_OWNERSHIP_MARKER || body.endsWith(`\n\n${WRITER_OWNERSHIP_MARKER}`);
}

function validateHistoryPull(pull, { repositoryId, branch, writerApp }) {
  if (!pull || typeof pull !== 'object' || Array.isArray(pull)) return 'managed_pr_invalid';
  if (!validPositiveNumber(pull.number)) return 'managed_pr_number_invalid';
  if (pull.state !== 'open' && pull.state !== 'closed') return 'managed_pr_state_invalid';
  if (typeof pull.merged !== 'boolean') return 'managed_pr_merged_invalid';
  if (pull.state === 'open' && pull.merged !== false) return 'managed_pr_open_merged_invalid';
  if (pull.base?.ref !== 'main') return 'managed_pr_base_not_main';
  if (!sameRepository(pull.base?.repo, repositoryId)) return 'managed_pr_base_repository_invalid';
  if (pull.head?.ref !== branch) return 'managed_pr_head_branch_invalid';

  // GitHub returns head.repo=null after a source branch is deleted. Only closed
  // history may use that tombstone representation.
  if (pull.head?.repo === null) {
    if (pull.state !== 'closed') return 'managed_pr_head_repository_invalid';
  } else if (!sameRepository(pull.head?.repo, repositoryId)) {
    return 'managed_pr_head_repository_invalid';
  }

  if (!validSha(pull.head?.sha)) return 'managed_pr_head_sha_invalid';
  const identityReason = writerIdentityReason(pull, writerApp);
  if (identityReason) return identityReason;
  if (!hasCanonicalWriterOwnershipMarker(pull.body)) return 'managed_pr_marker_missing';
  if (pull.state === 'open' && pull.draft !== true) return 'managed_pr_not_draft';
  return null;
}

/**
 * Resolve one Writer action from an immutable remote snapshot.
 *
 * `pullRequests` must be the complete GitHub REST `state=all` result for the
 * exact `owner:agent/issue-N` head and `main` base. Entries retain raw `user`
 * and `performed_via_github_app` fields. `writerApp` is `{ id, slug }`. The
 * matching Bot `user` is required; `performed_via_github_app` may be null, but
 * when present it must match the same App.
 * Callers perform create/update separately and must re-read before writing.
 */
export function evaluateWriterLifecycle({
  command,
  issueNumber,
  repositoryId,
  writerApp,
  expectedHeadSha = null,
  branch = null,
  pullRequests = [],
} = {}) {
  if (!COMMANDS.has(command)) return noop('unsupported_writer_command');
  if (!validPositiveNumber(issueNumber)) return noop('invalid_issue_number');
  if (!validPositiveNumber(repositoryId)) return noop('invalid_repository');
  if (!validWriterApp(writerApp)) return noop('invalid_writer_app');
  if (!Array.isArray(pullRequests)) return noop('invalid_pull_requests');

  const expectedBranch = branchFor(issueNumber);
  if (!branch || typeof branch !== 'object' || Array.isArray(branch) || branch.ref !== expectedBranch || typeof branch.exists !== 'boolean') {
    return noop('invalid_branch_snapshot');
  }
  if (branch.exists && !validSha(branch.headSha)) return noop('branch_head_sha_invalid');
  if (!branch.exists && branch.headSha !== null) return noop('branch_head_presence_invalid');

  const seenPullNumbers = new Set();
  for (const pull of pullRequests) {
    const invalidReason = validateHistoryPull(pull, {
      repositoryId,
      branch: expectedBranch,
      writerApp,
    });
    if (invalidReason) return noop(invalidReason);
    if (seenPullNumbers.has(pull.number)) return noop('duplicate_managed_pull_request');
    seenPullNumbers.add(pull.number);
  }

  // Closed or merged managed history is the permanent tombstone. This check is
  // independent of branch existence so branch deletion cannot reset lifecycle.
  if (pullRequests.some((pull) => pull.state === 'closed' || pull.merged === true)) {
    return noop('issue_tombstoned');
  }

  if (pullRequests.length > 1) return noop('multiple_managed_pull_requests');
  const pull = pullRequests[0] ?? null;

  if (pull) {
    if (!branch.exists) return noop('orphaned_managed_branch');
    if (branch.headSha !== pull.head.sha) return noop('managed_pr_branch_head_drift');
  }

  if (command === '/agent implement') {
    if (branch.exists) return noop('branch_already_exists');
    if (pull) return noop('managed_pull_request_already_exists');
    return { action: 'create', reason: 'create_draft_pull_request', branch: expectedBranch };
  }

  if (!pull) return noop('retry_requires_managed_draft_pull_request');
  if (!validSha(expectedHeadSha)) return noop('invalid_expected_head_sha');
  if (expectedHeadSha !== pull.head.sha) return noop('expected_head_sha_mismatch');
  if (branch.headSha !== expectedHeadSha) return noop('branch_head_sha_mismatch');
  return { action: 'update', reason: 'update_managed_draft_pull_request', branch: expectedBranch, prNumber: pull.number };
}
