import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import { GitHubClient } from './github-client.mjs';

export const HOLD_CHECK_NAME = 'Autonomy Finalizer / hold';
export const GITHUB_ACTIONS_APP_ID = 15368;
export const GITHUB_ACTIONS_APP_SLUG = 'github-actions';
export const MANAGED_MARKER = '<!-- aeris-autonomy-managed -->';

const SHA = /^[0-9a-f]{40}$/;
const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const ISSUE_BRANCH = /^agent\/issue-([1-9][0-9]*)$/;
const SYNC_BRANCH = 'automation/sync-upstream';
const SYNC_MARKER = '<!-- upstream-sync-managed -->';
const SYNC_SOURCE = /<!-- upstream-sync-source:[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+@[0-9a-f]{40} -->/;

export class AutonomyHoldInitializerError extends Error {
  constructor(message) { super(message); this.name = 'AutonomyHoldInitializerError'; }
}

function reject(message) { throw new AutonomyHoldInitializerError(message); }
function string(value, name, pattern = null) {
  if (typeof value !== 'string' || value.length === 0 || /[\u0000-\u001f\u007f]/.test(value) || (pattern && !pattern.test(value))) {
    reject(`${name} is invalid`);
  }
  return value;
}
function positive(value, name) {
  const number = typeof value === 'string' && /^[1-9][0-9]*$/.test(value) ? Number(value) : value;
  if (!Number.isSafeInteger(number) || number <= 0) reject(`${name} must be a positive integer`);
  return number;
}
function object(value, name) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) reject(`${name} is invalid`);
  return value;
}

export function holdExternalId({ repository_id, pull_number, head_sha }) {
  return `aeris-finalizer-hold:v1:${positive(repository_id, 'repository id')}:${positive(pull_number, 'pull number')}:${string(head_sha, 'head SHA', SHA)}`;
}

export function configFromEnvironment(environment = process.env) {
  const repository = string(environment.GITHUB_REPOSITORY, 'GITHUB_REPOSITORY', REPOSITORY);
  const writerSlug = string(environment.AERIS_WRITER_APP_SLUG, 'AERIS_WRITER_APP_SLUG', /^[a-z0-9][a-z0-9-]*$/);
  return Object.freeze({
    repository,
    repository_id: positive(environment.GITHUB_REPOSITORY_ID, 'GITHUB_REPOSITORY_ID'),
    pull_number: positive(environment.AERIS_PULL_NUMBER, 'AERIS_PULL_NUMBER'),
    default_branch: string(environment.AERIS_DEFAULT_BRANCH, 'AERIS_DEFAULT_BRANCH'),
    writer_login: `${writerSlug}[bot]`,
  });
}

function normalizedRepository(value) {
  value = object(value, 'repository projection');
  return Object.freeze({ id: positive(value.id, 'repository id'), full_name: string(value.full_name, 'repository full name', REPOSITORY), default_branch: string(value.default_branch, 'repository default branch') });
}

function normalizedPull(value) {
  value = object(value, 'pull request projection');
  const base = object(value.base, 'pull request base');
  const head = object(value.head, 'pull request head');
  const user = object(value.user, 'pull request user');
  return Object.freeze({
    number: positive(value.number, 'pull request number'),
    state: string(value.state, 'pull request state'),
    body: value.body === null ? '' : typeof value.body === 'string' ? value.body : reject('pull request body is invalid'),
    base_ref: string(base.ref, 'pull request base ref'),
    base_repository: string(object(base.repo, 'pull request base repository').full_name, 'pull request base repository name', REPOSITORY),
    head_ref: string(head.ref, 'pull request head ref'),
    head_sha: string(head.sha, 'pull request head SHA', SHA),
    head_repository: string(object(head.repo, 'pull request head repository').full_name, 'pull request head repository name', REPOSITORY),
    user_type: string(user.type, 'pull request user type'),
    user_login: string(user.login, 'pull request user login'),
  });
}

function equal(left, right) { return JSON.stringify(left) === JSON.stringify(right); }

