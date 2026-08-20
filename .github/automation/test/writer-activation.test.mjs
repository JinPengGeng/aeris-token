import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash, generateKeyPairSync } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  createWriterAppJwt,
  createWriterReceiptSigner,
  deriveNextWriterFixCycle,
  runWriterAnalyze,
  runWriterBuild,
  runWriterPreflight,
  runWriterPublish,
} from '../src/writer-activation.mjs';
import { GitHubClient } from '../src/github-client.mjs';
import {
  createWriterReceiptMarker,
  writerCommitMessage,
  writerPullMetadataDigest,
  WRITER_LIFECYCLE_EPOCH,
  WRITER_OWNERSHIP_MARKER,
} from '../src/writer-lifecycle.mjs';
import { calculateWriterCandidateSha } from '../src/writer-phase-contract.mjs';

const sha = (character, length = 40) => character.repeat(length);
const receiptKeys = generateKeyPairSync('rsa', { modulusLength: 2048 });
const receiptPrivateKey = receiptKeys.privateKey.export({ type: 'pkcs8', format: 'pem' });
const receiptPublicKey = receiptKeys.publicKey.export({ type: 'spki', format: 'pem' });
const receiptSigner = createWriterReceiptSigner(receiptPrivateKey, receiptPublicKey);
const sourceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..');

function git(root, args, environment = process.env) {
  return execFileSync('git', args, {
    cwd: root,
    encoding: 'utf8',
    windowsHide: true,
    env: environment,
  }).trim();
}

function sha256(value) {
  return createHash('sha256').update(value, 'utf8').digest('hex');
}

const canary = {
  schema_version: 1,
  artifact_type: 'writer_activation',
  phase: 'preflight',
  state: 'canary',
  reason: null,
  payload: { policy_sha: 'a'.repeat(40), run_id: 'canary-1' },
};

test('disabled canary traverses analyze, build, and publish without clients, secrets, or mutations', async () => {
  const analyzed = await runWriterAnalyze({ artifact: canary, environment: {}, repoRoot: 'unused' });
  const built = runWriterBuild({ artifact: analyzed, repoRoot: 'unused' });
  const published = await runWriterPublish({ artifact: built, environment: {}, repoRoot: 'unused' });
  assert.equal(analyzed.state, 'canary');
  assert.equal(built.state, 'canary');
  assert.equal(published.state, 'canary');
  assert.equal(published.reason, 'disabled_canary');
  assert.equal(published.payload.mutations, 0);
});

test('Writer App JWT is short-lived RS256 and binds the numeric App ID', () => {
  const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  const jwt = createWriterAppJwt(456, privateKey.export({ type: 'pkcs8', format: 'pem' }), new Date('2026-08-19T10:00:00Z'));
  const [header, payload, signature] = jwt.split('.');
  assert.deepEqual(JSON.parse(Buffer.from(header, 'base64url')), { alg: 'RS256', typ: 'JWT' });
  const claims = JSON.parse(Buffer.from(payload, 'base64url'));
  assert.equal(claims.iss, '456');
  assert.equal(claims.exp - claims.iat, 600);
  assert.ok(signature.length > 100);
});

function retryPull({
  cycle,
  commentId,
  headSha = sha('a'),
  receiptCommitSha = headSha,
  user = { type: 'Bot', login: 'aeris-writer[bot]' },
  writerLifecycle = { epoch: WRITER_LIFECYCLE_EPOCH, tombstoned: false },
} = {}) {
  const fields = {
    issueNumber: 12,
    commentId,
    lifecycleEpoch: WRITER_LIFECYCLE_EPOCH,
    fixCycle: cycle,
    candidateSha: sha(String(cycle + 1), 64),
    commitSha: receiptCommitSha,
  };
  const receipt = createWriterReceiptMarker({ ...fields, signature: receiptSigner(fields) });
  return {
    number: 77,
    state: 'open',
    merged: false,
    draft: true,
    locked: false,
    active_lock_reason: null,
    title: 'Writer retry',
    body: `${receipt}\n\n${WRITER_OWNERSHIP_MARKER}`,
    html_url: 'https://github.com/JinPengGeng/aeris-token/pull/77',
    maintainer_can_modify: false,
    labels: [],
    milestone: null,
    assignee: null,
    assignees: [],
    requested_reviewers: [],
    requested_teams: [],
    auto_merge: null,
    merge_queue_entry: null,
    user,
    performed_via_github_app: { id: 456, slug: 'aeris-writer' },
    writer_lifecycle: writerLifecycle,
    base: { ref: 'main', repo: { id: 1_316_750_512, full_name: 'JinPengGeng/aeris-token' } },
    head: { ref: 'agent/issue-12', sha: headSha, repo: { id: 1_316_750_512, full_name: 'JinPengGeng/aeris-token' } },
  };
}

