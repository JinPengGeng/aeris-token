import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { loadContracts } from './config.mjs';
import { evaluatePolicyGate, policyCheckConclusion } from './policy-gate.mjs';
import { PolicyGitHubClient } from './policy-github-client.mjs';
import {
  POLICY_ARTIFACT_SCHEMA_VERSION,
  validatePolicyEvaluationArtifact,
  validatePolicyReceiptArtifact,
  writePolicyArtifactAtomic,
} from './policy-phase-contract.mjs';

const SHA = /^[0-9a-f]{40}$/;
const defaultSourceDirectory = path.dirname(fileURLToPath(import.meta.url));
const defaultRepoRoot = path.resolve(defaultSourceDirectory, '..', '..', '..');

function fail(message) {
  throw new Error(message);
}

function requireCondition(condition, message) {
  if (!condition) fail(message);
}

function positiveInteger(value, name) {
  const number = typeof value === 'string' && /^[1-9]\d*$/.test(value) ? Number(value) : value;
  requireCondition(Number.isSafeInteger(number) && number > 0, `${name} must be a positive safe integer`);
  return number;
}

function enabled(value) {
  return typeof value === 'string' && ['1', 'true'].includes(value.trim().toLowerCase());
}

function policyShaAt(repoRoot) {
  const sha = execFileSync('git', ['rev-parse', 'HEAD'], {
    cwd: repoRoot,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  }).trim();
  requireCondition(SHA.test(sha), 'trusted policy checkout is not a commit SHA');
  return sha;
}

function assertEnabled(contracts, environment) {
  requireCondition(enabled(environment.AERIS_AGENTS_ENABLED), 'repository Agent kill switch is disabled');
  requireCondition(enabled(environment.AERIS_POLICY_ENABLED), 'Policy Agent kill switch is disabled');
  requireCondition(contracts?.agents?.agents?.policy?.enabled === true, 'Policy Agent registry entry is disabled');
  requireCondition(contracts?.policy?.policy_gate?.enabled === true, 'Policy gate policy is disabled');
  requireCondition(['shadow', 'human'].includes(contracts.policy.policy_gate.mode), 'Phase 4 supports only shadow or human mode');
  requireCondition(contracts.policy.policy_gate.allowlist_paths?.length === 0, 'Phase 4 automatic merge allowlist must remain empty');
}

function normalizedExpectedHead(value) {
  if (value === undefined || value === null || value === '') return null;
  requireCondition(SHA.test(value), 'expected PR head SHA is invalid');
  return value;
}

function policyExternalId({ repositoryId, pullNumber, headSha, policySha }) {
  return ['aeris-policy', 'v1', repositoryId, pullNumber, headSha, policySha].join(':');
}

function snapshotSha({ repositoryState, mainSha, pull, files, checkRuns, comparison, reviewThreads, policy }) {
  const requiredContexts = new Set(policy.policy_gate.required_checks);
  const snapshot = {
    repository: {
      id: repositoryState.id,
      full_name: repositoryState.full_name,
      default_branch: repositoryState.default_branch,
      main_sha: mainSha,
    },
    policy: {
      mode: policy.policy_gate.mode,
      check_name: policy.policy_gate.check_name,
    },
    pull: {
      number: pull.number,
      state: pull.state,
      draft: pull.draft,
      mergeable: pull.mergeable,
      labels: pull.labels.map((label) => (typeof label === 'string' ? label : label?.name)).sort(),
      head: pull.head,
      base: pull.base,
    },
    files: [...files.files]
      .map((file) => ({
        filename: file.filename,
        status: file.status,
        previous_filename: file.previous_filename,
      }))
      .sort((left, right) => JSON.stringify(left).localeCompare(JSON.stringify(right))),
    files_truncated: files.truncated,
    required_check_runs: checkRuns
      .filter((check) => requiredContexts.has(check?.name) && check?.head_sha === pull.head.sha)
      .map((check) => ({
        id: check.id,
        name: check.name,
        status: check.status,
        conclusion: check.conclusion,
        head_sha: check.head_sha,
        app: check.app,
      }))
      .sort((left, right) => left.id - right.id),
    comparison,
    review_threads: reviewThreads,
  };
  return createHash('sha256').update(JSON.stringify(snapshot), 'utf8').digest('hex');
}