function validateStableSnapshot(repository, pull, config) {
  if (repository.id !== config.repository_id || repository.full_name !== config.repository ||
      repository.default_branch !== 'main' || config.default_branch !== 'main') {
    reject('repository identity or default branch drifted');
  }
  if (pull.number !== config.pull_number || pull.state !== 'open' || pull.base_ref !== 'main' ||
      pull.base_repository !== config.repository) {
    reject('pull request identity is not an open main pull request for this repository');
  }
  return Object.freeze({ repository_id: repository.id, pull_number: pull.number, head_sha: pull.head_sha });
}

export function classifyPull(pull, config) {
  const writerPull = pull.head_repository === config.repository &&
    pull.user_type === 'Bot' && pull.user_login === config.writer_login;
  if (!writerPull) return Object.freeze({ conclusion: 'success', managed: false, reason: 'unmanaged pull request' });

  if (pull.head_ref === SYNC_BRANCH) {
    const validSync = pull.body.includes(SYNC_MARKER) &&
      pull.body.includes(`<!-- upstream-sync-owned-tip:${pull.head_sha} -->`) && SYNC_SOURCE.test(pull.body);
    return validSync
      ? Object.freeze({ conclusion: 'success', managed: true, reason: 'managed upstream synchronization pull request' })
      : Object.freeze({ conclusion: 'failure', managed: false, reason: 'writer synchronization pull request has invalid managed metadata' });
  }

  const issue = ISSUE_BRANCH.exec(pull.head_ref);
  if (issue === null) {
    return Object.freeze({ conclusion: 'failure', managed: false, reason: 'writer pull request uses an unauthorized branch' });
  }
  const issueNumber = issue[1];
  const valid = pull.body.includes(MANAGED_MARKER) && pull.body.includes(`<!-- aeris-autonomy-task:issue:${issueNumber} -->`);
  return valid
    ? Object.freeze({ conclusion: null, managed: true, reason: 'managed writer pull request' })
    : Object.freeze({ conclusion: 'failure', managed: false, reason: 'writer pull request has invalid managed marker' });
}

function checkIdentity(check, expected, externalId) {
  return check?.name === HOLD_CHECK_NAME && check?.head_sha === expected.head_sha && check?.external_id === externalId &&
    check?.app?.id === GITHUB_ACTIONS_APP_ID && check?.app?.slug === GITHUB_ACTIONS_APP_SLUG &&
    Array.isArray(check?.pull_requests) && check.pull_requests.length === 1 && check.pull_requests[0]?.number === expected.pull_number &&
    Number.isSafeInteger(check?.id) && check.id > 0;
}

function exactCheck(checks, expected, externalId) {
  if (!Array.isArray(checks)) reject('check runs response is invalid');
  const named = checks.filter((check) => check?.name === HOLD_CHECK_NAME && check?.head_sha === expected.head_sha);
  const matching = named.filter((check) => check?.external_id === externalId);
  if (matching.length > 1) reject('duplicate exact hold checks exist');
  if (named.length !== matching.length) reject('foreign hold check exists for the exact head');
  if (matching.length === 0) return null;
  if (!checkIdentity(matching[0], expected, externalId)) reject('hold check identity is invalid');
  return matching[0];
}

function stateMatches(check, desired) {
  if (desired.conclusion === null) return check.status === 'in_progress' && check.conclusion === null;
  return check.status === 'completed' && check.conclusion === desired.conclusion;
}

export class AutonomyHoldGitHubClient extends GitHubClient {
  getRepository() { return this.request('GET', `/repos/${this.repository}`); }
  getPull(number) { return super.getPull(positive(number, 'pull number')); }
  listChecks(sha) { return this.listCheckRunsForRef(string(sha, 'head SHA', SHA)); }
  getCheck(id) { return this.request('GET', `/repos/${this.repository}/check-runs/${positive(id, 'check run id')}`); }
  createCheck(sha, externalId, desired) {
    const body = { name: HOLD_CHECK_NAME, head_sha: sha, external_id: externalId, status: desired.conclusion === null ? 'in_progress' : 'completed' };
    if (desired.conclusion !== null) body.conclusion = desired.conclusion;
    return this.request('POST', `/repos/${this.repository}/check-runs`, { body });
  }
  completeCheck(id, externalId, conclusion) {
    return this.request('PATCH', `/repos/${this.repository}/check-runs/${positive(id, 'check run id')}`, {
      body: { external_id: externalId, status: 'completed', conclusion },
    });
  }
}