function deriveCycle(pull, currentCommentId, compare = null) {
  return deriveNextWriterFixCycle({
    configuredRepository: {
      repository_id: 1_316_750_512,
      repository_name: 'JinPengGeng/aeris-token',
      limits: { maximum_fix_cycles: 2 },
    },
    issue: { number: 12 },
    comment: { id: currentCommentId },
    branch: 'agent/issue-12',
    mainSha: sha('b'),
    sourceSha: pull.head.sha,
    pulls: [pull],
    writerApp: { id: 456, slug: 'aeris-writer' },
    receiptPublicKey,
    compare,
  });
}

test('retry cycle is derived from the verified persistent PR receipt and increases across runs', () => {
  const origin = retryPull({ cycle: 0, commentId: 91 });
  assert.equal(deriveCycle(origin, 92).fixCycle, 1);
  const firstRetrySha = sha('b');
  const firstCandidateSha = sha('2', 64);
  const advanced = retryPull({ cycle: 0, commentId: 91, headSha: firstRetrySha, receiptCommitSha: sha('a') });
  const compare = {
    status: 'ahead', ahead_by: 1, total_commits: 1,
    base_commit: { sha: sha('a') }, merge_base_commit: { sha: sha('a') },
    commits: [{
      sha: firstRetrySha,
      parents: [{ sha: sha('a') }],
      commit: { message: writerCommitMessage({
        issueNumber: 12,
        fixCycle: 1,
        commentId: 92,
        candidateSha: firstCandidateSha,
        pullMetadataSha: writerPullMetadataDigest(origin),
      }) },
    }],
  };
  assert.equal(deriveCycle(advanced, 93, compare).fixCycle, 2);
  assert.equal(deriveCycle(advanced, 92, compare).reason, 'retry_comment_already_applied');
});

test('retry replay, non-origin receipts, forged receipt, and non-App ownership fail closed', () => {
  assert.equal(deriveCycle(retryPull({ cycle: 0, commentId: 92 }), 92).reason, 'retry_comment_already_applied');
  assert.equal(deriveCycle(retryPull({ cycle: 2, commentId: 93 }), 94).reason, 'managed_pr_receipt_invalid');
  assert.equal(deriveCycle(retryPull({ cycle: 1, commentId: 91 }), 92).reason, 'managed_pr_receipt_invalid');
  assert.equal(deriveCycle({ ...retryPull({ cycle: 0, commentId: 91 }), body: WRITER_OWNERSHIP_MARKER }, 92).reason, 'managed_pr_receipt_invalid');
  const signed = retryPull({ cycle: 0, commentId: 91 });
  assert.equal(
    deriveCycle({ ...signed, body: signed.body.replace('cycle=0', 'cycle=1') }, 92).reason,
    'managed_pr_receipt_invalid',
  );
  const receiptLine = signed.body.split('\n')[0];
  assert.equal(
    deriveCycle({ ...signed, body: `${receiptLine}\nforged ${receiptLine}\n\n${WRITER_OWNERSHIP_MARKER}` }, 92).reason,
    'managed_pr_receipt_invalid',
  );
  assert.equal(
    deriveCycle(retryPull({ cycle: 0, commentId: 91, user: { type: 'User', login: 'maintainer' } }), 92).reason,
    'managed_pr_writer_identity_invalid',
  );
  assert.equal(deriveCycle(retryPull({
    cycle: 0,
    commentId: 91,
    writerLifecycle: { epoch: WRITER_LIFECYCLE_EPOCH, tombstoned: true },
  }), 92).reason, 'issue_tombstoned');
  const legacy = '<!-- aeris-writer-receipt:v1 issue=12 comment=91 cycle=0 candidate=' +
    `${sha('1', 64)} commit=${sha('a')} signature=${'A'.repeat(342)} -->`;
  assert.equal(deriveCycle({ ...retryPull({ cycle: 0, commentId: 91 }), body: `${legacy}\n\n${WRITER_OWNERSHIP_MARKER}` }, 92).reason,
    'managed_pr_receipt_invalid');
});

