import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import test from 'node:test';

import {
  artifactSha,
  canonicalJson,
  validateConflictBundle,
  validateConflictCandidate,
  validateModelCandidates,
  validateReviewReceipt,
} from '../src/sync-conflict-contract.mjs';
import {
  buildConflictBundle,
  collectReviewInput,
  finalizeConflictReview,
  materializeConflict,
  resolveConflict,
  reviewConflict,
  verifyAttestationBinding,
} from '../src/sync-conflict-review.mjs';

function git(repo, args, options = {}) {
  return execFileSync('git', args, {
    cwd: repo,
    encoding: options.encoding ?? 'utf8',
    input: options.input,
    env: { ...process.env, ...options.env },
  }).trim();
}

function write(repo, relative, text) {
  const destination = path.join(repo, relative);
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, text, 'utf8');
}

function commit(repo, message) {
  git(repo, ['add', '.']);
  git(repo, ['commit', '-m', message]);
  return git(repo, ['rev-parse', 'HEAD']);
}

function repositoryFixture() {
  const repo = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-sync-conflict-test-'));
  git(repo, ['init', '-b', 'main']);
  git(repo, ['config', 'user.name', 'test']);
  git(repo, ['config', 'user.email', 'test@example.invalid']);
  write(repo, 'shared.txt', 'base\n');
  write(repo, '.github/upstream-sync-policy.yml', 'version: 1\n');
  write(repo, '.github/ai-executors.json', `${JSON.stringify({
    schema_version: 1,
    executors: [
      { id: 'openai-chat-v1', protocol: 'openai-chat-completions-v1' },
      { id: 'openai-responses-v1', protocol: 'openai-responses-v1' },
    ],
    routes: {
      agent_analysis: 'openai-chat-v1',
      sync_conflict_resolver: 'openai-chat-v1',
      sync_conflict_reviewer: 'openai-chat-v1',
    },
  })}\n`);
  write(repo, '.github/upstream-sync-state.json', `${JSON.stringify({ last_integrated_sha: '0'.repeat(40) })}\n`);
  const checkpoint = commit(repo, 'checkpoint');
  write(repo, '.github/upstream-sync-state.json', `${JSON.stringify({ last_integrated_sha: checkpoint })}\n`);
  const stateCommit = commit(repo, 'bind checkpoint state');

  git(repo, ['switch', '-c', 'upstream', stateCommit]);
  write(repo, 'shared.txt', 'upstream behavior\n');
  const upstream = commit(repo, 'upstream change');

  git(repo, ['switch', '-c', 'fork', stateCommit]);
  write(repo, 'shared.txt', 'fork behavior\n');
  const base = commit(repo, 'fork change');

  const environment = {
    ...process.env,
    GITHUB_REPOSITORY: 'example/aeris-token',
    GITHUB_REPOSITORY_ID: '123',
    BASE_BRANCH: 'main',
    SYNC_BRANCH: 'automation/sync-upstream',
    AERIS_CONFLICT_BASE_SHA: base,
    AERIS_CONFLICT_CHECKPOINT_SHA: stateCommit,
    AERIS_CONFLICT_UPSTREAM_REPOSITORY: 'upstream/aether',
    AERIS_CONFLICT_UPSTREAM_REF: 'main',
    AERIS_CONFLICT_UPSTREAM_SHA: upstream,
    AERIS_CONFLICT_SYNTHETIC_COMMIT_SHA: upstream,
    AERIS_CONFLICT_POLICY_PATH: '.github/upstream-sync-policy.yml',
    AERIS_CONFLICT_STATE_PATH: '.github/upstream-sync-state.json',
    AERIS_SYNC_POLICY_VERDICT: 'eligible',
    AERIS_AI_MODEL_CONFLICT_RESOLVER: 'resolver-model',
    AERIS_AI_MODEL_CONFLICT_REVIEWER: 'reviewer-model',
    AERIS_ARTIFACT_ROOT: repo,
    GITHUB_RUN_ID: '456',
    GITHUB_RUN_ATTEMPT: '2',
  };
  return { repo, checkpoint: stateCommit, upstream, base, environment };
}

function fakeClient(content, model) {
  return { async complete() { return { content: JSON.stringify(content), model, usage: null, durationMs: 1 }; } };
}

function updateStateTree(repo, resolvedTree, statePath, upstreamSha) {
  const index = path.join(repo, 'final.index');
  const environment = { GIT_INDEX_FILE: index };
  git(repo, ['read-tree', resolvedTree], { env: environment });
  const state = Buffer.from(`${JSON.stringify({ last_integrated_sha: upstreamSha }, null, 2)}\n`, 'utf8');
  const blob = git(repo, ['hash-object', '-w', '--stdin'], { input: state });
  git(repo, ['update-index', '--add', '--cacheinfo', '100644', blob, statePath], { env: environment });
  return git(repo, ['write-tree'], { env: environment });
}