async function stableRead(client, config) {
  const firstRepository = normalizedRepository(await client.getRepository());
  const firstPull = normalizedPull(await client.getPull(config.pull_number));
  const secondRepository = normalizedRepository(await client.getRepository());
  const secondPull = normalizedPull(await client.getPull(config.pull_number));
  if (!equal(firstRepository, secondRepository) || !equal(firstPull, secondPull)) reject('repository or pull request drifted during authoritative reads');
  const expected = validateStableSnapshot(secondRepository, secondPull, config);
  return Object.freeze({ expected, pull: secondPull });
}

async function confirm(client, expected, externalId, desired) {
  const listed = exactCheck(await client.listChecks(expected.head_sha), expected, externalId);
  if (listed === null) reject('hold check could not be confirmed');
  const fetched = await client.getCheck(listed.id);
  if (!checkIdentity(fetched, expected, externalId) || !stateMatches(fetched, desired)) reject('hold check mutation was not confirmed');
  return fetched;
}

export async function initializeAutonomyHold({ client, config }) {
  const snapshot = await stableRead(client, config);
  const desired = classifyPull(snapshot.pull, config);
  const externalId = holdExternalId(snapshot.expected);
  let hold = exactCheck(await client.listChecks(snapshot.expected.head_sha), snapshot.expected, externalId);
  if (hold === null) {
    try { await client.createCheck(snapshot.expected.head_sha, externalId, desired); } catch (error) {
      // A response can be lost after GitHub persists the creation. Adopt only an exact unique check.
      hold = exactCheck(await client.listChecks(snapshot.expected.head_sha), snapshot.expected, externalId);
      if (hold === null) throw error;
    }
  }
  if (hold === null) hold = await confirm(client, snapshot.expected, externalId, desired);
  if (!checkIdentity(hold, snapshot.expected, externalId)) reject('hold check identity is invalid');
  if (!stateMatches(hold, desired)) {
    if (hold.status === 'completed') reject('completed hold check cannot be repurposed');
    if (hold.status !== 'in_progress' || hold.conclusion !== null || desired.conclusion === null) reject('hold check state is not safely mutable');
    if (desired.conclusion === 'success') reject('pending hold cannot be released as unmanaged');
    // A pending hold becomes terminal only after the malformed Writer
    // classification has been read again from the authoritative REST objects.
    const reconfirmed = await stableRead(client, config);
    if (!equal(reconfirmed.expected, snapshot.expected) || !equal(reconfirmed.pull, snapshot.pull) ||
        classifyPull(reconfirmed.pull, config).conclusion !== desired.conclusion) {
      reject('pull request classification drifted before hold completion');
    }
    await client.completeCheck(hold.id, externalId, desired.conclusion);
  }
  const confirmed = await confirm(client, snapshot.expected, externalId, desired);
  const reread = normalizedPull(await client.getPull(config.pull_number));
  if (!equal(reread, snapshot.pull)) reject('pull request drifted after hold mutation');
  return Object.freeze({ action: confirmed.status === 'in_progress' ? 'held' : 'released', managed: desired.managed, pull_number: snapshot.expected.pull_number, head_sha: snapshot.expected.head_sha, external_id: externalId, reason: desired.reason });
}

export async function runAutonomyHoldInitializer(environment = process.env, dependencies = {}) {
  const config = configFromEnvironment(environment);
  const client = dependencies.client ?? new AutonomyHoldGitHubClient({ token: string(environment.GITHUB_TOKEN, 'GITHUB_TOKEN'), repository: config.repository, apiUrl: environment.GITHUB_API_URL });
  const result = await initializeAutonomyHold({ client, config });
  if (environment.GITHUB_OUTPUT) fs.appendFileSync(environment.GITHUB_OUTPUT, `action=${result.action}\nmanaged=${result.managed}\npull_number=${result.pull_number}\nhead_sha=${result.head_sha}\n`);
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) await runAutonomyHoldInitializer();
