import { revalidateWriterPublishBoundary } from './router.mjs';
import {
  createWriterReceiptMarker,
  evaluateWriterLifecycle,
  verifyWriterReceiptMarker,
  WRITER_LIFECYCLE_EPOCH,
  WRITER_OWNERSHIP_MARKER,
  WRITER_RECEIPT_MARKER_PREFIX,
} from './writer-lifecycle.mjs';
import {
  validateWriterCandidateArtifact,
  validateWriterReceiptArtifact,
  writerFenceHasMargin,
} from './writer-phase-contract.mjs';
import { WriterGitHubApiError } from './writer-github-client.mjs';

const SHA = /^[0-9a-f]{40}$/;
const DEFAULT_EXPIRY_MARGIN_MS = 180_000;

function terminalReceipt(candidate, state, reason) {
  return validateWriterReceiptArtifact({
    schema_version: 2,
    artifact_type: 'receipt',
    state,
    reason,
    candidate,
    candidate_sha: candidate.candidate_sha,
    commit_sha: null,
    ref: null,
    pr_number: null,
    pr_url: null,
    draft: null,
  });
}

function publishedReceipt(candidate, state, commitSha, pull) {
  return validateWriterReceiptArtifact({
    schema_version: 2,
    artifact_type: 'receipt',
    state,
    reason: state,
    candidate,
    candidate_sha: candidate.candidate_sha,
    commit_sha: commitSha,
    ref: candidate.intent.branch,
    pr_number: pull.number,
    pr_url: pull.html_url,
    draft: true,
  });
}

function residueReceipt(candidate, reason) {
  return validateWriterReceiptArtifact({
    schema_version: 2,
    artifact_type: 'receipt',
    state: 'residue',
    reason,
    candidate,
    candidate_sha: candidate.candidate_sha,
    commit_sha: null,
    ref: null,
    pr_number: null,
    pr_url: null,
    draft: null,
  });
}

function changeSet(candidate) {
  return candidate.file_sizes.map(({ path, bytes }) => ({ path, mode: '100644', bytes }));
}

function pullMetadata(candidate, commitSha, supplied = {}, receiptSigner) {
  const title = supplied.title ?? `Writer: implement Issue #${candidate.intent.issue_number}`;
  const description = supplied.body ?? `Automated Draft PR for Issue #${candidate.intent.issue_number}.`;
  if (description.includes(WRITER_OWNERSHIP_MARKER) || description.includes(WRITER_RECEIPT_MARKER_PREFIX)) {
    throw new Error('Writer metadata contains a reserved marker');
  }
  if (typeof receiptSigner !== 'function') throw new Error('Writer receipt signer is unavailable');
  const receiptFields = {
    issueNumber: candidate.intent.issue_number,
    commentId: candidate.intent.comment_id,
    lifecycleEpoch: WRITER_LIFECYCLE_EPOCH,
    fixCycle: candidate.fix_cycle,
    candidateSha: candidate.candidate_sha,
    commitSha,
  };
  const receiptMarker = createWriterReceiptMarker({
    ...receiptFields,
    signature: receiptSigner(receiptFields),
  });
  return {
    title,
    body: `${description}\n\nAeris Writer candidate: ${candidate.candidate_sha}\n\n${receiptMarker}`,
  };
}

function idempotentPull(snapshot, candidate, commitSha, metadata, repositoryId, writerApp, receiptPublicKey) {
  if (!snapshot.branch.exists || snapshot.branch.headSha !== commitSha || snapshot.pulls.length !== 1) return null;
  const verified = evaluateWriterLifecycle({
    command: '/agent retry-write',
    issueNumber: candidate.intent.issue_number,
    repositoryId,
    writerApp,
    expectedHeadSha: commitSha,
    baseSha: snapshot.mainSha,
    sourceSha: commitSha,
    branch: snapshot.branch,
    pullRequests: snapshot.pulls,
  });
  const pull = snapshot.pulls[0];
  const receipt = verifyWriterReceiptMarker(pull.body, receiptPublicKey);
  return verified.action === 'update' && pull.title === metadata.title &&
    typeof pull.body === 'string' && pull.body.includes(`Aeris Writer candidate: ${candidate.candidate_sha}`) &&
    receipt?.issue_number === candidate.intent.issue_number &&
    receipt.lifecycle_epoch === WRITER_LIFECYCLE_EPOCH &&
    receipt.comment_id === candidate.intent.comment_id && receipt.fix_cycle === candidate.fix_cycle &&
    receipt.candidate_sha === candidate.candidate_sha && receipt.commit_sha === commitSha
    ? pull : null;
}