async function collectPolicyGeneration({
  client,
  contracts,
  repository,
  repositoryId,
  pullNumber,
  policySha,
  expectedHeadSha = null,
  expectedPolicySha = null,
}) {
  requireCondition(client && typeof client === 'object', 'Policy GitHub client is required');
  requireCondition(typeof repository === 'string', 'repository is required');
  positiveInteger(repositoryId, 'repository ID');
  positiveInteger(pullNumber, 'pull request number');
  requireCondition(SHA.test(policySha), 'policy SHA is invalid');
  if (expectedPolicySha !== null) {
    requireCondition(SHA.test(expectedPolicySha) && expectedPolicySha === policySha, 'executing policy checkout does not match the expected policy SHA');
  }
  const expectedHead = normalizedExpectedHead(expectedHeadSha);
  const repositoryState = await client.getRepository();
  requireCondition(repositoryState.id === repositoryId && repositoryState.full_name === repository, 'repository identity changed');
  const trustedRef = contracts.policy.trusted_source.ref;
  requireCondition(typeof trustedRef === 'string' && trustedRef.startsWith('refs/heads/'), 'trusted policy ref is invalid');
  const trustedBranch = trustedRef.slice('refs/heads/'.length);
  requireCondition(repositoryState.default_branch === trustedBranch, 'trusted policy branch is not the repository default branch');
  const mainSha = await client.getBranchHead(trustedBranch);
  requireCondition(mainSha === policySha, 'trusted policy checkout is stale');
  const pull = await client.getPull(pullNumber);
  requireCondition(typeof pull.head?.sha === 'string' && SHA.test(pull.head.sha), 'pull head SHA is invalid');
  requireCondition(typeof pull.base?.sha === 'string' && SHA.test(pull.base.sha), 'pull base SHA is invalid');
  requireCondition(expectedHead === null || pull.head.sha === expectedHead, 'pull head SHA changed');
  return { repositoryState, mainSha, pull };
}

export async function collectPolicyEvaluation({
  client,
  contracts,
  repository,
  repositoryId,
  pullNumber,
  policySha,
  expectedHeadSha = null,
  expectedPolicySha = null,
  clock = () => new Date(),
}) {
  const { repositoryState, mainSha, pull } = await collectPolicyGeneration({
    client,
    contracts,
    repository,
    repositoryId,
    pullNumber,
    policySha,
    expectedHeadSha,
    expectedPolicySha,
  });
  const headSha = pull.head?.sha;
  const baseSha = pull.base?.sha;
  const [files, checkRuns, comparison, reviewThreads] = await Promise.all([
    client.listPullFiles(pullNumber),
    client.listCheckRunsForRef(headSha),
    client.compare(baseSha, headSha),
    client.listReviewThreads(pullNumber),
  ]);
  requireCondition(reviewThreads.head_sha === headSha && reviewThreads.base_sha === baseSha, 'review-thread snapshot is stale');
  const result = evaluatePolicyGate({
    policy: contracts.policy,
    repository,
    pull,
    files,
    checkRuns,
    comparison,
    reviewThreads,
    expectedHeadSha,
  });
  const now = clock();
  requireCondition(now instanceof Date && Number.isFinite(now.getTime()), 'policy evaluation clock is invalid');
  return validatePolicyEvaluationArtifact({
    schema_version: POLICY_ARTIFACT_SCHEMA_VERSION,
    artifact_type: 'policy_evaluation',
    repository_id: repositoryId,
    repository,
    pull_number: pullNumber,
    head_sha: headSha,
    base_sha: baseSha,
    policy_sha: policySha,
    snapshot_sha: snapshotSha({
      repositoryState,
      mainSha,
      pull,
      files,
      checkRuns,
      comparison,
      reviewThreads,
      policy: contracts.policy,
    }),
    evaluated_at: now.toISOString(),
    result,
  });
}