test('receipt signer rejects a public key that does not match the App private key', () => {
  const other = generateKeyPairSync('rsa', { modulusLength: 2048 }).publicKey.export({ type: 'spki', format: 'pem' });
  assert.throws(() => createWriterReceiptSigner(receiptPrivateKey, other), /does not match/);
});

test('implement and retry cross preflight, real Writer client compare, lease push, reconciliation, and replay fences', async () => {
  const temporaryRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-writer-ready-'));
  const repoRoot = path.join(temporaryRoot, 'repo');
  const buildRoot = path.join(temporaryRoot, 'build');
  fs.mkdirSync(path.join(repoRoot, '.github'), { recursive: true });
  fs.mkdirSync(path.join(repoRoot, 'docs'), { recursive: true });
  for (const name of ['agents.yml', 'automation-policy.yml']) {
    let source = fs.readFileSync(path.join(sourceRoot, '.github', name), 'utf8');
    if (name === 'agents.yml') source = source.replace(/(\n  writer:\r?\n    enabled:) false/, '$1 true');
    else source = source.replace(/(\nwriter:\r?\n  enabled:) false/, '$1 true');
    fs.writeFileSync(path.join(repoRoot, '.github', name), source);
  }
  fs.writeFileSync(path.join(repoRoot, 'docs', 'writer-base.md'), 'base\n');
  git(repoRoot, ['init', '--initial-branch=main']);
  git(repoRoot, ['config', 'user.name', 'Test']);
  git(repoRoot, ['config', 'user.email', 'test@example.invalid']);
  git(repoRoot, ['add', '.']);
  git(repoRoot, ['commit', '-m', 'base']);
  const baseSha = git(repoRoot, ['rev-parse', 'HEAD']);
  git(temporaryRoot, ['clone', '--quiet', repoRoot, buildRoot]);
  fs.writeFileSync(path.join(buildRoot, 'docs', 'writer-ready.md'), 'ready path\n');
  git(buildRoot, ['add', 'docs/writer-ready.md']);
  const patch = `${git(buildRoot, ['diff', '--cached', '--binary'])}\n`;
  const intent = {
    repository_id: 1_316_750_512,
    repository_name: 'JinPengGeng/aeris-token',
    issue_number: 12,
    issue_url: 'https://api.github.com/repos/JinPengGeng/aeris-token/issues/12',
    issue_updated_at: '2026-08-19T10:00:00Z',
    issue_labels: ['agent-ready'],
    input_sha: sha('b', 64),
    comment_id: 91,
    actor: 'maintainer',
    command: '/agent implement',
    base_sha: baseSha,
    source_sha: baseSha,
    policy_sha: baseSha,
    config_sha: baseSha,
    run_id: 'ready-integration',
    agent: 'writer',
    branch: 'agent/issue-12',
    expected_remote_head: null,
    pull_metadata_sha: null,
    lease_token: sha('e', 64),
    cancel_epoch: 0,
    lease_expires_at: '2026-08-19T11:00:00Z',
  };
  const candidate = {
    schema_version: 2,
    artifact_type: 'candidate',
    state: 'ready',
    intent,
    patch_sha: sha256(patch),
    changed_paths: ['docs/writer-ready.md'],
    file_sizes: [{ path: 'docs/writer-ready.md', bytes: 11 }],
    file_count: 1,
    patch_bytes: Buffer.byteLength(patch),
    total_file_bytes: 11,
    limits: {
      maximum_files: 50,
      maximum_patch_bytes: 65536,
      maximum_file_size_bytes: 524288,
      maximum_total_file_bytes: 2097152,
      maximum_fix_cycles: 2,
    },
    fix_cycle: 0,
    tests: { state: 'passed', plan_ids: ['diff-check-v1'], summary: 'ready integration passed' },
  };
  candidate.candidate_sha = calculateWriterCandidateSha(candidate);
  const commitEnvironment = {
    ...process.env,
    GIT_AUTHOR_NAME: 'Aeris Writer',
    GIT_AUTHOR_EMAIL: 'aeris-writer@users.noreply.github.com',
    GIT_COMMITTER_NAME: 'Aeris Writer',
    GIT_COMMITTER_EMAIL: 'aeris-writer@users.noreply.github.com',
    GIT_AUTHOR_DATE: intent.issue_updated_at,
    GIT_COMMITTER_DATE: intent.issue_updated_at,
  };
  git(buildRoot, ['commit', '--no-gpg-sign', '-m', writerCommitMessage({
    issueNumber: 12,
    fixCycle: 0,
    commentId: 91,
    candidateSha: candidate.candidate_sha,
  })], commitEnvironment);
  const commitSha = git(buildRoot, ['rev-parse', 'HEAD']);
  const artifact = {
    schema_version: 1,
    artifact_type: 'writer_activation',
    phase: 'build',
    state: 'ready',
    reason: null,
    payload: { candidate, commit_sha: commitSha, patch, metadata: { title: 'Ready path', body: 'Ready body' } },
  };
  let branchSha = null;
  let pull = null;
  const mutations = [];
  const leasePushes = [];
  const compareRequests = [];
  let crashAfterNextAdvance = false;
  let failNextRefRead = false;
  const writerExecFileImpl = async (command, args, options) => {
    assert.equal(command, 'git');
    assert.equal(options.cwd, repoRoot);
    if (args[0] === 'cat-file') {
      assert.equal(args[1], '-e');
      assert.match(args[2], /^[0-9a-f]{40}\^\{commit\}$/);
      git(repoRoot, ['cat-file', '-e', args[2]]);
      return { stdout: '', stderr: '' };
    }
    if (args[0] === 'remote') {
      assert.deepEqual(args, ['remote', 'get-url', '--push', 'origin']);
      return { stdout: 'https://github.com/JinPengGeng/aeris-token.git\n', stderr: '' };
    }
    assert.equal(args[0], 'push');
    assert.equal(args[1], '--porcelain');
    const expectedOldSha = args[2].slice('--force-with-lease=refs/heads/agent/issue-12:'.length);
    const newCommitSha = args[4].split(':')[0];
    assert.equal(expectedOldSha, branchSha ?? '');
    assert.equal(args[3], 'origin');
    assert.equal(args[4], `${newCommitSha}:refs/heads/agent/issue-12`);
    leasePushes.push({ expectedOldSha, newCommitSha });
    const action = branchSha === null ? 'create_ref' : 'advance_ref';
    mutations.push(action);
    branchSha = newCommitSha;
    if (pull) pull = { ...pull, head: { ...pull.head, sha: branchSha } };
    if (action === 'advance_ref' && crashAfterNextAdvance) {
      crashAfterNextAdvance = false;
      failNextRefRead = true;
      throw new Error('simulated process interruption after successful push');
    }
    return { stdout: '', stderr: '' };
  };
  const json = (value, status = 200) => new Response(JSON.stringify(value), { status });
  const fetchImpl = async (url, init) => {
    const requestPath = new URL(url).pathname;
    const method = init.method;
    if (requestPath === '/app') return json({ id: 456, slug: 'aeris-writer' });
    if (requestPath === '/repos/JinPengGeng/aeris-token/installation') return json({
      id: 789,
      app_id: 456,
      app_slug: 'aeris-writer',
      repository_selection: 'selected',
      account: { login: 'JinPengGeng' },
      permissions: { contents: 'write', pull_requests: 'write', metadata: 'read' },
    });
    if (requestPath === '/app/installations/789/access_tokens') return json({
      token: 'ready-installation-token',
      expires_at: new Date(Date.now() + 3_600_000).toISOString(),
      repository_selection: 'selected',
      permissions: { contents: 'write', pull_requests: 'write', metadata: 'read' },
    }, 201);
    if (requestPath === '/installation/repositories') return json({
      repository_selection: 'selected',
      total_count: 1,
      repositories: [{ id: intent.repository_id, full_name: intent.repository_name }],
    });
    if (requestPath === '/repos/JinPengGeng/aeris-token') return json({ id: intent.repository_id, full_name: intent.repository_name });
    const commentMatch = /^\/repos\/JinPengGeng\/aeris-token\/issues\/comments\/(91|92|93)$/.exec(requestPath);
    if (commentMatch) {
      const id = Number(commentMatch[1]);
      return json({
        id,
        body: id === 91 ? '/agent implement' : '/agent retry-write',
        issue_url: intent.issue_url,
        user: { login: 'maintainer' },
      });
    }
    if (requestPath === '/repos/JinPengGeng/aeris-token/issues/12') return json({
      number: 12,
      state: 'open',
      title: 'Writer retry integration',
      body: 'Exercise the bounded Writer retry path.',
      updated_at: intent.issue_updated_at,
      url: intent.issue_url,
      labels: [{ name: 'agent-ready' }],
    });
    if (requestPath.endsWith('/collaborators/maintainer/permission')) return json({ permission: 'write' });
    if (requestPath.endsWith('/git/ref/heads/main')) return json({ ref: 'refs/heads/main', object: { type: 'commit', sha: baseSha } });
    if (requestPath.endsWith('/git/ref/heads/agent%2Fissue-12')) {
      if (failNextRefRead) {
        failNextRefRead = false;
        return json({ message: 'simulated unavailable readback' }, 503);
      }
      return branchSha === null ? json({ message: 'Not Found' }, 404) : json({
        ref: 'refs/heads/agent/issue-12', object: { type: 'commit', sha: branchSha },
      });
    }
    const compareMatch = /^\/repos\/JinPengGeng\/aeris-token\/compare\/([0-9a-f]{40})\.\.\.([0-9a-f]{40})$/.exec(requestPath);
    if (compareMatch) {
      const [, compareBase, compareHead] = compareMatch;
      compareRequests.push({
        base: compareBase,
        head: compareHead,
        authorization: init.headers.authorization,
      });
      const mergeBase = git(repoRoot, ['merge-base', compareBase, compareHead]);
      const commitShas = git(repoRoot, ['rev-list', '--reverse', `${compareBase}..${compareHead}`])
        .split(/\r?\n/).filter(Boolean);
      return json({
        status: mergeBase === compareBase ? 'ahead' : 'diverged',
        ahead_by: commitShas.length,
        total_commits: commitShas.length,
        base_commit: { sha: compareBase },
        merge_base_commit: { sha: mergeBase },
        commits: commitShas.map((shaValue) => ({
          sha: shaValue,
          parents: git(repoRoot, ['show', '-s', '--format=%P', shaValue]).split(' ').filter(Boolean).map((parentSha) => ({ sha: parentSha })),
          commit: { message: git(repoRoot, ['show', '-s', '--format=%B', shaValue]) },
        })),
      });
    }
    if (requestPath.endsWith('/pulls') && method === 'POST') {
      const body = JSON.parse(init.body);
      mutations.push('create_pr');
      pull = {
        number: 77,
        state: 'open',
        merged: false,
        merged_at: null,
        draft: true,
        locked: false,
        active_lock_reason: null,
        title: body.title,
        body: body.body,
        maintainer_can_modify: false,
        labels: [{ id: 1, name: 'agent-ready', color: '00ff00' }],
        milestone: null,
        assignee: null,
        assignees: [],
        requested_reviewers: [],
        requested_teams: [],
        auto_merge: null,
        merge_queue_entry: null,
        html_url: 'https://github.com/JinPengGeng/aeris-token/pull/77',
        user: { type: 'Bot', login: 'aeris-writer[bot]' },
        performed_via_github_app: { id: 456, slug: 'aeris-writer' },
        base: { ref: 'main', repo: { id: intent.repository_id, full_name: intent.repository_name } },
        head: { ref: 'agent/issue-12', sha: branchSha, repo: { id: intent.repository_id, full_name: intent.repository_name } },
      };
      return json({ number: 77 }, 201);
    }
    if (requestPath === '/repos/JinPengGeng/aeris-token/pulls/77') return json(pull);
    if (requestPath === '/repos/JinPengGeng/aeris-token/pulls') {
      if (pull === null) return json([]);
      const { maintainer_can_modify: _omitted, ...listPull } = pull;
      return json([listPull]);
    }
    if (requestPath === '/repos/JinPengGeng/aeris-token/issues/77/timeline') return json([]);
    throw new Error(`unexpected ready-path request: ${method} ${url}`);
  };
  try {
    const result = await runWriterPublish({
      artifact,
      environment: {
        GITHUB_REPOSITORY: intent.repository_name,
        AERIS_AGENTS_ENABLED: 'true',
        AERIS_WRITER_ENABLED: 'true',
        AERIS_WRITER_APP_ID: '456',
        AERIS_WRITER_APP_SLUG: 'aeris-writer',
        AERIS_WRITER_PRIVATE_KEY: receiptPrivateKey,
        AERIS_WRITER_PUBLIC_KEY: receiptPublicKey,
      },
      repoRoot,
      clock: () => new Date('2026-08-19T10:30:00Z'),
      fetchImpl,
      repositoryPath: repoRoot,
      writerExecFileImpl,
    });
    assert.equal(result.state, 'complete', JSON.stringify(result));
    assert.equal(result.payload.receipt.state, 'draft_created');
    assert.deepEqual(mutations, ['create_ref', 'create_pr']);
    assert.match(pull.body, /aeris-writer-receipt:v2 .* epoch=0 /);

    const readonlyClient = new GitHubClient({
      token: 'readonly-token',
      repository: intent.repository_name,
      fetchImpl,
    });
    const writerEnvironment = {
      GITHUB_REPOSITORY: intent.repository_name,
      GITHUB_EVENT_NAME: 'issue_comment',
      GITHUB_RUN_ID: 'retry-integration',
      AERIS_AGENTS_ENABLED: 'true',
      AERIS_WRITER_ENABLED: 'true',
      AERIS_WRITER_APP_ID: '456',
      AERIS_WRITER_APP_SLUG: 'aeris-writer',
      AERIS_WRITER_PUBLIC_KEY: receiptPublicKey,
    };
    const eventFor = (commentId) => ({
      action: 'created',
      repository: { id: intent.repository_id },
      issue: { number: 12 },
      comment: {
        id: commentId,
        body: '/agent retry-write',
        author_association: 'OWNER',
        user: { login: 'maintainer', type: 'User' },
      },
      sender: { login: 'maintainer', type: 'User' },
    });
    const patchFor = (sourceSha, name, content) => {
      const scratch = path.join(temporaryRoot, `scratch-${name}`);
      git(temporaryRoot, ['clone', '--quiet', '--shared', '--no-checkout', repoRoot, scratch]);
      git(scratch, ['checkout', '--detach', sourceSha]);
      fs.writeFileSync(path.join(scratch, 'docs', name), content);
      git(scratch, ['add', `docs/${name}`]);
      return `${git(scratch, ['diff', '--cached', '--binary'])}\n`;
    };
    const runRetry = async (commentId, name, content, { crashAfterPush = false } = {}) => {
      git(repoRoot, ['checkout', '--detach', baseSha]);
      const preflight = await runWriterPreflight({
        event: eventFor(commentId),
        environment: writerEnvironment,
        repoRoot,
        github: readonlyClient,
        clock: () => new Date('2026-08-19T10:30:00Z'),
      });
      assert.equal(preflight.state, 'ready', JSON.stringify(preflight));
      const generatedPatch = patchFor(preflight.payload.intent.source_sha, name, content);
      const analysis = {
        schema_version: 1,
        artifact_type: 'writer_activation',
        phase: 'analyze',
        state: 'ready',
        reason: null,
        payload: {
          intent: preflight.payload.intent,
          fix_cycle: preflight.payload.fix_cycle,
          generated: { title: `Retry ${commentId}`, body: `Retry ${commentId} body`, patch: generatedPatch },
        },
      };
      git(repoRoot, ['checkout', '--detach', baseSha]);
      const built = runWriterBuild({ artifact: analysis, repoRoot });
      assert.equal(built.state, 'ready', JSON.stringify(built));
      git(repoRoot, ['checkout', '--detach', baseSha]);
      if (crashAfterPush) crashAfterNextAdvance = true;
      let published = await runWriterPublish({
        artifact: built,
        environment: { ...writerEnvironment, AERIS_WRITER_PRIVATE_KEY: receiptPrivateKey },
        repoRoot,
        clock: () => new Date('2026-08-19T10:30:00Z'),
        fetchImpl,
        repositoryPath: repoRoot,
        writerExecFileImpl,
      });
      if (crashAfterPush) {
        assert.equal(published.payload.receipt.state, 'residue', JSON.stringify(published));
        assert.equal(published.payload.receipt.reason, 'ambiguous_platform_residue');
        git(repoRoot, ['checkout', '--detach', baseSha]);
        published = await runWriterPublish({
          artifact: built,
          environment: { ...writerEnvironment, AERIS_WRITER_PRIVATE_KEY: receiptPrivateKey },
          repoRoot,
          clock: () => new Date('2026-08-19T10:30:00Z'),
          fetchImpl,
          repositoryPath: repoRoot,
          writerExecFileImpl,
        });
      }
      assert.equal(published.state, 'complete', JSON.stringify(published));
      assert.equal(published.payload.receipt.state, 'draft_updated', JSON.stringify(published));
      assert.equal(published.payload.receipt.reason, 'branch_updated_metadata_preserved');
      assert.equal(published.payload.receipt.commit_sha, branchSha);
      return { preflight, built, published };
    };

    const bodyAfterCreate = pull.body;
    const firstRetry = await runRetry(92, 'writer-retry-1.md', 'retry one\n', { crashAfterPush: true });
    assert.equal(firstRetry.preflight.payload.fix_cycle, 1);
    assert.equal(pull.body, bodyAfterCreate);

    const secondRetry = await runRetry(93, 'writer-retry-2.md', 'retry two\n');
    assert.equal(secondRetry.preflight.payload.fix_cycle, 2);
    assert.equal(pull.body, bodyAfterCreate);
    assert.deepEqual(mutations, ['create_ref', 'create_pr', 'advance_ref', 'advance_ref']);
    assert.equal(leasePushes.length, 3);
    assert.equal(leasePushes[1].expectedOldSha, commitSha);
    assert.equal(leasePushes[2].expectedOldSha, firstRetry.built.payload.commit_sha);
    assert.ok(compareRequests.some(({ authorization }) => authorization === 'Bearer ready-installation-token'));

    git(repoRoot, ['checkout', '--detach', baseSha]);
    const replay = await runWriterPreflight({
      event: eventFor(93),
      environment: writerEnvironment,
      repoRoot,
      github: readonlyClient,
      clock: () => new Date('2026-08-19T10:30:00Z'),
    });
    assert.equal(replay.state, 'terminal');
    assert.equal(replay.reason, 'retry_comment_already_applied');
    assert.deepEqual(mutations, ['create_ref', 'create_pr', 'advance_ref', 'advance_ref']);
  } finally {
    const resolved = path.resolve(temporaryRoot);
    assert.ok(resolved.startsWith(path.resolve(os.tmpdir()) + path.sep));
    fs.rmSync(resolved, { recursive: true, force: true });
  }
});
