import { execFileSync } from 'node:child_process';
import { createHash, createPublicKey, createSign, randomBytes } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

import { loadContracts, resolveModelCandidates } from './config.mjs';
import { GitHubClient } from './github-client.mjs';
import { OpenAICompatibleClient } from './openai-client.mjs';
import { routeWriterInvocation } from './router.mjs';
import { WriterGitHubClient } from './writer-github-client.mjs';
import {
  evaluateWriterLifecycle,
  evaluateWriterRetryLineage,
  verifyWriterReceiptMarker,
  WRITER_LIFECYCLE_EPOCH,
  writerPullLifecycleAttestation,
  writerCommitMessage,
  writerPullMetadataDigest,
  writerReceiptSigningPayload,
} from './writer-lifecycle.mjs';
import { calculateWriterCandidateSha, validateWriterCandidateArtifact } from './writer-phase-contract.mjs';
import { publishWriterCandidate } from './writer-publisher.mjs';

const SHA = /^[0-9a-f]{40}$/;
const MAX_PATCH_BYTES = 65_536;

function git(repoRoot, args, options = {}) {
  return execFileSync('git', args, {
    cwd: repoRoot,
    encoding: 'utf8',
    windowsHide: true,
    timeout: options.timeout ?? 30_000,
    maxBuffer: options.maxBuffer ?? 2 * 1024 * 1024,
    env: options.env ?? process.env,
  }).trim();
}

