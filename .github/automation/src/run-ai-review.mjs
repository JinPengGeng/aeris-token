import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { loadContracts } from './config.mjs';
import { AiReviewGitHubClient } from './ai-review-github-client.mjs';
import { collectExactPullDiff } from './ai-review-exact-diff.mjs';
import { AiReviewModelClient } from './ai-review-model-client.mjs';
import { analyzeAiReview, finalizeAiReview, prepareAiReview } from './ai-review.mjs';
import { canonicalJson, validateAnalysisArtifact, validateCandidateArtifact } from './ai-review-phase-contract.mjs';

const defaultRepoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..');

function requireCondition(condition, message) { if (!condition) throw new Error(message); }
function enabled(value) { return typeof value === 'string' && ['1', 'true'].includes(value.trim().toLowerCase()); }
function positive(value, name) {
  const number = typeof value === 'string' && /^[1-9]\d*$/.test(value) ? Number(value) : value;
  requireCondition(Number.isSafeInteger(number) && number > 0, `${name} is invalid`);
  return number;
}
function read(file) {
  const stat = fs.statSync(file);
  requireCondition(stat.isFile() && stat.size > 0 && stat.size <= 16 * 1024 * 1024, 'AI review artifact file is invalid');
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}
function write(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const temporary = `${file}.${process.pid}.${Date.now()}.tmp`;
  try {
    fs.writeFileSync(temporary, `${canonicalJson(value)}\n`, { encoding: 'utf8', flag: 'wx', mode: 0o600 });
    fs.renameSync(temporary, file);
  } finally { if (fs.existsSync(temporary)) fs.rmSync(temporary, { force: true }); }
}
function policySha(repoRoot) {
  const value = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: repoRoot, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  requireCondition(/^[0-9a-f]{40}$/.test(value), 'trusted checkout SHA is invalid');
  return value;
}
function modelCandidates(environment) {
  const candidates = (role) => {
    const values = [{ alias: role, id: environment[`AERIS_AI_MODEL_${role.toUpperCase()}`] }];
    if (environment.AERIS_AI_MODEL_FALLBACK) values.push({ alias: 'fallback', id: environment.AERIS_AI_MODEL_FALLBACK });
    const normalized = values.filter((entry) => typeof entry.id === 'string' && entry.id.trim().length > 0).map((entry) => ({ ...entry, id: entry.id.trim() }));
    return normalized.filter((entry, index) => normalized.findIndex((candidate) => candidate.id === entry.id) === index);
  };
  return { reviewer: candidates('reviewer'), security: candidates('security') };
}
function assertEnabled(contracts, environment) {
  requireCondition(enabled(environment.AERIS_AGENTS_ENABLED), 'repository Agent kill switch is disabled');
  requireCondition(enabled(environment.AERIS_AI_ATTESTATION_ENABLED), 'AI attestation kill switch is disabled');
  requireCondition(contracts.agents.agents.reviewer.enabled === true, 'reviewer Agent is disabled');
  requireCondition(contracts.agents.agents.security.enabled === true, 'security Agent is disabled');
}
function githubClient(environment) {
  return new AiReviewGitHubClient({
    token: environment.GITHUB_TOKEN,
    repository: environment.GITHUB_REPOSITORY,
    repositoryId: positive(environment.AERIS_REPOSITORY_ID, 'repository ID'),
  });
}
function common(environment, repoRoot, contracts) {
  return {
    client: githubClient(environment),
    repository: environment.GITHUB_REPOSITORY,
    repositoryId: positive(environment.AERIS_REPOSITORY_ID, 'repository ID'),
    pullNumber: positive(environment.AERIS_PULL_REQUEST_NUMBER, 'pull number'),
    policySha: policySha(repoRoot),
    modelCandidates: modelCandidates(environment),
    maximumOutputTokens: contracts.agents.runtime.limits.maximum_output_tokens,
    exactDiff: null,
  };
}

