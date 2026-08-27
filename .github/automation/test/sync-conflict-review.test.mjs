import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync, spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  SYNC_CONFLICT_PROFILE,
  SYNC_CONFLICT_SCHEMA_VERSION,
  artifactSha,
  canonicalJson,
  conflictGeneration,
  reviewGeneration,
  validateConflictBundle,
  validateConflictCandidate,
  validateFinalAttestation,
  validateModelCandidates,
  validateReviewInput,
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

const TRUSTED_SYNC_EXECUTOR = Object.freeze({
  id: 'openai-chat-v1',
  protocol: 'openai-chat-completions-v1',
});
const REVIEW_CLI = fileURLToPath(new URL('../src/sync-conflict-review.mjs', import.meta.url));

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

function fakeClient(content, model, executor) {
  return { async complete() { return { content: JSON.stringify(content), model, executor, usage: null, durationMs: 1 }; } };
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
    'Sync-Upstream-Conflict-Profile: aeris-sync-conflict-v2',
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
    }, { alias: 'conflict-resolver', id: 'resolver-model' }, TRUSTED_SYNC_EXECUTOR),
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
      { alias: 'conflict-reviewer', id: 'reviewer-model' }, TRUSTED_SYNC_EXECUTOR),
  });
  assert.equal(receipt.output.verdict, 'pass');

  const attestation = await finalizeConflictReview({ bundle, candidate, input, receipt, environment: publishedEnvironment });
  assert.equal(attestation.decision, 'approved');
  assert.equal(attestation.head_sha, headSha);
  assert.deepEqual(attestation.resolver_executor, { id: 'openai-chat-v1', protocol: 'openai-chat-completions-v1' });
  assert.deepEqual(attestation.reviewer_executor, { id: 'openai-chat-v1', protocol: 'openai-chat-completions-v1' });
  assert.notEqual(attestation.resolver_model.id, attestation.reviewer_model.id);
  assert.equal(conflictGeneration(bundle).schema_version, SYNC_CONFLICT_SCHEMA_VERSION);
  assert.equal(conflictGeneration(bundle).profile, SYNC_CONFLICT_PROFILE);
  assert.equal(reviewGeneration(input).schema_version, SYNC_CONFLICT_SCHEMA_VERSION);
  assert.equal(reviewGeneration(input).profile, SYNC_CONFLICT_PROFILE);
  const artifactStages = [
    ['bundle', bundle, (value) => validateConflictBundle(value)],
    ['candidate', candidate, (value) => validateConflictCandidate(value, bundle)],
    ['review input', input, (value) => validateReviewInput(value, bundle, candidate)],
    ['review receipt', receipt, (value) => validateReviewReceipt(value, input, bundle, candidate)],
    ['final attestation', attestation, (value) => validateFinalAttestation(value)],
  ];
  for (const [name, artifact, validate] of artifactStages) {
    assert.equal(artifact.schema_version, SYNC_CONFLICT_SCHEMA_VERSION, name);
    assert.equal(artifact.profile, SYNC_CONFLICT_PROFILE, name);
    const legacySchema = structuredClone(artifact);
    legacySchema.schema_version = 1;
    assert.throws(() => validate(legacySchema), /version|identity/, `${name} accepted legacy schema v1`);
    const legacyProfile = structuredClone(artifact);
    legacyProfile.profile = 'aeris-sync-conflict-v1';
    assert.throws(() => validate(legacyProfile), /version|identity/, `${name} accepted legacy profile v1`);
  }
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
    bundleSha: artifactSha(bundle),
    candidateSha: artifactSha(candidate),
    reviewInputSha: artifactSha(input),
    reviewReceiptSha: artifactSha(receipt),
    runId: fixture.environment.GITHUB_RUN_ID,
    runAttempt: fixture.environment.GITHUB_RUN_ATTEMPT,
  };
  const artifactChain = { bundle, candidate, input, receipt };
  assert.equal(verifyAttestationBinding(attestation, mergeBinding, artifactChain).decision, 'approved');
  assert.throws(
    () => verifyAttestationBinding(attestation, { ...mergeBinding, runAttempt: '3' }, artifactChain),
    /exact workflow run and attempt/,
  );

  const artifactDirectory = path.join(fixture.repo, 'verify-artifacts');
  const artifactPaths = Object.fromEntries(Object.entries({
    attestation,
    bundle,
    candidate,
    input,
    receipt,
  }).map(([name, value]) => {
    const filename = path.join(artifactDirectory, `${name}.json`);
    write(fixture.repo, path.relative(fixture.repo, filename), `${canonicalJson(value)}\n`);
    return [name, filename];
  }));
  const verifyEnvironment = {
    ...process.env,
    AERIS_ARTIFACT_ROOT: fixture.repo,
    GITHUB_REPOSITORY: mergeBinding.repository,
    AERIS_CONFLICT_PULL_NUMBER: String(mergeBinding.pullNumber),
    AERIS_CONFLICT_HEAD_SHA: mergeBinding.headSha,
    AERIS_CONFLICT_BASE_SHA: mergeBinding.baseSha,
    AERIS_CONFLICT_UPSTREAM_REPOSITORY: mergeBinding.upstreamRepository,
    AERIS_CONFLICT_UPSTREAM_SHA: mergeBinding.upstreamSha,
    AERIS_CONFLICT_ATTESTATION_PATH: artifactPaths.attestation,
    AERIS_CONFLICT_ATTESTATION_SHA: mergeBinding.attestationSha,
    AERIS_CONFLICT_BUNDLE_PATH: artifactPaths.bundle,
    AERIS_CONFLICT_CANDIDATE_PATH: artifactPaths.candidate,
    AERIS_CONFLICT_REVIEW_INPUT_PATH: artifactPaths.input,
    AERIS_CONFLICT_REVIEW_RECEIPT_PATH: artifactPaths.receipt,
    AERIS_CONFLICT_BUNDLE_SHA: mergeBinding.bundleSha,
    AERIS_CONFLICT_CANDIDATE_SHA: mergeBinding.candidateSha,
    AERIS_CONFLICT_REVIEW_INPUT_SHA: mergeBinding.reviewInputSha,
    AERIS_CONFLICT_REVIEW_RECEIPT_SHA: mergeBinding.reviewReceiptSha,
    AERIS_CONFLICT_RUN_ID: String(mergeBinding.runId),
    AERIS_CONFLICT_RUN_ATTEMPT: String(mergeBinding.runAttempt),
  };
  const cliResult = spawnSync(process.execPath, [REVIEW_CLI, 'verify-attestation'], {
    cwd: fixture.repo,
    env: verifyEnvironment,
    encoding: 'utf8',
  });
  assert.equal(cliResult.status, 0, cliResult.stderr);
  const missingArtifactResult = spawnSync(process.execPath, [REVIEW_CLI, 'verify-attestation'], {
    cwd: fixture.repo,
    env: { ...verifyEnvironment, AERIS_CONFLICT_REVIEW_RECEIPT_PATH: '' },
    encoding: 'utf8',
  });
  assert.notEqual(missingArtifactResult.status, 0, 'verify-attestation CLI accepted a missing receipt path');

  for (const [name, mutate] of [
    ['resolver executor', (value) => { value.resolver_executor = { id: 'openai-responses-v1', protocol: 'openai-responses-v1' }; }],
    ['reviewer executor', (value) => { value.reviewer_executor = { id: 'openai-responses-v1', protocol: 'openai-responses-v1' }; }],
    ['resolver model', (value) => { value.resolver_model = { alias: 'other-resolver', id: 'other-resolver-model' }; }],
    ['reviewer model', (value) => { value.reviewer_model = { alias: 'other-reviewer', id: 'other-reviewer-model' }; }],
    ['bundle hash', (value) => { value.bundle_sha = '0'.repeat(64); }],
  ]) {
    const tampered = structuredClone(attestation);
    mutate(tampered);
    assert.doesNotThrow(() => validateFinalAttestation(tampered), `${name} fixture must remain schema-valid`);
    assert.throws(
      () => verifyAttestationBinding(
        tampered,
        { ...mergeBinding, attestationSha: artifactSha(tampered) },
        artifactChain,
      ),
      /does not exactly match the verified artifact chain/,
      `${name} tampering must fail even with a recomputed attestation hash`,
    );
  }

  assert.throws(
    () => verifyAttestationBinding(attestation, { ...mergeBinding, candidateSha: '0'.repeat(64) }, artifactChain),
    /candidate does not match the trusted cross-job artifact hash/,
    'a self-consistent local artifact chain must remain anchored to the trusted cross-job hashes',
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
  const resolverOutput = {
    schema_version: 1, verdict: 'resolved', summary: 'Resolved.',
    resolutions: [{ path: 'shared.txt', content: 'resolved\n' }],
  };
  for (const executor of [
    undefined,
    { id: 'openai-responses-v1', protocol: 'openai-responses-v1' },
    { ...TRUSTED_SYNC_EXECUTOR, untrusted: true },
  ]) {
    await assert.rejects(() => resolveConflict({
      bundle,
      environment: fixture.environment,
      client: fakeClient(resolverOutput, { alias: 'conflict-resolver', id: 'resolver-model' }, executor),
    }), /executor identity/);
  }
  const candidate = await resolveConflict({
    bundle,
    environment: fixture.environment,
    client: fakeClient(resolverOutput, { alias: 'conflict-resolver', id: 'resolver-model' }, TRUSTED_SYNC_EXECUTOR),
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
  const reviewerOutput = {
    schema_version: 1, verdict: 'pass', summary: 'The exact result is approved.', findings: [],
  };
  for (const executor of [
    undefined,
    { id: 'openai-responses-v1', protocol: 'openai-responses-v1' },
    { ...TRUSTED_SYNC_EXECUTOR, untrusted: true },
  ]) {
    await assert.rejects(() => reviewConflict({
      input, bundle, candidate, environment,
      client: fakeClient(reviewerOutput, { alias: 'conflict-reviewer', id: 'reviewer-model' }, executor),
    }), /executor identity/);
  }
  await assert.rejects(() => reviewConflict({
    input, bundle, candidate, environment,
    client: fakeClient({
      schema_version: 1, verdict: 'fail', summary: 'Behavior is uncertain.',
      findings: [{ severity: 'high', path: 'shared.txt', details: 'The semantic ordering is not justified.' }],
    }, { alias: 'conflict-reviewer', id: 'reviewer-model' }, TRUSTED_SYNC_EXECUTOR),
  }), /did not approve/);

  assert.throws(() => validateReviewReceipt({
    schema_version: SYNC_CONFLICT_SCHEMA_VERSION,
    artifact_type: 'sync_conflict_review',
    profile: SYNC_CONFLICT_PROFILE,
  }, input, bundle, candidate), /fields are invalid/);
  assert.ok(canonicalJson(bundle).length > 0);
});
