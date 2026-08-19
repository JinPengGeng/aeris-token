import assert from 'node:assert/strict';
import { createSign, generateKeyPairSync } from 'node:crypto';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import { loadContracts } from '../src/config.mjs';
import { calculateWriterCandidateSha } from '../src/writer-phase-contract.mjs';
import { WriterGitHubApiError } from '../src/writer-github-client.mjs';
import {
  createWriterReceiptMarker,
  writerReceiptSigningPayload,
  WRITER_LIFECYCLE_EPOCH,
} from '../src/writer-lifecycle.mjs';
import { publishWriterCandidate } from '../src/writer-publisher.mjs';

const sha = (character, length = 40) => character.repeat(length);
const repositoryId = 1_316_750_512;
const repository = 'JinPengGeng/aeris-token';
const issueUrl = `https://api.github.com/repos/${repository}/issues/12`;
const writerApp = { id: 456, slug: 'aeris-writer' };
const baseSha = sha('a');
const commitSha = sha('f');
const receiptKeys = generateKeyPairSync('rsa', { modulusLength: 2048 });
const receiptPrivateKey = receiptKeys.privateKey.export({ type: 'pkcs8', format: 'pem' });
const receiptPublicKey = receiptKeys.publicKey.export({ type: 'spki', format: 'pem' });
const receiptSigner = (fields) => {
  const signer = createSign('RSA-SHA256');
  signer.update(writerReceiptSigningPayload(fields));
  signer.end();
  return signer.sign(receiptPrivateKey).toString('base64url');
};
const signedReceipt = ({ cycle, commentId, candidateSha = sha(String(cycle + 1), 64), commit = baseSha }) => {
  const fields = {
    issueNumber: 12,
    commentId,
    lifecycleEpoch: WRITER_LIFECYCLE_EPOCH,
    fixCycle: cycle,
    candidateSha,
    commitSha: commit,
  };
  return createWriterReceiptMarker({ ...fields, signature: receiptSigner(fields) });
};

