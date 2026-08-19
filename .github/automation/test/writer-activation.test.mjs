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
  runWriterPublish,
} from '../src/writer-activation.mjs';
import {
  createWriterReceiptMarker,
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
  user = { type: 'Bot', login: 'aeris-writer[bot]' },
  writerLifecycle = { epoch: WRITER_LIFECYCLE_EPOCH, tombstoned: false },
} = {}) {
  const fields = {
    issueNumber: 12,
    commentId,
    lifecycleEpoch: WRITER_LIFECYCLE_EPOCH,
    fixCycle: cycle,
    candidateSha: sha(String(cycle + 1), 64),
    commitSha: headSha,
  };
  const receipt = createWriterReceiptMarker({ ...fields, signature: receiptSigner(fields) });
  return {
    number: 77,
    state: 'open',
    merged: false,
    draft: true,
    body: `${receipt}\n\n${WRITER_OWNERSHIP_MARKER}`,
    user,
    performed_via_github_app: { id: 456, slug: 'aeris-writer' },
    writer_lifecycle: writerLifecycle,
    base: { ref: 'main', repo: { id: 1_316_750_512, full_name: 'JinPengGeng/aeris-token' } },
    head: { ref: 'agent/issue-12', sha: headSha, repo: { id: 1_316_750_512, full_name: 'JinPengGeng/aeris-token' } },
  };
}

function deriveCycle(pull, currentCommentId) {
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
    sourceSha: sha('a'),
    pulls: [pull],
    writerApp: { id: 456, slug: 'aeris-writer' },
    receiptPublicKey,
  });
}

test('retry cycle is derived from the verified persistent PR receipt and increases across runs', () => {
  assert.equal(deriveCycle(retryPull({ cycle: 0, commentId: 91 }), 92).fixCycle, 1);
  assert.equal(deriveCycle(retryPull({ cycle: 1, commentId: 92 }), 93).fixCycle, 2);
});

test('retry replay, exhausted budget, forged receipt, and non-App ownership fail closed', () => {
  assert.equal(deriveCycle(retryPull({ cycle: 1, commentId: 92 }), 92).reason, 'retry_comment_already_applied');
  assert.equal(deriveCycle(retryPull({ cycle: 2, commentId: 93 }), 94).reason, 'maximum_fix_cycles_exceeded');
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

test('ready publish crosses the real Writer client constructor and publishes with separately wired receipt dependencies', async () => {
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
  git(buildRoot, ['commit', '--no-gpg-sign', '-m', 'writer: implement issue #12'], commitEnvironment);
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
    if (requestPath === '/repos/JinPengGeng/aeris-token/issues/comments/91') return json({
      id: 91,
      body: '/agent implement',
      issue_url: intent.issue_url,
      user: { login: 'maintainer' },
    });
    if (requestPath === '/repos/JinPengGeng/aeris-token/issues/12') return json({
      number: 12,
      state: 'open',
      updated_at: intent.issue_updated_at,
      url: intent.issue_url,
      labels: [{ name: 'agent-ready' }],
    });
    if (requestPath.endsWith('/collaborators/maintainer/permission')) return json({ permission: 'write' });
    if (requestPath.endsWith('/git/ref/heads/main')) return json({ ref: 'refs/heads/main', object: { type: 'commit', sha: baseSha } });
    if (requestPath.endsWith('/git/ref/heads/agent%2Fissue-12')) {
      return branchSha === null ? json({ message: 'Not Found' }, 404) : json({
        ref: 'refs/heads/agent/issue-12', object: { type: 'commit', sha: branchSha },
      });
    }
    if (requestPath.endsWith('/git/refs') && method === 'POST') {
      branchSha = JSON.parse(init.body).sha;
      mutations.push('create_ref');
      return json({ ref: 'refs/heads/agent/issue-12', object: { type: 'commit', sha: branchSha } }, 201);
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
        title: body.title,
        body: body.body,
        html_url: 'https://github.com/JinPengGeng/aeris-token/pull/77',
        user: { type: 'Bot', login: 'aeris-writer[bot]' },
        performed_via_github_app: { id: 456, slug: 'aeris-writer' },
        base: { ref: 'main', repo: { id: intent.repository_id, full_name: intent.repository_name } },
        head: { ref: 'agent/issue-12', sha: branchSha, repo: { id: intent.repository_id, full_name: intent.repository_name } },
      };
      return json({ number: 77 }, 201);
    }
    if (requestPath === '/repos/JinPengGeng/aeris-token/pulls/77') return json(pull);
    if (requestPath === '/repos/JinPengGeng/aeris-token/pulls') return json(pull === null ? [] : [pull]);
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
      repositoryPath: null,
    });
    assert.equal(result.state, 'complete', JSON.stringify(result));
    assert.equal(result.payload.receipt.state, 'draft_created');
    assert.deepEqual(mutations, ['create_ref', 'create_pr']);
    assert.match(pull.body, /aeris-writer-receipt:v2 .* epoch=0 /);
  } finally {
    const resolved = path.resolve(temporaryRoot);
    assert.ok(resolved.startsWith(path.resolve(os.tmpdir()) + path.sep));
    fs.rmSync(resolved, { recursive: true, force: true });
  }
});
