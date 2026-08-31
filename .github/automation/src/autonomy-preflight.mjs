import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import { GitHubClient } from './github-client.mjs';

const SHA = /^[0-9a-f]{40}$/;
const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const WRITE_PERMISSIONS = new Set(['admin', 'maintain', 'write']);

export class AutonomyPreflightError extends Error {
  constructor(message) {
    super(message);
    this.name = 'AutonomyPreflightError';
  }
}

function reject(message) {
  throw new AutonomyPreflightError(message);
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

function labels(issue) {
  if (!Array.isArray(issue?.labels)) reject('Issue labels response is invalid');
  return issue.labels.map((label) => typeof label === 'string' ? label : label?.name).filter(Boolean);
}

export function writeAutonomyPreflightOutput(outputPath, result) {
  fs.mkdirSync(path.dirname(outputPath), { recursive: true, mode: 0o700 });
  fs.writeFileSync(outputPath, `${JSON.stringify(result, null, 2)}\n`, { mode: 0o600 });
}

export async function evaluateAutonomyPreflight(input, client) {
  const repository = required(input?.repository, 'repository', REPOSITORY);
  const repositoryId = positiveInteger(input?.repository_id, 'repository_id');
  const issueNumber = positiveInteger(input?.issue_number, 'issue_number');
  const actor = required(input?.actor, 'actor', /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$/);
  const baseRef = required(input?.base_ref, 'base_ref', /^refs\/heads\/[A-Za-z0-9._/-]+$/);
  if (baseRef !== 'refs/heads/main') reject('base_ref must be refs/heads/main');

  const [repositoryState, issue, permission, ref] = await Promise.all([
    client.request('GET', `/repos/${repository}`),
    client.getIssue(issueNumber),
    client.getCollaboratorPermission(actor),
    client.request('GET', `/repos/${repository}/git/ref/heads/main`),
  ]);
  if (repositoryState?.id !== repositoryId || repositoryState?.full_name !== repository) {
    reject('repository identity does not match the workflow event');
  }
  if (repositoryState?.default_branch !== 'main' || repositoryState?.archived === true) {
    reject('repository default branch is unavailable for autonomy');
  }
  if (!WRITE_PERMISSIONS.has(permission)) reject('workflow actor lacks repository write access');
  if (issue?.number !== issueNumber || issue?.state !== 'open' || issue?.pull_request) {
    reject('target must be an open Issue');
  }
  if (!labels(issue).includes('agent-ready')) reject('Issue is missing the agent-ready label');
  const baseSha = ref?.object?.sha;
  if (typeof baseSha !== 'string' || !SHA.test(baseSha)) reject('base branch SHA is invalid');

  return Object.freeze({
    schema_version: 1,
    repository,
    repository_id: repositoryId,
    task_id: `issue:${issueNumber}`,
    issue_number: issueNumber,
    issue_updated_at: required(issue.updated_at, 'issue.updated_at'),
    actor,
    base_ref: baseRef,
    base_sha: baseSha,
  });
}

export async function runAutonomyPreflight(environment = process.env) {
  const repository = required(environment.GITHUB_REPOSITORY, 'GITHUB_REPOSITORY', REPOSITORY);
  const client = new GitHubClient({
    token: required(environment.GITHUB_TOKEN, 'GITHUB_TOKEN'),
    repository,
    apiUrl: environment.GITHUB_API_URL,
  });
  const result = await evaluateAutonomyPreflight({
    repository,
    repository_id: environment.GITHUB_REPOSITORY_ID,
    issue_number: environment.AERIS_ISSUE_NUMBER,
    actor: environment.GITHUB_ACTOR,
    base_ref: 'refs/heads/main',
  }, client);
  if (environment.AERIS_PREFLIGHT_OUTPUT) {
    writeAutonomyPreflightOutput(environment.AERIS_PREFLIGHT_OUTPUT, result);
  }
  if (environment.GITHUB_OUTPUT) {
    fs.appendFileSync(
      environment.GITHUB_OUTPUT,
      `repository=${result.repository}\nrepository_id=${result.repository_id}\nissue_number=${result.issue_number}\nbase_ref=${result.base_ref}\nbase_sha=${result.base_sha}\n`,
    );
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  await runAutonomyPreflight();
}