function createPublishedHead(repo, bundle, candidate, materialization, headTree) {
  const messages = [
    'chore: sync upstream',
    'Sync-Upstream-Automation: true',
    `Sync-Upstream-Source: ${bundle.upstream.repository}@${bundle.upstream.sha}`,
    `Sync-Upstream-Checkpoint: ${bundle.checkpoint_sha}->${bundle.upstream.sha}`,
    `Sync-Upstream-Base: ${bundle.base_sha}`,
    'Sync-Upstream-Policy-Verdict: conflict_ai_review',
    'Sync-Upstream-Conflict-Profile: aeris-sync-conflict-v1',
    `Sync-Upstream-Conflict-Generation: ${bundle.generation_sha}`,
    `Sync-Upstream-Conflict-Bundle: ${artifactSha(bundle)}`,
    `Sync-Upstream-Resolution-Candidate: ${artifactSha(candidate)}`,
    `Sync-Upstream-Resolution-SHA: ${candidate.resolution_sha}`,
    `Sync-Upstream-Resolved-Merge-Tree: ${materialization.resolved_merge_tree_sha}`,
    `Sync-Upstream-Prepared-Tree: ${headTree}`,
    `Sync-Upstream-Resolver-Model-SHA: ${artifactSha(candidate.model)}`,
  ];
  const argumentsList = ['commit-tree', headTree, '-p', bundle.base_sha];
  for (const message of messages) argumentsList.push('-m', message);
  return git(repo, argumentsList, {
    env: {
      GIT_AUTHOR_NAME: 'github-actions[bot]', GIT_AUTHOR_EMAIL: 'bot@example.invalid',
      GIT_COMMITTER_NAME: 'github-actions[bot]', GIT_COMMITTER_EMAIL: 'bot@example.invalid',
    },
  });
}

function pullResponse({ bundle, pullNumber, headSha, mergeableState = 'clean' }) {
  return {
    number: pullNumber,
    state: 'open',
    merged: false,
    draft: false,
    head: { sha: headSha, ref: bundle.sync_ref, repo: { full_name: bundle.repository } },
    base: { sha: bundle.base_sha, ref: bundle.base_ref },
    auto_merge: null,
    mergeable: true,
    mergeable_state: mergeableState,
  };
}