async function preparedValues(environment, repoRoot, contracts) {
  const values = common(environment, repoRoot, contracts);
  const pull = await values.client.getPull(values.pullNumber);
  values.exactDiff = collectExactPullDiff({ repoRoot, repository: values.repository, pullNumber: values.pullNumber, baseSha: pull.base.sha, headSha: pull.head.sha });
  return values;
}
function jobFailureCode(environment) {
  if (environment.AERIS_PREPARE_JOB_RESULT && environment.AERIS_PREPARE_JOB_RESULT !== 'success') return 'prepare_job_failed';
  if (environment.AERIS_ANALYZE_JOB_RESULT === 'cancelled') return 'cancelled';
  if (environment.AERIS_ANALYZE_JOB_RESULT === 'skipped') return 'analysis_job_skipped';
  if (environment.AERIS_ANALYZE_JOB_RESULT && environment.AERIS_ANALYZE_JOB_RESULT !== 'success') return 'analysis_job_failed';
  return null;
}

export async function runAiReviewCli({ mode = process.argv[2], environment = process.env, repoRoot = defaultRepoRoot } = {}) {
  requireCondition(['prepare', 'analyze', 'finalize'].includes(mode), 'AI review mode is invalid');
  const contracts = loadContracts(repoRoot);
  assertEnabled(contracts, environment);
  if (mode === 'prepare') {
    const artifact = await prepareAiReview(await preparedValues(environment, repoRoot, contracts));
    write(environment.AERIS_OUTPUT_PATH, artifact);
    return artifact;
  }
  if (mode === 'analyze') {
    const candidate = validateCandidateArtifact(read(environment.AERIS_CANDIDATE_PATH));
    const modelClient = new AiReviewModelClient({
      baseUrl: environment.AERIS_AI_BASE_URL,
      apiKey: environment.AERIS_AI_API_KEY,
      timeoutMs: contracts.agents.runtime.reviewer_limits.request_timeout_seconds * 1000,
      maximumResponseBytes: contracts.agents.runtime.api.maximum_response_bytes,
      retryableStatuses: contracts.agents.model_policy.retryable_http_statuses,
    });
    const artifact = await analyzeAiReview({
      modelClient,
      candidate,
      apiKey: environment.AERIS_AI_API_KEY,
      maximumOutputTokens: contracts.agents.runtime.limits.maximum_output_tokens,
    });
    write(environment.AERIS_OUTPUT_PATH, artifact);
    return artifact;
  }

  const values = await preparedValues(environment, repoRoot, contracts);
  let candidate;
  let forcedFailureCode = jobFailureCode(environment);
  try { candidate = validateCandidateArtifact(read(environment.AERIS_CANDIDATE_PATH)); } catch {
    candidate = await prepareAiReview(values);
    forcedFailureCode ??= 'candidate_artifact_missing';
  }
  let analysis = null;
  if (forcedFailureCode === null) {
    try { analysis = validateAnalysisArtifact(read(environment.AERIS_ANALYSIS_PATH)); } catch { forcedFailureCode = 'analysis_artifact_missing'; }
  }
  const artifact = await finalizeAiReview({
    client: values.client,
    candidate,
    analysis,
    freshCandidate: () => prepareAiReview(values),
    forcedFailureCode,
    runGroupId: environment.GITHUB_RUN_ID ?? 'unknown',
    runId: `${environment.GITHUB_RUN_ID ?? 'unknown'}.${environment.GITHUB_RUN_ATTEMPT ?? '1'}`,
    detailsUrl: environment.AERIS_DETAILS_URL,
  });
  write(environment.AERIS_OUTPUT_PATH, artifact);
  requireCondition(artifact.state === 'success', 'one or more AI review attestations failed');
  return artifact;
}

if (path.resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  try { await runAiReviewCli(); } catch (error) { console.error(`aeris AI review failed: ${error.message}`); process.exitCode = 1; }
}
