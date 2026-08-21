import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import { evaluateAutonomyPreflight } from './autonomy-preflight.mjs';
import { GitHubClient } from './github-client.mjs';

const SHA = /^[0-9a-f]{40}$/;
const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const LOGIN = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$/;
const MAXIMUM_CANDIDATE_ARTIFACT_BYTES = 2 * 1024 * 1024;
const MAXIMUM_RUNTIME_ARTIFACT_BYTES = 1024 * 1024;

export class AutonomyPublishPreflightError extends Error {
  constructor(message) {
    super(message);
    this.name = 'AutonomyPublishPreflightError';
  }
}

function reject(message) {
  throw new AutonomyPublishPreflightError(message);
}

function required(value, name, pattern = null) {
  if (typeof value !== 'string' || value.length === 0 || /[\u0000-\u001f\u007f]/.test(value)) {
    reject(`${name} is invalid`);
  }
  if (pattern && !pattern.test(value)) reject(`${name} format is invalid`);
  return value;
}

function positiveInteger(value, name) {
  const parsed = typeof value === 'string' && /^[1-9][0-9]*$/.test(value) ? Number(value) : value;
  if (!Number.isSafeInteger(parsed) || parsed <= 0) reject(`${name} must be a positive integer`);
  return parsed;
}

function workflowIdentity(run) {
  return {
    repository: run?.repository?.full_name,
    repository_id: run?.repository?.id,
    head_repository: run?.head_repository?.full_name,
    head_repository_id: run?.head_repository?.id,
    id: run?.id,
    run_attempt: run?.run_attempt,
    event: run?.event,
    status: run?.status,
    conclusion: run?.conclusion,
    name: run?.name,
    path: run?.path,
    head_branch: run?.head_branch,
    head_sha: run?.head_sha,
    actor: run?.actor?.login,
  };
}

export class AutonomyPublishPreflightClient extends GitHubClient {
  getWorkflowRun(runId) {
    return this.request('GET', `/repos/${this.repository}/actions/runs/${runId}`);
  }

  getRunArtifacts(runId) {
    return this.request('GET', `/repos/${this.repository}/actions/runs/${runId}/artifacts?per_page=100&page=1`);
  }
}

export async function evaluatePublishPreflight(input, client) {
  const repository = required(input?.repository, 'repository', REPOSITORY);
  const repositoryId = positiveInteger(input?.repository_id, 'repository_id');
  const runId = positiveInteger(input?.run_id, 'run_id');
  const runAttempt = positiveInteger(input?.run_attempt, 'run_attempt');
  const run = workflowIdentity(await client.getWorkflowRun(runId));
  if (run.repository !== repository || run.repository_id !== repositoryId ||
      run.head_repository !== repository || run.head_repository_id !== repositoryId) {
    reject('candidate workflow run repository identity is invalid');
  }
  if (run.id !== runId || run.run_attempt !== runAttempt || run.event !== 'workflow_dispatch' ||
      run.status !== 'completed' || run.conclusion !== 'success') {
    reject('candidate workflow run lifecycle is invalid');
  }
  if (run.name !== 'Agent candidate' || run.path !== '.github/workflows/agent-candidate.yml' || run.head_branch !== 'main') {
    reject('candidate workflow identity is invalid');
  }
  const headSha = required(run.head_sha, 'candidate workflow head SHA', SHA);
  const actor = required(run.actor, 'candidate workflow actor', LOGIN);

  const artifactPage = await client.getRunArtifacts(runId);
  if (!Number.isSafeInteger(artifactPage?.total_count) || artifactPage.total_count !== 2 ||
      !Array.isArray(artifactPage?.artifacts) || artifactPage.artifacts.length !== 2) {
    reject('candidate workflow must contain exactly the runtime and candidate artifacts');
  }
  const escapedRun = String(runId).replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const escapedAttempt = String(runAttempt).replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const candidatePattern = new RegExp(`^agent-candidate-issue-([1-9][0-9]*)-run-${escapedRun}-${escapedAttempt}$`);
  const runtimeName = `agent-candidate-runtime-${runId}-${runAttempt}`;
  const namedArtifacts = artifactPage.artifacts.map((artifact) => ({
    artifact,
    name: required(artifact?.name, 'artifact name'),
  }));
  const runtimeArtifacts = namedArtifacts.filter(({ name }) => name === runtimeName);
  const candidateArtifacts = namedArtifacts
    .map(({ artifact, name }) => ({ artifact, name, match: candidatePattern.exec(name) }))
    .filter(({ match }) => match !== null);
  if (runtimeArtifacts.length !== 1 || candidateArtifacts.length !== 1) {
    reject('candidate workflow artifact names do not form the exact bound pair');
  }
  const runtimeArtifact = runtimeArtifacts[0].artifact;
  const { artifact, name, match } = candidateArtifacts[0];
  const runtimeArtifactId = positiveInteger(runtimeArtifact.id, 'runtime artifact id');
  const candidateArtifactId = positiveInteger(artifact.id, 'candidate artifact id');
  if (runtimeArtifactId === candidateArtifactId) reject('candidate workflow artifact identities are not unique');
  if (runtimeArtifact?.expired !== false || !Number.isSafeInteger(runtimeArtifact?.size_in_bytes) ||
      runtimeArtifact.size_in_bytes <= 0 || runtimeArtifact.size_in_bytes > MAXIMUM_RUNTIME_ARTIFACT_BYTES) {
    reject('runtime artifact lifecycle or size is invalid');
  }
  if (artifact?.expired !== false || !Number.isSafeInteger(artifact?.size_in_bytes) ||
      artifact.size_in_bytes <= 0 || artifact.size_in_bytes > MAXIMUM_CANDIDATE_ARTIFACT_BYTES) {
    reject('candidate artifact lifecycle or size is invalid');
  }

  const bound = await evaluateAutonomyPreflight({
    repository,
    repository_id: repositoryId,
    issue_number: Number(match[1]),
    actor,
    base_ref: 'refs/heads/main',
  }, client);
  if (bound.base_sha !== headSha) reject('candidate workflow base is stale');
  return Object.freeze({
    ...bound,
    trigger_run_id: String(runId),
    trigger_run_attempt: runAttempt,
    artifact_id: candidateArtifactId,
    artifact_name: name,
  });
}

export async function runPublishPreflight(environment = process.env, dependencies = {}) {
  const repository = required(environment.GITHUB_REPOSITORY, 'GITHUB_REPOSITORY', REPOSITORY);
  const client = dependencies.client ?? new AutonomyPublishPreflightClient({
    token: required(environment.GITHUB_TOKEN, 'GITHUB_TOKEN'),
    repository,
    apiUrl: environment.GITHUB_API_URL,
  });
  const result = await evaluatePublishPreflight({
    repository,
    repository_id: environment.GITHUB_REPOSITORY_ID,
    run_id: environment.AERIS_TRIGGER_RUN_ID,
    run_attempt: environment.AERIS_TRIGGER_RUN_ATTEMPT,
  }, client);
  if (environment.GITHUB_OUTPUT) {
    fs.appendFileSync(environment.GITHUB_OUTPUT, [
      `repository=${result.repository}`,
      `repository_id=${result.repository_id}`,
      `issue_number=${result.issue_number}`,
      `base_ref=${result.base_ref}`,
      `base_sha=${result.base_sha}`,
      `trigger_run_id=${result.trigger_run_id}`,
      `trigger_run_attempt=${result.trigger_run_attempt}`,
      `artifact_id=${result.artifact_id}`,
      `artifact_name=${result.artifact_name}`,
      '',
    ].join('\n'));
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  await runPublishPreflight();
}