export async function publishPolicyEvaluation({
  client,
  contracts,
  repository,
  repositoryId,
  pullNumber,
  policySha,
  expectedHeadSha,
  policyApp,
  runId,
  detailsUrl,
  expectedPolicySha = null,
  expectedFenceCheckRunId = null,
  clock = () => new Date(),
}) {
  requireCondition(policyApp && typeof policyApp === 'object', 'Policy App identity is required');
  const expectedHead = normalizedExpectedHead(expectedHeadSha);
  requireCondition(expectedHead !== null, 'expected PR head SHA is required');
  const generationState = await collectPolicyGeneration({
    client,
    contracts,
    repository,
    repositoryId,
    pullNumber,
    policySha,
    expectedHeadSha: expectedHead,
    expectedPolicySha,
  });
  const generation = {
    repository_id: repositoryId,
    repository,
    pull_number: pullNumber,
    head_sha: generationState.pull.head.sha,
    base_sha: generationState.pull.base.sha,
    policy_sha: policySha,
  };
  const currentExternalId = policyExternalId({
    repositoryId,
    pullNumber,
    headSha: generation.head_sha,
    policySha,
  });
  const currentGenerationChecks = (await client.listCheckRunsForRef(generation.head_sha))
    .filter((checkRun) => checkRun?.external_id === currentExternalId &&
      checkRun?.status === 'in_progress' && checkRun?.conclusion == null);
  requireCondition(
    currentGenerationChecks.length <= 1,
    'multiple in-progress policy checks exist for the current generation',
  );
  const checkName = contracts.policy.policy_gate.check_name;
  if (expectedFenceCheckRunId !== null) positiveInteger(expectedFenceCheckRunId, 'expected Policy fence check run ID');
  let check = await client.beginPolicyCheck(generation, checkName, detailsUrl, expectedFenceCheckRunId);
  let current;
  try {
    let previous = await collectPolicyEvaluation({
      client,
      contracts,
      repository,
      repositoryId,
      pullNumber,
      policySha,
      expectedHeadSha: generation.head_sha,
      expectedPolicySha,
      clock,
    });
    current = await collectPolicyEvaluation({
      client,
      contracts,
      repository,
      repositoryId,
      pullNumber,
      policySha,
      expectedHeadSha: generation.head_sha,
      expectedPolicySha,
      clock,
    });
    if (previous.snapshot_sha !== current.snapshot_sha) {
      previous = current;
      current = await collectPolicyEvaluation({
        client,
        contracts,
        repository,
        repositoryId,
        pullNumber,
        policySha,
        expectedHeadSha: generation.head_sha,
        expectedPolicySha,
        clock,
      });
      requireCondition(previous.snapshot_sha === current.snapshot_sha, 'policy inputs did not reach a stable snapshot');
    }
    check = await client.completePolicyCheck(check.id, current, checkName, detailsUrl);
    requireCondition(typeof check.html_url === 'string' && check.html_url.startsWith('https://github.com/'), 'published check URL is invalid');
    const verified = await collectPolicyEvaluation({
      client,
      contracts,
      repository,
      repositoryId,
      pullNumber,
      policySha,
      expectedHeadSha: generation.head_sha,
      expectedPolicySha,
      clock,
    });
    requireCondition(verified.snapshot_sha === current.snapshot_sha, 'policy inputs changed during check completion');
  } catch (error) {
    try {
      await client.restorePolicyCheckInProgress(check.id, generation, checkName, detailsUrl);
    } catch (recoveryError) {
      throw new AggregateError([error, recoveryError], 'policy check failed and could not be restored to in-progress');
    }
    throw error;
  }
  const now = clock();
  requireCondition(now instanceof Date && Number.isFinite(now.getTime()), 'policy publication clock is invalid');
  return validatePolicyReceiptArtifact({
    schema_version: POLICY_ARTIFACT_SCHEMA_VERSION,
    artifact_type: 'policy_receipt',
    state: 'published',
    evaluation: current,
    run_id: runId,
    policy_app_id: policyApp.id,
    check_run_id: check.id,
    check_url: check.html_url,
    conclusion: policyCheckConclusion(current.result),
    published_at: now.toISOString(),
  });
}