function run(repoRoot, command, args, timeout) {
  execFileSync(command, args, {
    cwd: repoRoot,
    windowsHide: true,
    timeout,
    maxBuffer: 2 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
}

function trustedSha(repoRoot) {
  const value = git(repoRoot, ['rev-parse', 'HEAD']);
  if (!SHA.test(value)) throw new Error('trusted checkout SHA is invalid');
  return value;
}

function canonicalLabels(issue) {
  return (issue.labels ?? []).map((item) => typeof item === 'string' ? item : item.name)
    .sort((left, right) => left.localeCompare(right, 'en'));
}

function sha256(value) {
  return createHash('sha256').update(value, 'utf8').digest('hex');
}

function activation(state, phase, payload = null, reason = null) {
  return { schema_version: 1, artifact_type: 'writer_activation', phase, state, reason, payload };
}

function normalizedWriterPull(pull) {
  if (!pull || typeof pull !== 'object' || Array.isArray(pull)) return pull;
  const merged = typeof pull.merged === 'boolean'
    ? pull.merged
    : pull.state === 'open' && pull.merged_at === null ? false
      : pull.state === 'closed' && typeof pull.merged_at === 'string';
  return { ...pull, merged };
}

export function deriveNextWriterFixCycle({
  configuredRepository,
  issue,
  comment,
  branch,
  mainSha,
  sourceSha,
  pulls,
  writerApp,
  receiptPublicKey,
  compare = null,
} = {}) {
  const sameRepository = (repo) => repo?.id === configuredRepository.repository_id &&
    typeof repo.full_name === 'string' &&
    repo.full_name.toLowerCase() === configuredRepository.repository_name.toLowerCase();
  if (!Array.isArray(pulls) || pulls.some((pull) => !sameRepository(pull?.base?.repo) || !sameRepository(pull?.head?.repo))) {
    return { allowed: false, reason: 'managed_pr_repository_invalid' };
  }
  const lifecycle = evaluateWriterLifecycle({
    command: '/agent retry-write',
    issueNumber: issue.number,
    repositoryId: configuredRepository.repository_id,
    writerApp,
    expectedHeadSha: sourceSha,
    baseSha: mainSha,
    sourceSha,
    branch: { ref: branch, exists: true, headSha: sourceSha },
    pullRequests: pulls,
  });
  if (lifecycle.action !== 'update' || pulls.length !== 1) return { allowed: false, reason: lifecycle.reason };
  const receipt = verifyWriterReceiptMarker(pulls[0].body, receiptPublicKey);
  if (!receipt || receipt.issue_number !== issue.number ||
    receipt.lifecycle_epoch !== WRITER_LIFECYCLE_EPOCH || receipt.fix_cycle !== 0) {
    return { allowed: false, reason: 'managed_pr_receipt_invalid' };
  }
  const lineage = evaluateWriterRetryLineage({
    receipt,
    sourceSha,
    compare,
    issueNumber: issue.number,
    commentId: comment.id,
    maximumFixCycles: configuredRepository.limits.maximum_fix_cycles,
  });
  if (!lineage.allowed) return lineage;
  let pullMetadataSha;
  try {
    pullMetadataSha = writerPullMetadataDigest(pulls[0]);
  } catch {
    return { allowed: false, reason: 'managed_pr_detail_invalid' };
  }
  return { allowed: true, fixCycle: lineage.fixCycle, sourceSha, expectedRemoteHead: sourceSha, pullMetadataSha };
}

async function deriveRetryFixCycle({ client, configuredRepository, environment, issue, comment, branch, mainSha }) {
  const appId = Number(environment.AERIS_WRITER_APP_ID);
  const appSlug = environment.AERIS_WRITER_APP_SLUG;
  if (!Number.isSafeInteger(appId) || appId <= 0 || typeof appSlug !== 'string') {
    return { allowed: false, reason: 'writer_app_identity_unconfigured' };
  }
  let ref;
  try {
    ref = await client.request('GET', `/repos/${environment.GITHUB_REPOSITORY}/git/ref/heads/${encodeURIComponent(branch)}`);
  } catch {
    return { allowed: false, reason: 'retry_requires_managed_draft_pull_request' };
  }
  const sourceSha = ref?.object?.sha;
  if (ref?.ref !== `refs/heads/${branch}` || ref?.object?.type !== 'commit' || !SHA.test(sourceSha ?? '')) {
    return { allowed: false, reason: 'writer_retry_ref_invalid' };
  }
  const owner = environment.GITHUB_REPOSITORY.split('/')[0];
  const listed = await client.list(
    `/repos/${environment.GITHUB_REPOSITORY}/pulls?state=all&base=main&head=${encodeURIComponent(`${owner}:${branch}`)}`,
    3,
  );
  if (listed.truncated) return { allowed: false, reason: 'managed_pr_history_truncated' };
  const pulls = [];
  for (const rawPull of listed.items) {
    let timeline;
    try {
      timeline = await client.listPullTimeline(rawPull?.number);
    } catch {
      return { allowed: false, reason: 'managed_pr_timeline_unavailable' };
    }
    const writerLifecycle = writerPullLifecycleAttestation(timeline?.events, timeline?.truncated);
    if (writerLifecycle === null) return { allowed: false, reason: 'managed_pr_timeline_invalid' };
    let detailedPull;
    try {
      detailedPull = await client.getPull(rawPull?.number);
    } catch {
      return { allowed: false, reason: 'managed_pr_detail_unavailable' };
    }
    if (detailedPull?.number !== rawPull?.number) return { allowed: false, reason: 'managed_pr_detail_invalid' };
    pulls.push({ ...normalizedWriterPull(detailedPull), writer_lifecycle: writerLifecycle });
  }
  let compare = null;
  if (pulls.length === 1) {
    const receipt = verifyWriterReceiptMarker(pulls[0].body, environment.AERIS_WRITER_PUBLIC_KEY);
    if (receipt && receipt.commit_sha !== sourceSha) {
      try {
        compare = await client.request(
          'GET',
          `/repos/${environment.GITHUB_REPOSITORY}/compare/${receipt.commit_sha}...${sourceSha}`,
        );
      } catch {
        return { allowed: false, reason: 'managed_pr_retry_lineage_unavailable' };
      }
    }
  }
  return deriveNextWriterFixCycle({
    configuredRepository,
    issue,
    comment,
    branch,
    mainSha,
    sourceSha,
    pulls,
    writerApp: { id: appId, slug: appSlug },
    receiptPublicKey: environment.AERIS_WRITER_PUBLIC_KEY,
    compare,
  });
}

export async function runWriterPreflight({ event, environment, repoRoot, github = null, clock = () => new Date() }) {
  const policySha = trustedSha(repoRoot);
  if (environment.AERIS_WRITER_CANARY === 'true') {
    return activation('canary', 'preflight', { policy_sha: policySha, run_id: environment.GITHUB_RUN_ID ?? 'local-canary' });
  }
  const contracts = loadContracts(repoRoot);
  const client = github ?? new GitHubClient({ token: environment.GITHUB_TOKEN, repository: environment.GITHUB_REPOSITORY });
  const configuredRepository = contracts.agents.agents.writer;
  if (event.repository?.id !== configuredRepository.repository_id || environment.GITHUB_REPOSITORY !== configuredRepository.repository_name) {
    return activation('terminal', 'preflight', null, 'writer_repository_mismatch');
  }
  const issue = await client.getIssue(event.issue.number);
  const comment = await client.getIssueComment(event.comment.id);
  const main = await client.request('GET', `/repos/${environment.GITHUB_REPOSITORY}/git/ref/heads/main`);
  if (main?.ref !== 'refs/heads/main' || main?.object?.type !== 'commit' || !SHA.test(main?.object?.sha ?? '')) throw new Error('main ref is invalid');
  let fixCycle = 0;
  let sourceSha = main.object.sha;
  let expectedRemoteHead = null;
  let pullMetadataSha = null;
  if (event.comment?.body === '/agent retry-write') {
    const branch = `agent/issue-${event.issue.number}`;
    const retry = await deriveRetryFixCycle({
      client,
      configuredRepository,
      environment,
      issue,
      comment,
      branch,
      mainSha: main.object.sha,
    });
    if (!retry.allowed) return activation('terminal', 'preflight', null, retry.reason);
    fixCycle = retry.fixCycle;
    sourceSha = retry.sourceSha;
    expectedRemoteHead = retry.expectedRemoteHead;
    pullMetadataSha = retry.pullMetadataSha;
  }
  const decision = await routeWriterInvocation({
    eventName: environment.GITHUB_EVENT_NAME,
    event,
    github: client,
    trustedContracts: contracts,
    environment,
    fixCycle,
  });
  if (decision.action !== 'write') return activation('terminal', 'preflight', null, decision.reason);
  const now = clock();
  const intent = {
    repository_id: configuredRepository.repository_id,
    repository_name: configuredRepository.repository_name,
    issue_number: issue.number,
    issue_url: issue.url,
    issue_updated_at: issue.updated_at,
    issue_labels: canonicalLabels(issue),
    input_sha: sha256(JSON.stringify({ id: comment.id, body: comment.body, issue: issue.number, updated_at: issue.updated_at, labels: canonicalLabels(issue) })),
    comment_id: comment.id,
    actor: comment.user.login,
    command: decision.command,
    base_sha: main.object.sha,
    source_sha: sourceSha,
    policy_sha: policySha,
    config_sha: policySha,
    run_id: environment.GITHUB_RUN_ID,
    agent: 'writer',
    branch: decision.branch,
    expected_remote_head: expectedRemoteHead,
    pull_metadata_sha: pullMetadataSha,
    lease_token: randomBytes(32).toString('hex'),
    cancel_epoch: 0,
    lease_expires_at: new Date(now.getTime() + 30 * 60 * 1000).toISOString(),
  };
  return activation('ready', 'preflight', {
    intent,
    fix_cycle: fixCycle,
    request: {
      title: typeof issue.title === 'string' ? issue.title.slice(0, 512) : '',
      body: typeof issue.body === 'string' ? issue.body.slice(0, 24_000) : '',
    },
  });
}

function parseWriterModelOutput(content) {
  let value;
  try { value = JSON.parse(content); } catch { throw new Error('Writer model output is not JSON'); }
  if (!value || typeof value !== 'object' || Array.isArray(value) ||
    Object.keys(value).sort().join(',') !== 'body,patch,title') throw new Error('Writer model output shape is invalid');
  if (typeof value.patch !== 'string' || value.patch.length === 0 || Buffer.byteLength(value.patch) > MAX_PATCH_BYTES) throw new Error('Writer patch is invalid');
  if (typeof value.title !== 'string' || value.title.length === 0 || value.title.length > 256 || /[\r\n]/.test(value.title)) throw new Error('Writer title is invalid');
  if (typeof value.body !== 'string' || Buffer.byteLength(value.body) > 60_000) throw new Error('Writer body is invalid');
  return value;
}

export async function runWriterAnalyze({ artifact, environment, repoRoot, aiClient = null }) {
  if (artifact.state === 'canary') return activation('canary', 'analyze', artifact.payload);
  if (artifact.state !== 'ready') return activation('terminal', 'analyze', null, artifact.reason);
  const contracts = loadContracts(repoRoot);
  const writer = contracts.agents.agents.writer;
  const candidates = resolveModelCandidates('writer', { ...writer, enabled: true }, environment);
  const client = aiClient ?? new OpenAICompatibleClient({
    baseUrl: environment.AERIS_AI_BASE_URL,
    apiKey: environment.AERIS_AI_API_KEY,
    endpoint: contracts.agents.runtime.api.endpoint,
    retryableStatuses: contracts.agents.model_policy.retryable_http_statuses,
    connectTimeoutMs: 120_000,
    timeoutMs: 300_000,
    deadlineAtMs: Date.now() + 10 * 60 * 1000,
    maximumResponseBytes: contracts.agents.runtime.api.maximum_response_bytes,
  });
  const completion = await client.complete({
    candidates,
    maxTokens: 8000,
    messages: [
      { role: 'system', content: 'Return JSON only with exactly title, body, patch. patch must be a git unified diff against the supplied base. Never modify .github, manifests, auth, migrations, security files, symlinks, or submodules.' },
      { role: 'user', content: JSON.stringify({ intent: artifact.payload.intent, issue: artifact.payload.request, instruction: 'Implement this Issue using only allowlisted source, test, frontend/src, or docs paths.' }) },
    ],
  });
  return activation('ready', 'analyze', { ...artifact.payload, generated: parseWriterModelOutput(completion.content) });
}

function changedFiles(repoRoot) {
  const names = git(repoRoot, ['diff', '--cached', '--name-only', '-z'], { maxBuffer: 1024 * 1024 });
  return names.length === 0 ? [] : names.split('\0').filter(Boolean);
}

function validateStagedFiles(repoRoot, paths) {
  const status = git(repoRoot, ['diff', '--cached', '--name-status', '-z']);
  const entries = status.length === 0 ? [] : status.split('\0').filter(Boolean);
  if (entries.length !== paths.length * 2) throw new Error('Writer staged status is ambiguous');
  for (let index = 0; index < entries.length; index += 2) {
    if (!/^[AM]$/.test(entries[index]) || entries[index + 1] !== paths[index / 2]) {
      throw new Error('Writer permits only add or modify changes');
    }
  }
  for (const candidatePath of paths) {
    const record = git(repoRoot, ['ls-files', '-s', '--', candidatePath]);
    if (!record.startsWith('100644 ')) throw new Error('Writer permits only regular non-executable files');
    if (fs.readFileSync(path.join(repoRoot, candidatePath)).includes(0)) {
      throw new Error('Writer candidate files must be text');
    }
  }
}

export function runWriterBuild({ artifact, repoRoot }) {
  if (artifact.state === 'canary') return activation('canary', 'build', artifact.payload);
  if (artifact.state !== 'ready') return activation('terminal', 'build', null, artifact.reason);
  const { intent, fix_cycle: fixCycle, generated } = artifact.payload;
  const trustedContracts = loadContracts(repoRoot);
  if (trustedSha(repoRoot) !== intent.base_sha) return activation('terminal', 'build', null, 'base_main_changed');
  git(repoRoot, ['checkout', '--detach', intent.source_sha], { timeout: 60_000 });
  const patchPath = path.join(repoRoot, '.git', 'aeris-writer-candidate.patch');
  fs.writeFileSync(patchPath, generated.patch, { encoding: 'utf8', mode: 0o600, flag: 'wx' });
  try {
    git(repoRoot, ['apply', '--index', '--whitespace=error-all', patchPath], { timeout: 60_000 });
    git(repoRoot, ['diff', '--cached', '--check']);
    const paths = changedFiles(repoRoot);
    validateStagedFiles(repoRoot, paths);
    const fileSizes = paths.map((candidatePath) => ({ path: candidatePath, bytes: fs.statSync(path.join(repoRoot, candidatePath)).size }));
    const patch = `${git(repoRoot, ['diff', '--cached', '--binary'], { maxBuffer: MAX_PATCH_BYTES + 4096 })}\n`;
    const candidate = {
      schema_version: 2,
      artifact_type: 'candidate',
      state: 'ready',
      intent,
      patch_sha: sha256(patch),
      changed_paths: paths,
      file_sizes: fileSizes,
      file_count: paths.length,
      patch_bytes: Buffer.byteLength(patch),
      total_file_bytes: fileSizes.reduce((sum, item) => sum + item.bytes, 0),
      limits: trustedContracts.agents.agents.writer.limits,
      fix_cycle: fixCycle,
      tests: { state: 'passed', plan_ids: [], summary: 'trusted build validation passed' },
    };
    const families = paths.map((candidatePath) => candidatePath.startsWith('frontend/src/') ? 'frontend' : candidatePath.startsWith('docs/') ? 'docs' : 'rust');
    candidate.tests.plan_ids = ['diff-check-v1'];
    if (families.includes('rust')) candidate.tests.plan_ids.push('rust-changed-packages-v1');
    if (families.includes('frontend')) candidate.tests.plan_ids.push('frontend-v1');
    if (families.includes('rust')) run(repoRoot, 'cargo', ['test', '--workspace'], 10 * 60 * 1000);
    if (families.includes('frontend')) run(path.join(repoRoot, 'frontend'), 'npm', ['test', '--', '--run'], 10 * 60 * 1000);
    candidate.candidate_sha = calculateWriterCandidateSha(candidate);
    validateWriterCandidateArtifact(candidate);
    const env = {
      ...process.env,
      GIT_AUTHOR_NAME: 'Aeris Writer', GIT_AUTHOR_EMAIL: 'aeris-writer@users.noreply.github.com',
      GIT_COMMITTER_NAME: 'Aeris Writer', GIT_COMMITTER_EMAIL: 'aeris-writer@users.noreply.github.com',
      GIT_AUTHOR_DATE: intent.issue_updated_at, GIT_COMMITTER_DATE: intent.issue_updated_at,
    };
    git(repoRoot, ['commit', '--no-gpg-sign', '-m', writerCommitMessage({
      issueNumber: intent.issue_number,
      fixCycle,
      commentId: intent.comment_id,
      candidateSha: candidate.candidate_sha,
      pullMetadataSha: intent.pull_metadata_sha,
    })], { timeout: 60_000, env });
    const commitSha = trustedSha(repoRoot);
    return activation('ready', 'build', { candidate, commit_sha: commitSha, patch, metadata: { title: generated.title, body: generated.body } });
  } catch (error) {
    return activation('terminal', 'build', null, 'candidate_build_failed');
  } finally {
    try { fs.unlinkSync(patchPath); } catch {}
  }
}

function base64url(value) {
  return Buffer.from(JSON.stringify(value)).toString('base64url');
}

export function createWriterAppJwt(appId, privateKey, now = new Date()) {
  if (!Number.isSafeInteger(appId) || appId <= 0 || typeof privateKey !== 'string' || privateKey.length === 0) throw new Error('Writer App JWT configuration is invalid');
  const seconds = Math.floor(now.getTime() / 1000);
  const unsigned = `${base64url({ alg: 'RS256', typ: 'JWT' })}.${base64url({ iat: seconds - 60, exp: seconds + 540, iss: String(appId) })}`;
  const signer = createSign('RSA-SHA256');
  signer.update(unsigned);
  signer.end();
  return `${unsigned}.${signer.sign(privateKey).toString('base64url')}`;
}

export function createWriterReceiptSigner(privateKey, configuredPublicKey) {
  if (typeof privateKey !== 'string' || privateKey.length === 0 ||
    typeof configuredPublicKey !== 'string' || configuredPublicKey.length === 0) {
    throw new Error('Writer receipt signing configuration is invalid');
  }
  let derived;
  let configured;
  try {
    derived = createPublicKey(privateKey).export({ type: 'spki', format: 'pem' }).trim();
    configured = createPublicKey(configuredPublicKey).export({ type: 'spki', format: 'pem' }).trim();
  } catch {
    throw new Error('Writer receipt signing key is invalid');
  }
  if (derived !== configured) throw new Error('Writer receipt public key does not match the App private key');
  return (fields) => {
    const signer = createSign('RSA-SHA256');
    signer.update(writerReceiptSigningPayload(fields));
    signer.end();
    return signer.sign(privateKey).toString('base64url');
  };
}

export async function runWriterPublish({
  artifact,
  environment,
  repoRoot,
  clock = () => new Date(),
  client = null,
  fetchImpl = globalThis.fetch,
  repositoryPath = repoRoot,
  writerExecFileImpl = undefined,
}) {
  if (artifact.state === 'canary') return activation('canary', 'publish', { ...artifact.payload, mutations: 0 }, 'disabled_canary');
  if (artifact.state !== 'ready') return activation('terminal', 'publish', null, artifact.reason);
  const { candidate, commit_sha: expectedCommitSha, patch, metadata } = artifact.payload;
  const trustedContracts = loadContracts(repoRoot);
  const currentMainSha = trustedSha(repoRoot);
  if (currentMainSha !== candidate.intent.base_sha) return activation('terminal', 'publish', null, 'base_main_changed');
  git(repoRoot, ['checkout', '--detach', candidate.intent.source_sha], { timeout: 60_000 });
  const patchPath = path.join(repoRoot, '.git', 'aeris-writer-publish.patch');
  fs.writeFileSync(patchPath, patch, { encoding: 'utf8', mode: 0o600, flag: 'wx' });
  try {
    git(repoRoot, ['apply', '--index', '--whitespace=error-all', patchPath], { timeout: 60_000 });
    const env = {
      ...process.env,
      GIT_AUTHOR_NAME: 'Aeris Writer', GIT_AUTHOR_EMAIL: 'aeris-writer@users.noreply.github.com',
      GIT_COMMITTER_NAME: 'Aeris Writer', GIT_COMMITTER_EMAIL: 'aeris-writer@users.noreply.github.com',
      GIT_AUTHOR_DATE: candidate.intent.issue_updated_at, GIT_COMMITTER_DATE: candidate.intent.issue_updated_at,
    };
    const stagedPatch = `${git(repoRoot, ['diff', '--cached', '--binary'], { maxBuffer: MAX_PATCH_BYTES + 4096 })}\n`;
    if (sha256(stagedPatch) !== candidate.patch_sha) return activation('terminal', 'publish', null, 'candidate_patch_changed');
    git(repoRoot, ['commit', '--no-gpg-sign', '-m', writerCommitMessage({
      issueNumber: candidate.intent.issue_number,
      fixCycle: candidate.fix_cycle,
      commentId: candidate.intent.comment_id,
      candidateSha: candidate.candidate_sha,
      pullMetadataSha: candidate.intent.pull_metadata_sha,
    })], { timeout: 60_000, env });
    if (trustedSha(repoRoot) !== expectedCommitSha) return activation('terminal', 'publish', null, 'candidate_commit_mismatch');
    const appId = Number(environment.AERIS_WRITER_APP_ID);
    const writerApp = { id: appId, slug: environment.AERIS_WRITER_APP_SLUG };
    const receiptSigner = createWriterReceiptSigner(
      environment.AERIS_WRITER_PRIVATE_KEY,
      environment.AERIS_WRITER_PUBLIC_KEY,
    );
    const github = client ?? new WriterGitHubClient({
      appJwt: createWriterAppJwt(appId, environment.AERIS_WRITER_PRIVATE_KEY, clock()),
      repository: environment.GITHUB_REPOSITORY,
      repositoryId: candidate.intent.repository_id,
      writerApp,
      fetchImpl,
      execFileImpl: writerExecFileImpl,
      totalTimeoutMs: 30_000,
      headersTimeoutMs: 10_000,
      bodyTimeoutMs: 15_000,
    });
    await github.verifyInstallationIdentity();
    const receipt = await publishWriterCandidate({
      candidate,
      commitSha: expectedCommitSha,
      metadata,
      github,
      trustedContracts,
      environment,
      currentPolicySha: currentMainSha,
      currentConfigSha: currentMainSha,
      repositoryId: candidate.intent.repository_id,
      writerApp,
      receiptSigner,
      receiptPublicKey: environment.AERIS_WRITER_PUBLIC_KEY,
      fixCycle: candidate.fix_cycle,
      verifyCandidateCommit: async () => true,
      clock,
      repositoryPath,
    });
    return activation('complete', 'publish', { receipt });
  } finally {
    try { fs.unlinkSync(patchPath); } catch {}
  }
}