function exactManagedMetadata(pull, metadata) {
  const expectedBody = metadata.body.length === 0
    ? WRITER_OWNERSHIP_MARKER
    : `${metadata.body}\n\n${WRITER_OWNERSHIP_MARKER}`;
  return pull?.title === metadata.title && pull?.body === expectedBody;
}

function validRef(raw, expectedRef) {
  return raw?.ref === `refs/heads/${expectedRef}` && raw?.object?.type === 'commit' && SHA.test(raw.object.sha);
}

async function readSnapshot(github, candidate, repositoryId, writerApp) {
  const main = await github.getMainRef();
  if (!validRef(main, 'main')) throw new Error('writer_main_ref_invalid');
  let branchRaw = null;
  try {
    branchRaw = await github.getRef(candidate.intent.branch);
  } catch (error) {
    if (!(error instanceof WriterGitHubApiError) || error.status !== 404) throw error;
  }
  if (branchRaw !== null && !validRef(branchRaw, candidate.intent.branch)) {
    throw new Error('writer_issue_ref_invalid');
  }
  const pulls = await github.listPullsForHead(candidate.intent.branch);
  const branch = {
    ref: candidate.intent.branch,
    exists: branchRaw !== null,
    headSha: branchRaw?.object?.sha ?? null,
  };
  const lifecycle = evaluateWriterLifecycle({
    command: candidate.intent.command,
    issueNumber: candidate.intent.issue_number,
    repositoryId,
    writerApp,
    expectedHeadSha: candidate.intent.expected_remote_head,
    baseSha: main.object.sha,
    sourceSha: candidate.intent.source_sha,
    branch,
    pullRequests: pulls,
  });
  return { mainSha: main.object.sha, branch, pulls, lifecycle };
}

function ownedPullAt(snapshot, candidate, repositoryId, writerApp, expectedHeadSha, expectedPullNumber = null) {
  if (!snapshot.branch.exists || snapshot.branch.headSha !== expectedHeadSha || snapshot.pulls.length !== 1) return null;
  const lifecycle = evaluateWriterLifecycle({
    command: '/agent retry-write',
    issueNumber: candidate.intent.issue_number,
    repositoryId,
    writerApp,
    expectedHeadSha,
    baseSha: snapshot.mainSha,
    sourceSha: expectedHeadSha,
    branch: snapshot.branch,
    pullRequests: snapshot.pulls,
  });
  if (lifecycle.action !== 'update') return null;
  const pull = snapshot.pulls[0];
  const sameRepository = (repo) => repo?.id === repositoryId &&
    typeof repo.full_name === 'string' &&
    repo.full_name.toLowerCase() === candidate.intent.repository_name.toLowerCase();
  if (!sameRepository(pull.base?.repo) || !sameRepository(pull.head?.repo)) return null;
  if (expectedPullNumber !== null && pull.number !== expectedPullNumber) return null;
  return pull;
}