function clientFromEnvironment(environment, { policyApp = null } = {}) {
  return new PolicyGitHubClient({
    token: environment.AERIS_POLICY_TOKEN ?? environment.GITHUB_TOKEN,
    repository: environment.GITHUB_REPOSITORY,
    repositoryId: positiveInteger(environment.AERIS_REPOSITORY_ID, 'repository ID'),
    policyApp,
  });
}

export async function runPolicyGateCli({
  argv = process.argv.slice(2),
  environment = process.env,
  repoRoot = environment.GITHUB_WORKSPACE ?? defaultRepoRoot,
  contracts = null,
  clock = () => new Date(),
} = {}) {
  requireCondition(argv.length === 1 && ['evaluate', 'publish'].includes(argv[0]), 'usage: run-policy-gate.mjs <evaluate|publish>');
  const phase = argv[0];
  const loaded = contracts ?? loadContracts(repoRoot);
  assertEnabled(loaded, environment);
  const repository = environment.GITHUB_REPOSITORY;
  const repositoryId = positiveInteger(environment.AERIS_REPOSITORY_ID, 'repository ID');
  const sha = policyShaAt(repoRoot);
  const expectedHeadSha = normalizedExpectedHead(environment.AERIS_EXPECTED_HEAD_SHA);
  const expectedPolicySha = normalizedExpectedHead(environment.AERIS_EXPECTED_POLICY_SHA);
  requireCondition(expectedPolicySha !== null && expectedPolicySha === sha, 'trusted checkout is not bound to the expected policy SHA');
  requireCondition(typeof environment.AERIS_OUTPUT_PATH === 'string' && environment.AERIS_OUTPUT_PATH.length > 0, 'AERIS_OUTPUT_PATH is required');

  if (phase === 'evaluate') {
    requireCondition(typeof environment.GITHUB_TOKEN === 'string' && environment.GITHUB_TOKEN.length > 0, 'GITHUB_TOKEN is required');
    const artifact = await collectPolicyEvaluation({
      client: clientFromEnvironment(environment),
      contracts: loaded,
      repository,
      repositoryId,
      pullNumber: positiveInteger(environment.AERIS_PULL_REQUEST_NUMBER, 'pull request number'),
      policySha: sha,
      expectedHeadSha,
      expectedPolicySha,
      clock,
    });
    return writePolicyArtifactAtomic(environment.AERIS_OUTPUT_PATH, artifact, 'policy_evaluation');
  }

  requireCondition(typeof environment.AERIS_POLICY_TOKEN === 'string' && environment.AERIS_POLICY_TOKEN.length > 0, 'AERIS_POLICY_TOKEN is required');
  const policyApp = {
    id: positiveInteger(environment.AERIS_POLICY_APP_ID, 'Policy App ID'),
    slug: environment.AERIS_POLICY_APP_SLUG,
  };
  const runId = `${environment.GITHUB_RUN_ID ?? ''}.${environment.GITHUB_RUN_ATTEMPT ?? ''}`;
  const detailsUrl = `${environment.GITHUB_SERVER_URL ?? 'https://github.com'}/${repository}/actions/runs/${environment.GITHUB_RUN_ID}`;
  const expectedFenceCheckRunId = positiveInteger(
    environment.AERIS_EXPECTED_FENCE_CHECK_ID,
    'expected Policy fence check run ID',
  );
  const receipt = await publishPolicyEvaluation({
    client: clientFromEnvironment(environment, { policyApp }),
    contracts: loaded,
    repository,
    repositoryId,
    pullNumber: positiveInteger(environment.AERIS_PULL_REQUEST_NUMBER, 'pull request number'),
    policySha: sha,
    expectedHeadSha,
    policyApp,
    runId,
    detailsUrl,
    expectedPolicySha,
    expectedFenceCheckRunId,
    clock,
  });
  return writePolicyArtifactAtomic(environment.AERIS_OUTPUT_PATH, receipt, 'policy_receipt');
}

const invokedDirectly = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invokedDirectly) {
  try {
    await runPolicyGateCli();
  } catch (error) {
    console.error(`aeris policy gate failed: ${error.message}`);
    process.exitCode = 1;
  }
}