test('conflict bundle, resolver, independent reviewer, and final attestation bind one exact published tree', async (t) => {
  const fixture = repositoryFixture();
  const previous = process.cwd();
  process.chdir(fixture.repo);
  t.after(() => {
    process.chdir(previous);
    fs.rmSync(fixture.repo, { recursive: true, force: true });
  });

  const bundle = buildConflictBundle({ environment: fixture.environment });
  assert.equal(bundle.conflicts.length, 1);
  assert.equal(bundle.conflicts[0].path, 'shared.txt');
  assert.match(bundle.conflicts[0].marker_content, /^<{7}/m);

  const candidate = await resolveConflict({
    bundle,
    environment: fixture.environment,
    client: fakeClient({
      schema_version: 1,
      verdict: 'resolved',
      summary: 'Preserve both behaviors in a deterministic order.',
      resolutions: [{ path: 'shared.txt', content: 'fork behavior\nupstream behavior\n' }],
    }, { alias: 'conflict-resolver', id: 'resolver-model' }),
  });
  const materialization = materializeConflict({ bundle, candidate, environment: fixture.environment });
  assert.equal(git(fixture.repo, ['show', `${materialization.resolved_merge_tree_sha}:shared.txt`]), 'fork behavior\nupstream behavior');

  const headTree = updateStateTree(
    fixture.repo,
    materialization.resolved_merge_tree_sha,
    bundle.policy.state_path,
    bundle.upstream.sha,
  );
  const headSha = createPublishedHead(fixture.repo, bundle, candidate, materialization, headTree);
  const pullNumber = 9;
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () => new Response(JSON.stringify(pullResponse({ bundle, pullNumber, headSha })), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
  t.after(() => { globalThis.fetch = originalFetch; });

  const publishedEnvironment = {
    ...fixture.environment,
    GITHUB_TOKEN: 'read-token',
    GITHUB_API_URL: 'https://api.github.test',
    AERIS_CONFLICT_PULL_NUMBER: String(pullNumber),
    AERIS_CONFLICT_HEAD_SHA: headSha,
    AERIS_CONFLICT_HEAD_TREE_SHA: headTree,
  };
  const input = await collectReviewInput({ bundle, candidate, environment: publishedEnvironment });
  assert.equal(input.head_sha, headSha);
  assert.equal(input.resolved_merge_tree_sha, materialization.resolved_merge_tree_sha);

  const receipt = await reviewConflict({
    input,
    bundle,
    candidate,
    environment: publishedEnvironment,
    client: fakeClient({ schema_version: 1, verdict: 'pass', summary: 'The exact result preserves both intended lines.', findings: [] },
      { alias: 'conflict-reviewer', id: 'reviewer-model' }),
  });
  assert.equal(receipt.output.verdict, 'pass');

  const attestation = await finalizeConflictReview({ bundle, candidate, input, receipt, environment: publishedEnvironment });
  assert.equal(attestation.decision, 'approved');
  assert.equal(attestation.head_sha, headSha);
  assert.deepEqual(attestation.resolver_executor, { id: 'openai-chat-v1', protocol: 'openai-chat-completions-v1' });
  assert.deepEqual(attestation.reviewer_executor, { id: 'openai-chat-v1', protocol: 'openai-chat-completions-v1' });
  assert.notEqual(attestation.resolver_model.id, attestation.reviewer_model.id);
  const staleReceipt = structuredClone(receipt);
  staleReceipt.run.attempt += 1;
  await assert.rejects(
    () => finalizeConflictReview({ bundle, candidate, input, receipt: staleReceipt, environment: publishedEnvironment }),
    /exact workflow run and attempt/,
  );
  const mergeBinding = {
    repository: bundle.repository,
    pullNumber,
    headSha,
    baseSha: bundle.base_sha,
    upstreamRepository: bundle.upstream.repository,
    upstreamSha: bundle.upstream.sha,
    attestationSha: artifactSha(attestation),
    runId: fixture.environment.GITHUB_RUN_ID,
    runAttempt: fixture.environment.GITHUB_RUN_ATTEMPT,
  };
  assert.equal(verifyAttestationBinding(attestation, mergeBinding).decision, 'approved');
  assert.throws(
    () => verifyAttestationBinding(attestation, { ...mergeBinding, runAttempt: '3' }),
    /exact workflow run and attempt/,
  );
});

test('contracts reject shared resolver/reviewer models, stale candidates, markers, and review findings', async (t) => {
  assert.throws(() => validateModelCandidates({
    resolver: [{ alias: 'resolver', id: 'same-model' }],
    reviewer: [{ alias: 'reviewer', id: 'same-model' }],
  }), /disjoint/);

  const fixture = repositoryFixture();
  const previous = process.cwd();
  process.chdir(fixture.repo);
  t.after(() => {
    process.chdir(previous);
    fs.rmSync(fixture.repo, { recursive: true, force: true });
  });
  const bundle = buildConflictBundle({ environment: fixture.environment });
  const candidate = await resolveConflict({
    bundle,
    environment: fixture.environment,
    client: fakeClient({
      schema_version: 1, verdict: 'resolved', summary: 'Resolved.',
      resolutions: [{ path: 'shared.txt', content: 'resolved\n' }],
    }, { alias: 'conflict-resolver', id: 'resolver-model' }),
  });

  const staleBundle = structuredClone(bundle);
  staleBundle.base_sha = 'a'.repeat(40);
  assert.throws(() => validateConflictBundle(staleBundle), /workflow SHA|generation hash/);

  const staleCandidate = structuredClone(candidate);
  staleCandidate.output.resolutions[0].content = '<<<<<<< ours\n';
  staleCandidate.resolution_sha = artifactSha(staleCandidate.output.resolutions);
  assert.throws(() => validateConflictCandidate(staleCandidate, bundle), /conflict markers/);
  const untrustedExecutor = structuredClone(candidate);
  untrustedExecutor.executor = { id: 'openai-responses-v1', protocol: 'openai-responses-v1' };
  assert.throws(() => validateConflictCandidate(untrustedExecutor, bundle), /resolver executor is not allowed/);

  const materialization = materializeConflict({ bundle, candidate, environment: fixture.environment });
  const headTree = updateStateTree(fixture.repo, materialization.resolved_merge_tree_sha, bundle.policy.state_path, bundle.upstream.sha);
  const headSha = createPublishedHead(fixture.repo, bundle, candidate, materialization, headTree);
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () => new Response(JSON.stringify(pullResponse({ bundle, pullNumber: 10, headSha })), { status: 200 });
  t.after(() => { globalThis.fetch = originalFetch; });
  const environment = {
    ...fixture.environment, GITHUB_TOKEN: 'read', GITHUB_API_URL: 'https://api.github.test',
    AERIS_CONFLICT_PULL_NUMBER: '10', AERIS_CONFLICT_HEAD_SHA: headSha, AERIS_CONFLICT_HEAD_TREE_SHA: headTree,
  };
  const input = await collectReviewInput({ bundle, candidate, environment });
  await assert.rejects(() => reviewConflict({
    input, bundle, candidate, environment,
    client: fakeClient({
      schema_version: 1, verdict: 'fail', summary: 'Behavior is uncertain.',
      findings: [{ severity: 'high', path: 'shared.txt', details: 'The semantic ordering is not justified.' }],
    }, { alias: 'conflict-reviewer', id: 'reviewer-model' }),
  }), /did not approve/);

  assert.throws(() => validateReviewReceipt({
    schema_version: 1,
    artifact_type: 'sync_conflict_review',
    profile: 'aeris-sync-conflict-v1',
  }, input, bundle, candidate), /fields are invalid/);
  assert.ok(canonicalJson(bundle).length > 0);
});