function samePullState(left, right) {
  if (!left || !right) return false;
  const fields = (pull) => ({
    number: pull.number,
    state: pull.state,
    merged: pull.merged,
    draft: pull.draft,
    title: pull.title,
    body: pull.body,
    html_url: pull.html_url,
    base: { ref: pull.base?.ref, repo_id: pull.base?.repo?.id, repo_name: pull.base?.repo?.full_name },
    head: { ref: pull.head?.ref, sha: pull.head?.sha, repo_id: pull.head?.repo?.id, repo_name: pull.head?.repo?.full_name },
    user: { type: pull.user?.type, login: pull.user?.login },
    app: { id: pull.performed_via_github_app?.id ?? null, slug: pull.performed_via_github_app?.slug ?? null },
    author: { type: pull.author?.type, id: pull.author?.id },
    writer_lifecycle: {
      epoch: pull.writer_lifecycle?.epoch,
      tombstoned: pull.writer_lifecycle?.tombstoned,
    },
  });
  return JSON.stringify(fields(left)) === JSON.stringify(fields(right));
}

function exactSnapshotReason(snapshot, expected, candidate, repositoryId, writerApp) {
  if (snapshot.mainSha !== candidate.intent.base_sha) return 'base_main_changed';
  if (expected.kind === 'create_ref') {
    return !snapshot.branch.exists && snapshot.pulls.length === 0 && snapshot.lifecycle.action === 'create'
      ? null : 'create_ref_expected_state_changed';
  }
  if (expected.kind === 'create_pr') {
    return snapshot.branch.exists && snapshot.branch.headSha === expected.refSha && snapshot.pulls.length === 0
      ? null : 'create_pr_expected_state_changed';
  }
  const pull = ownedPullAt(
    snapshot,
    candidate,
    repositoryId,
    writerApp,
    expected.refSha,
    expected.pullNumber,
  );
  if (!pull) return `${expected.kind}_expected_state_changed`;
  return expected.pull === undefined || samePullState(pull, expected.pull)
    ? null : `${expected.kind}_expected_state_changed`;
}

function publishedPullMatches(snapshot, returnedPull, candidate, commitSha, repositoryId, writerApp, receiptPublicKey) {
  if (snapshot.mainSha !== candidate.intent.base_sha) return false;
  const pull = ownedPullAt(snapshot, candidate, repositoryId, writerApp, commitSha, returnedPull?.number ?? null);
  if (!pull || pull.title !== returnedPull.title || pull.body !== returnedPull.body || pull.html_url !== returnedPull.html_url) return false;
  const marker = verifyWriterReceiptMarker(pull.body, receiptPublicKey);
  return marker?.issue_number === candidate.intent.issue_number &&
    marker.lifecycle_epoch === WRITER_LIFECYCLE_EPOCH &&
    marker.comment_id === candidate.intent.comment_id && marker.fix_cycle === candidate.fix_cycle &&
    marker.candidate_sha === candidate.candidate_sha && marker.commit_sha === commitSha;
}

function priorRetryReceiptMatches(snapshot, candidate, receiptPublicKey) {
  if (candidate.intent.command !== '/agent retry-write' || snapshot.pulls.length !== 1) return false;
  const marker = verifyWriterReceiptMarker(snapshot.pulls[0].body, receiptPublicKey);
  return marker?.issue_number === candidate.intent.issue_number &&
    marker.lifecycle_epoch === WRITER_LIFECYCLE_EPOCH &&
    marker.comment_id !== candidate.intent.comment_id &&
    marker.fix_cycle === candidate.fix_cycle - 1 &&
    marker.commit_sha === candidate.intent.source_sha;
}

function isResidueError(error) {
  const messages = error instanceof AggregateError
    ? [error.message, ...error.errors.map((item) => item?.message)]
    : [error?.message];
  return messages.some((message) => typeof message === 'string' &&
    /ambiguous|residue|platform state may|platform residue may|expected_state_changed|mutation boundary rejected/i.test(message));
}

/**
 * Deterministic Writer publish coordinator. Candidate code is never executed
 * here. The caller must verify that commitSha materializes candidate.patch_sha
 * before invoking this function.
 */