function enabledContracts() {
  const contracts = loadContracts(path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..'));
  const copy = structuredClone(contracts);
  copy.agents.agents.writer.enabled = true;
  copy.policy.writer.enabled = true;
  return copy;
}

function candidate(overrides = {}) {
  const value = {
    schema_version: 2,
    artifact_type: 'candidate',
    state: 'ready',
    intent: {
      repository_id: repositoryId,
      repository_name: repository,
      issue_number: 12,
      issue_url: issueUrl,
      issue_updated_at: '2026-08-19T10:00:00Z',
      issue_labels: ['agent-ready'],
      input_sha: sha('b', 64),
      comment_id: 91,
      actor: 'maintainer',
      command: '/agent implement',
      base_sha: baseSha,
      source_sha: baseSha,
      policy_sha: sha('c'),
      config_sha: sha('d'),
      run_id: 'run-123',
      agent: 'writer',
      branch: 'agent/issue-12',
      expected_remote_head: null,
      lease_token: sha('e', 64),
      cancel_epoch: 0,
      lease_expires_at: '2026-08-19T11:00:00Z',
    },
    patch_sha: sha('1', 64),
    changed_paths: ['docs/writer-canary.md'],
    file_sizes: [{ path: 'docs/writer-canary.md', bytes: 32 }],
    file_count: 1,
    patch_bytes: 32,
    total_file_bytes: 32,
    limits: {
      maximum_files: 50,
      maximum_patch_bytes: 65536,
      maximum_file_size_bytes: 524288,
      maximum_total_file_bytes: 2097152,
      maximum_fix_cycles: 2,
    },
    fix_cycle: 0,
    tests: { state: 'passed', plan_ids: ['diff-check-v1'], summary: 'trusted diff check passed' },
    ...overrides,
  };
  value.candidate_sha = calculateWriterCandidateSha(value);
  return value;
}

function rawPull(headSha, overrides = {}) {
  return {
    number: 77,
    state: 'open',
    merged: false,
    merged_at: null,
    draft: true,
    title: 'Writer: implement Issue #12',
    body: '<!-- aeris-writer-managed -->',
    html_url: `https://github.com/${repository}/pull/77`,
    user: { type: 'Bot', login: 'aeris-writer[bot]' },
    performed_via_github_app: writerApp,
    writer_lifecycle: { epoch: WRITER_LIFECYCLE_EPOCH, tombstoned: false },
    base: { ref: 'main', repo: { id: repositoryId, full_name: repository } },
    head: { ref: 'agent/issue-12', sha: headSha, repo: { id: repositoryId, full_name: repository } },
    ...overrides,
  };
}

function mockGitHub({
  existing = false,
  ambiguous = false,
  boundaryDriftAt = null,
  duplicate = false,
  manual = false,
  mainSha = baseSha,
  mainDriftAtBoundary = null,
  injectPullAtBoundary = null,
  postCreateMainDrift = false,
  existingPullBody = null,
  pullTitleDriftAtBoundary = null,
  tombstoned = false,
  tombstoneAtBoundary = null,
  command = '/agent implement',
  commentId = 91,
} = {}) {
  let branchSha = existing ? baseSha : null;
  let pull = existing ? rawPull(baseSha, {
    ...(existingPullBody === null ? {} : { body: existingPullBody }),
    writer_lifecycle: { epoch: WRITER_LIFECYCLE_EPOCH, tombstoned },
  }) : null;
  let liveMainSha = mainSha;
  let boundaryReads = 0;
  const mutations = [];
  const timeline = [];
  const reads = [];
  const read = (name) => { reads.push(name); timeline.push(name); };
  const requireBoundary = async (boundary) => {
    assert.equal(typeof boundary, 'function');
    const decision = await boundary();
    if (decision.action !== 'write') throw new Error(`boundary rejected: ${decision.reason}`);
  };
  return {
    mutations,
    reads,
    timeline,
    getRepository: async () => { read('repository'); return { id: repositoryId, full_name: repository }; },
    getIssueComment: async () => {
      boundaryReads += 1;
      read('comment');
      if (mainDriftAtBoundary === boundaryReads) liveMainSha = sha('8');
      if (injectPullAtBoundary === boundaryReads) pull = rawPull(branchSha ?? commitSha, { number: 79 });
      if (pullTitleDriftAtBoundary === boundaryReads && pull) pull = { ...pull, title: 'Concurrent edit' };
      if (tombstoneAtBoundary === boundaryReads && pull) pull = {
        ...pull,
        writer_lifecycle: { epoch: WRITER_LIFECYCLE_EPOCH, tombstoned: true },
      };
      return {
        id: commentId,
        body: boundaryReads === boundaryDriftAt
          ? command === '/agent implement' ? '/agent retry-write' : '/agent implement'
          : command,
        issue_url: issueUrl,
        user: { login: 'maintainer' },
      };
    },
    getIssue: async () => { read('issue'); return ({
      number: 12,
      state: 'open',
      updated_at: '2026-08-19T10:00:00Z',
      url: issueUrl,
      labels: [{ name: 'agent-ready' }],
    }); },
    getCollaboratorPermission: async () => { read('permission'); return 'write'; },
    getMainRef: async () => { read('main'); return { ref: 'refs/heads/main', object: { type: 'commit', sha: liveMainSha } }; },
    getRef: async () => {
      read('ref');
      if (branchSha === null) throw new WriterGitHubApiError('missing', 404);
      return { ref: 'refs/heads/agent/issue-12', object: { type: 'commit', sha: branchSha } };
    },
    listPullsForHead: async () => {
      read('pulls');
      if (!pull) return [];
      const visible = manual ? rawPull(branchSha, { user: { type: 'User', login: 'maintainer' } }) : rawPull(branchSha, pull);
      return duplicate ? [visible, rawPull(branchSha, { number: 78 })] : [visible];
    },
    pushAgentRefFromRepository: async (_issue, _oldSha, nextSha, _repositoryPath, boundary) => {
      await requireBoundary(boundary);
      const action = branchSha === null ? 'create_ref' : 'advance_ref';
      mutations.push(action);
      timeline.push(action);
      branchSha = nextSha;
      if (ambiguous) throw new AggregateError([new Error('timeout')], 'ambiguous ref create; platform state may have changed');
      if (pull) pull = {
        ...pull,
        head: { ...pull.head, sha: nextSha },
      };
    },
    createDraftPull: async (_issue, metadata, boundary) => {
      await requireBoundary(boundary);
      mutations.push('create_pr');
      timeline.push('create_pr');
      pull = rawPull(branchSha, { title: metadata.title, body: `${metadata.body}\n\n<!-- aeris-writer-managed -->` });
      if (postCreateMainDrift) liveMainSha = sha('8');
      return pull;
    },
    verifyManagedDraftPullMetadata: async (_issue, _number, _sha, metadata, boundary) => {
      await requireBoundary(boundary);
      timeline.push('verify_pr_metadata');
      return pull;
    },
  };
}

function options(github, value = candidate(), overrides = {}) {
  return {
    candidate: value,
    commitSha,
    github,
    trustedContracts: enabledContracts(),
    environment: { AERIS_AGENTS_ENABLED: 'true', AERIS_WRITER_ENABLED: 'true' },
    currentPolicySha: sha('c'),
    currentConfigSha: sha('d'),
    repositoryId,
    writerApp,
    receiptSigner,
    receiptPublicKey,
    fixCycle: value.fix_cycle,
    verifyCandidateCommit: async () => true,
    clock: () => new Date('2026-08-19T10:30:00Z'),
    repositoryPath: process.cwd(),
    ...overrides,
  };
}

test('publisher revalidates immediately before ref and Draft PR creation', async () => {
  const github = mockGitHub();
  const receipt = await publishWriterCandidate(options(github));
  assert.equal(receipt.state, 'draft_created');
  assert.deepEqual(github.mutations, ['create_ref', 'create_pr']);
});

test('publisher fails before mutation when atomic ref CAS is unavailable', async () => {
  const withoutMethod = mockGitHub();
  withoutMethod.pushAgentRefFromRepository = undefined;
  const methodReceipt = await publishWriterCandidate(options(withoutMethod));
  assert.equal(methodReceipt.state, 'failed');
  assert.equal(methodReceipt.reason, 'atomic_ref_cas_unavailable');
  assert.deepEqual(withoutMethod.mutations, []);

  const withoutPath = mockGitHub();
  const pathReceipt = await publishWriterCandidate(options(withoutPath, candidate(), { repositoryPath: null }));
  assert.equal(pathReceipt.state, 'failed');
  assert.equal(pathReceipt.reason, 'atomic_ref_cas_unavailable');
  assert.deepEqual(withoutPath.mutations, []);
});

test('publisher replay returns the same successful receipt without another mutation', async () => {
  const github = mockGitHub();
  const first = await publishWriterCandidate(options(github));
  const second = await publishWriterCandidate(options(github));
  assert.equal(first.state, 'draft_created');
  assert.deepEqual(second, first);
  assert.deepEqual(github.mutations, ['create_ref', 'create_pr']);
});

test('disabled canary exercises all read fences without any live mutation', async () => {
  const github = mockGitHub();
  const receipt = await publishWriterCandidate(options(github, candidate(), { dryRun: true }));
  assert.equal(receipt.state, 'cancelled');
  assert.equal(receipt.reason, 'disabled_canary');
  assert.deepEqual(github.mutations, []);
});

test('post-ref command drift and ambiguous responses produce non-destructive residue receipts', async () => {
  const drifted = mockGitHub({ boundaryDriftAt: 4 });
  const driftReceipt = await publishWriterCandidate(options(drifted));
  assert.equal(driftReceipt.state, 'residue');
  assert.deepEqual(drifted.mutations, ['create_ref']);

  const ambiguous = mockGitHub({ ambiguous: true });
  const ambiguousReceipt = await publishWriterCandidate(options(ambiguous));
  assert.equal(ambiguousReceipt.state, 'residue');
  assert.deepEqual(ambiguous.mutations, ['create_ref']);
});

test('exact mutation fence is the final read sequence and catches main or PR races before write', async () => {
  const normal = mockGitHub();
  const receipt = await publishWriterCandidate(options(normal));
  assert.equal(receipt.state, 'draft_created');
  const createRefIndex = normal.timeline.indexOf('create_ref');
  assert.deepEqual(normal.timeline.slice(createRefIndex - 3, createRefIndex), ['main', 'ref', 'pulls']);
  const createPrIndex = normal.timeline.indexOf('create_pr');
  assert.deepEqual(normal.timeline.slice(createPrIndex - 3, createPrIndex), ['main', 'ref', 'pulls']);

  const movedMain = mockGitHub({ mainDriftAtBoundary: 3 });
  const movedReceipt = await publishWriterCandidate(options(movedMain));
  assert.equal(movedReceipt.state, 'failed');
  assert.deepEqual(movedMain.mutations, []);

  const concurrentPull = mockGitHub({ injectPullAtBoundary: 3 });
  const concurrentReceipt = await publishWriterCandidate(options(concurrentPull));
  assert.equal(concurrentReceipt.state, 'residue');
  assert.deepEqual(concurrentPull.mutations, []);
});

test('post-create readback treats main drift as residue instead of a successful receipt', async () => {
  const github = mockGitHub({ postCreateMainDrift: true });
  const receipt = await publishWriterCandidate(options(github));
  assert.equal(receipt.state, 'residue');
  assert.equal(receipt.reason, 'post_pr_create_drift');
  assert.deepEqual(github.mutations, ['create_ref', 'create_pr']);
});

test('retry publish verifies its signed receipt but refuses metadata replacement without CAS', async () => {
  const retry = candidate({
    intent: {
      ...candidate().intent,
      command: '/agent retry-write',
      source_sha: baseSha,
      expected_remote_head: baseSha,
      comment_id: 92,
    },
    fix_cycle: 1,
  });
  const priorBody = `${signedReceipt({ cycle: 0, commentId: 91 })}\n\n<!-- aeris-writer-managed -->`;
  const success = mockGitHub({ existing: true, existingPullBody: priorBody, command: '/agent retry-write', commentId: 92 });
  const successReceipt = await publishWriterCandidate(options(success, retry));
  assert.equal(successReceipt.state, 'stale', JSON.stringify(successReceipt));
  assert.equal(successReceipt.reason, 'managed_pr_metadata_change_requires_cas');
  assert.deepEqual(success.mutations, []);

  const missing = mockGitHub({ existing: true, command: '/agent retry-write', commentId: 92 });
  const missingReceipt = await publishWriterCandidate(options(missing, retry));
  assert.equal(missingReceipt.reason, 'managed_pr_receipt_changed');
  assert.deepEqual(missing.mutations, []);

  const drifted = mockGitHub({
    existing: true,
    existingPullBody: priorBody,
    pullTitleDriftAtBoundary: 5,
    command: '/agent retry-write',
    commentId: 92,
  });
  const driftReceipt = await publishWriterCandidate(options(drifted, retry));
  assert.equal(driftReceipt.state, 'stale');
  assert.equal(driftReceipt.reason, 'managed_pr_metadata_change_requires_cas');
  assert.deepEqual(drifted.mutations, []);
});

test('publisher permanently rejects reopened and concurrently tombstoned PRs with a valid prior receipt', async () => {
  const retry = candidate({
    intent: {
      ...candidate().intent,
      command: '/agent retry-write',
      source_sha: baseSha,
      expected_remote_head: baseSha,
      comment_id: 92,
    },
    fix_cycle: 1,
  });
  const priorBody = `${signedReceipt({ cycle: 0, commentId: 91 })}\n\n<!-- aeris-writer-managed -->`;
  const github = mockGitHub({
    existing: true,
    existingPullBody: priorBody,
    tombstoned: true,
    command: '/agent retry-write',
    commentId: 92,
  });

  const receipt = await publishWriterCandidate(options(github, retry));
  assert.equal(receipt.state, 'stale');
  assert.equal(receipt.reason, 'issue_tombstoned');
  assert.deepEqual(github.mutations, []);

  const raced = mockGitHub({
    existing: true,
    existingPullBody: priorBody,
    tombstoneAtBoundary: 2,
    command: '/agent retry-write',
    commentId: 92,
  });
  const racedReceipt = await publishWriterCandidate(options(raced, retry));
  assert.equal(racedReceipt.state, 'stale');
  assert.equal(racedReceipt.reason, 'managed_pr_metadata_change_requires_cas');
  assert.deepEqual(raced.mutations, []);
});

test('publisher rejects stale policy, config, base, artifact commit, and autonomy expiry before mutation', async () => {
  const fixtures = [
    { overrides: { currentPolicySha: sha('9') }, reason: 'policy_sha_changed' },
    { overrides: { currentConfigSha: sha('9') }, reason: 'config_sha_changed' },
    { overrides: { verifyCandidateCommit: async () => false }, reason: 'candidate_commit_mismatch' },
    { overrides: { clock: () => new Date('2026-08-19T10:58:00Z') }, reason: 'autonomy_expiry_margin' },
  ];
  for (const fixture of fixtures) {
    const github = mockGitHub();
    const receipt = await publishWriterCandidate(options(github, candidate(), fixture.overrides));
    assert.equal(receipt.reason, fixture.reason);
    assert.deepEqual(github.mutations, []);
  }
  const movedMain = mockGitHub({ mainSha: sha('8') });
  const baseReceipt = await publishWriterCandidate(options(movedMain));
  assert.equal(baseReceipt.reason, 'base_main_changed');
  assert.deepEqual(movedMain.mutations, []);
});

test('expiry crossing after ref creation stops before PR creation and reports residue', async () => {
  const github = mockGitHub();
  const receipt = await publishWriterCandidate(options(github, candidate(), {
    clock: () => new Date(github.mutations.length === 0 ? '2026-08-19T10:30:00Z' : '2026-08-19T10:58:00Z'),
  }));
  assert.equal(receipt.state, 'residue');
  assert.equal(receipt.reason, 'autonomy_expiry_margin');
  assert.deepEqual(github.mutations, ['create_ref']);
});

test('duplicate and manually-owned PR histories fail closed before mutation', async () => {
  const retry = candidate({
    intent: {
      ...candidate().intent,
      command: '/agent retry-write',
      source_sha: baseSha,
      expected_remote_head: baseSha,
    },
    fix_cycle: 1,
  });
  for (const github of [mockGitHub({ existing: true, duplicate: true }), mockGitHub({ existing: true, manual: true })]) {
    const receipt = await publishWriterCandidate(options(github, retry));
    assert.equal(receipt.state, 'stale');
    assert.deepEqual(github.mutations, []);
  }
});