export async function publishWriterCandidate({
  candidate: inputCandidate,
  commitSha,
  metadata = {},
  github,
  trustedContracts,
  environment,
  currentPolicySha,
  currentConfigSha,
  repositoryId,
  writerApp,
  receiptSigner,
  receiptPublicKey,
  fixCycle,
  verifyCandidateCommit,
  clock = () => new Date(),
  minimumExpiryMarginMs = DEFAULT_EXPIRY_MARGIN_MS,
  dryRun = false,
  repositoryPath = null,
} = {}) {
  const candidate = validateWriterCandidateArtifact(inputCandidate);
  if (candidate.state !== 'ready') return terminalReceipt(candidate, 'rejected', 'candidate_rejected');
  if (candidate.tests.state !== 'passed') return terminalReceipt(candidate, 'failed', 'trusted_tests_failed');
  if (!SHA.test(commitSha ?? '')) return terminalReceipt(candidate, 'failed', 'commit_sha_invalid');
  if (candidate.intent.policy_sha !== currentPolicySha) return terminalReceipt(candidate, 'stale', 'policy_sha_changed');
  if (candidate.intent.config_sha !== currentConfigSha) return terminalReceipt(candidate, 'stale', 'config_sha_changed');
  if (!writerFenceHasMargin(candidate.intent, clock(), minimumExpiryMarginMs)) {
    return terminalReceipt(candidate, 'stale', 'autonomy_expiry_margin');
  }
  if (typeof verifyCandidateCommit !== 'function' || !(await verifyCandidateCommit({ candidate, commitSha }))) {
    return terminalReceipt(candidate, 'failed', 'candidate_commit_mismatch');
  }

  const changes = changeSet(candidate);
  const revalidate = async () => {
    if (!writerFenceHasMargin(candidate.intent, clock(), minimumExpiryMarginMs)) {
      return { action: 'skip', reason: 'autonomy_expiry_margin' };
    }
    return revalidateWriterPublishBoundary({
      intent: candidate.intent,
      github,
      trustedContracts,
      environment,
      fixCycle,
      limits: candidate.limits,
      changeSet: changes,
      patchBytes: candidate.patch_bytes,
    });
  };
  const initialBoundary = await revalidate();
  if (initialBoundary.action !== 'write') return terminalReceipt(candidate, 'stale', initialBoundary.reason);

  try {
    let snapshot = await readSnapshot(github, candidate, repositoryId, writerApp);
    if (snapshot.mainSha !== candidate.intent.base_sha) return terminalReceipt(candidate, 'stale', 'base_main_changed');
    const prMetadata = pullMetadata(candidate, commitSha, metadata, receiptSigner);
    if (candidate.intent.command === '/agent retry-write' &&
      !priorRetryReceiptMatches(snapshot, candidate, receiptPublicKey)) {
      return terminalReceipt(candidate, 'stale', 'managed_pr_receipt_changed');
    }
    if (snapshot.lifecycle.action === 'noop') {
      const replay = idempotentPull(
        snapshot,
        candidate,
        commitSha,
        prMetadata,
        repositoryId,
        writerApp,
        receiptPublicKey,
      );
      if (replay) return publishedReceipt(
        candidate,
        candidate.intent.command === '/agent implement' ? 'draft_created' : 'draft_updated',
        commitSha,
        replay,
      );
      return terminalReceipt(candidate, 'stale', snapshot.lifecycle.reason);
    }
    if (dryRun) return terminalReceipt(candidate, 'cancelled', 'disabled_canary');

    const mutationBoundary = (expected) => async () => {
      const authorization = await revalidate();
      if (authorization.action !== 'write') return authorization;
      const live = await readSnapshot(github, candidate, repositoryId, writerApp);
      const reason = exactSnapshotReason(live, expected, candidate, repositoryId, writerApp);
      return reason === null ? { action: 'write' } : { action: 'skip', reason };
    };

    if (snapshot.lifecycle.action === 'create') {
      if (repositoryPath === null || typeof github.pushAgentRefFromRepository !== 'function') {
        return terminalReceipt(candidate, 'failed', 'atomic_ref_cas_unavailable');
      }
      const createRefBoundary = mutationBoundary({ kind: 'create_ref' });
      const refBoundary = await createRefBoundary();
      if (refBoundary.action !== 'write') return terminalReceipt(candidate, 'stale', refBoundary.reason);
      await github.pushAgentRefFromRepository(candidate.intent.issue_number, null, commitSha, repositoryPath, createRefBoundary);

      snapshot = await readSnapshot(github, candidate, repositoryId, writerApp);
      if (snapshot.mainSha !== candidate.intent.base_sha || !snapshot.branch.exists || snapshot.branch.headSha !== commitSha || snapshot.pulls.length !== 0) {
        return residueReceipt(candidate, 'post_ref_create_drift');
      }
      const createPrBoundary = mutationBoundary({ kind: 'create_pr', refSha: commitSha });
      const prBoundary = await createPrBoundary();
      if (prBoundary.action !== 'write') return residueReceipt(candidate, prBoundary.reason);
      const pull = await github.createDraftPull(candidate.intent.issue_number, prMetadata, createPrBoundary);
      snapshot = await readSnapshot(github, candidate, repositoryId, writerApp);
      if (!publishedPullMatches(snapshot, pull, candidate, commitSha, repositoryId, writerApp, receiptPublicKey)) {
        return residueReceipt(candidate, 'post_pr_create_drift');
      }
      return publishedReceipt(candidate, 'draft_created', commitSha, pull);
    }

    const pullNumber = snapshot.lifecycle.prNumber;
    if (!exactManagedMetadata(snapshot.pulls[0], prMetadata)) {
      return terminalReceipt(candidate, 'stale', 'managed_pr_metadata_change_requires_cas');
    }
    if (repositoryPath === null || typeof github.pushAgentRefFromRepository !== 'function') {
      return terminalReceipt(candidate, 'failed', 'atomic_ref_cas_unavailable');
    }
    const advanceRefBoundary = mutationBoundary({
      kind: 'advance_ref',
      refSha: candidate.intent.expected_remote_head,
      pullNumber,
      pull: snapshot.pulls[0],
    });
    const refBoundary = await advanceRefBoundary();
    if (refBoundary.action !== 'write') return terminalReceipt(candidate, 'stale', refBoundary.reason);
    await github.pushAgentRefFromRepository(
      candidate.intent.issue_number,
      candidate.intent.expected_remote_head,
      commitSha,
      repositoryPath,
      advanceRefBoundary,
    );

    snapshot = await readSnapshot(github, candidate, repositoryId, writerApp);
    if (snapshot.mainSha !== candidate.intent.base_sha || snapshot.branch.headSha !== commitSha || snapshot.pulls.length !== 1) {
      return residueReceipt(candidate, 'post_ref_advance_drift');
    }
    const pull = snapshot.pulls[0];
    if (pull.number !== snapshot.lifecycle.prNumber && snapshot.lifecycle.action !== 'noop') {
      return residueReceipt(candidate, 'managed_pr_lifecycle_drift');
    }
    const updatePrBoundary = mutationBoundary({
      kind: 'update_pr',
      refSha: commitSha,
      pullNumber: pull.number,
      pull,
    });
    const prBoundary = await updatePrBoundary();
    if (prBoundary.action !== 'write') return residueReceipt(candidate, prBoundary.reason);
    const updated = await github.verifyManagedDraftPullMetadata(
      candidate.intent.issue_number,
      pull.number,
      commitSha,
      prMetadata,
      updatePrBoundary,
    );
    snapshot = await readSnapshot(github, candidate, repositoryId, writerApp);
    if (!publishedPullMatches(snapshot, updated, candidate, commitSha, repositoryId, writerApp, receiptPublicKey)) {
      return residueReceipt(candidate, 'post_pr_update_drift');
    }
    return publishedReceipt(candidate, 'draft_updated', commitSha, updated);
  } catch (error) {
    return isResidueError(error)
      ? residueReceipt(candidate, 'ambiguous_platform_residue')
      : terminalReceipt(candidate, 'failed', 'publish_failed');
  }
}
