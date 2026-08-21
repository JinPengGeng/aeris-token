import crypto from 'node:crypto';
import fs from 'node:fs';

const REPOSITORY_PATTERN = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const APP_SLUG_PATTERN = /^[a-z0-9][a-z0-9-]{0,99}$/;
const MANAGED_MARKER = '<!-- aeris-autonomy-managed -->';
const ISSUE_MARKER = /<!-- aeris-autonomy-task:issue:(\d+) -->/;
const SYNC_BRANCH = 'automation/sync-upstream';
const SYNC_MARKER = '<!-- upstream-sync-managed -->';
const LEGACY_SYNC_REST_WRITERS = new Set(['github-actions[bot]', 'app/github-actions']);
const LEGACY_SYNC_GRAPHQL_WRITERS = new Set(['github-actions']);

export class AutonomyExpiryError extends Error {
  constructor(message, status = null) {
    super(message);
    this.name = 'AutonomyExpiryError';
    this.status = status;
  }
}

function fail(message) { throw new AutonomyExpiryError(message); }

function positiveInteger(value, name) {
  if (!Number.isSafeInteger(value) || value <= 0) fail(`${name} must be a positive integer`);
  return value;
}

function exactRepository(value, name) {
  if (typeof value !== 'string' || !REPOSITORY_PATTERN.test(value)) fail(`${name} is invalid`);
  return value;
}

function parseExpiry(value) {
  if (typeof value !== 'string' || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/.test(value)) {
    fail('AERIS_AUTONOMY_EXPIRES_AT must be a UTC second timestamp');
  }
  const epoch = Date.parse(value);
  if (!Number.isFinite(epoch)) fail('autonomy expiry timestamp is not canonical');
  const canonical = new Date(epoch).toISOString().replace('.000Z', 'Z');
  if (canonical !== value) fail('autonomy expiry timestamp is not canonical');
  return epoch;
}

export function evaluateRevocationWindow({ expiresAt, nowMs = Date.now(), safetyWindowSeconds = 2_700, force = false }) {
  const expiryMs = typeof expiresAt === 'number' ? expiresAt : parseExpiry(expiresAt);
  if (!Number.isFinite(nowMs)) fail('current time is invalid');
  if (!Number.isSafeInteger(safetyWindowSeconds) || safetyWindowSeconds < 0 || safetyWindowSeconds > 31_536_000) {
    fail('safety window is invalid');
  }
  const revokeAtMs = expiryMs - safetyWindowSeconds * 1000;
  return Object.freeze({
    expiresAtMs: expiryMs,
    revokeAtMs,
    due: force || nowMs >= revokeAtMs,
    expired: nowMs >= expiryMs,
    forced: force,
  });
}

export function createAppJwt({ appId, privateKey, nowMs = Date.now() }) {
  positiveInteger(Number(appId), 'Writer App ID');
  if (typeof privateKey !== 'string' || !privateKey.includes('PRIVATE KEY')) fail('Writer App private key is missing');
  const now = Math.floor(nowMs / 1000);
  if (!Number.isSafeInteger(now) || now <= 0) fail('JWT clock is invalid');
  const encode = (value) => Buffer.from(JSON.stringify(value)).toString('base64url');
  const header = encode({ alg: 'RS256', typ: 'JWT' });
  const payload = encode({ iat: now - 60, exp: now + 540, iss: String(appId) });
  const input = `${header}.${payload}`;
  const signature = crypto.createSign('RSA-SHA256').update(input).end().sign(privateKey, 'base64url');
  return `${input}.${signature}`;
}

export class RevokerClient {
  constructor({ token, repository, apiUrl = 'https://api.github.com', fetchImpl = globalThis.fetch }) {
    if (!token) fail('GitHub token is not configured');
    this.repository = exactRepository(repository, 'repository');
    const parsed = new URL(apiUrl);
    if (parsed.protocol !== 'https:') fail('GitHub API URL must use HTTPS');
    this.apiUrl = parsed.toString().replace(/\/$/, '');
    this.token = token;
    this.fetchImpl = fetchImpl;
  }

  async request(method, path, { body, signal } = {}) {
    const requestSignal = signal
      ? AbortSignal.any([AbortSignal.timeout(15_000), signal])
      : AbortSignal.timeout(15_000);
    const response = await this.fetchImpl(`${this.apiUrl}${path}`, {
      method,
      headers: {
        accept: 'application/vnd.github+json',
        authorization: `Bearer ${this.token}`,
        'content-type': 'application/json',
        'x-github-api-version': '2022-11-28',
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: requestSignal,
    });
    const text = response.status === 204 ? '' : await response.text();
    let value = null;
    if (text) {
      try { value = JSON.parse(text); } catch { throw new AutonomyExpiryError('GitHub API returned invalid JSON', response.status); }
    }
    if (!response.ok) throw new AutonomyExpiryError(`GitHub API returned HTTP ${response.status}`, response.status);
    return value;
  }

  getApp() { return this.request('GET', '/app'); }
  async listInstallations(maximumPages = 10) {
    const installations = [];
    for (let page = 1; page <= maximumPages; page += 1) {
      const batch = await this.request('GET', `/app/installations?per_page=100&page=${page}`);
      if (!Array.isArray(batch)) fail('App installations response is invalid');
      installations.push(...batch);
      if (batch.length < 100) return installations;
    }
    fail('App installations pagination exceeds the safety limit');
  }
  getInstallation(id) { return this.request('GET', `/app/installations/${positiveInteger(id, 'installation id')}`); }
  async getInstallationOrMissing(id) {
    try {
      return await this.getInstallation(id);
    } catch (error) {
      if (error instanceof AutonomyExpiryError && error.status === 404) return null;
      throw error;
    }
  }
  getRepository({ signal } = {}) { return this.request('GET', `/repos/${this.repository}`, { signal }); }
  async listOpenPulls({ maximumPages = 10, signal } = {}) {
    const pulls = [];
    for (let page = 1; page <= maximumPages; page += 1) {
      const batch = await this.request('GET', `/repos/${this.repository}/pulls?state=open&sort=updated&direction=desc&per_page=100&page=${page}`, { signal });
      if (!Array.isArray(batch)) fail('open pull request response is invalid');
      pulls.push(...batch);
      if (batch.length < 100) return pulls;
    }
    fail('open pull request pagination exceeds the safety limit');
  }
  async graphql(query, variables, { signal } = {}) {
    const value = await this.request('POST', '/graphql', { body: { query, variables }, signal });
    if (Array.isArray(value?.errors) && value.errors.length) fail(`GitHub GraphQL error: ${value.errors[0].message ?? 'unknown'}`);
    return value?.data;
  }
  async getPullGovernance(number, { signal } = {}) {
    const [owner, name] = this.repository.split('/');
    const data = await this.graphql(`query($owner:String!,$name:String!,$number:Int!){ repository(owner:$owner,name:$name){ pullRequest(number:$number){ id number state body isDraft headRefName headRefOid headRepository{nameWithOwner} baseRefName baseRefOid baseRepository{nameWithOwner} author{__typename login} autoMergeRequest{enabledAt mergeMethod} } } }`, { owner, name, number }, { signal });
    const pull = data?.repository?.pullRequest;
    if (!pull) fail(`managed pull request #${number} disappeared`);
    return pull;
  }
  async disablePullRequestAutoMerge(id, { signal } = {}) {
    const data = await this.graphql('mutation($id:ID!){ disablePullRequestAutoMerge(input:{pullRequestId:$id}){ pullRequest{id autoMergeRequest{enabledAt mergeMethod}} } }', { id }, { signal });
    return data?.disablePullRequestAutoMerge?.pullRequest;
  }
  deleteInstallation(id) { return this.request('DELETE', `/app/installations/${positiveInteger(id, 'installation id')}`); }
  async deleteInstallationOrMissing(id) {
    try {
      await this.deleteInstallation(id);
      return true;
    } catch (error) {
      if (error instanceof AutonomyExpiryError && error.status === 404) return false;
      throw error;
    }
  }
}

function validateAppIdentity(app, config) {
  if (String(app?.id) !== String(config.appId) || typeof app?.slug !== 'string' ||
      !APP_SLUG_PATTERN.test(app.slug)) {
    fail('Writer App identity mismatch');
  }
  const configuredGraphQlLogin = typeof config.writerLogin === 'string'
    ? config.writerLogin.replace(/\[bot\]$/, '')
    : null;
  return Object.freeze({
    ...config,
    appSlug: app.slug,
    writerLogin: `${app.slug}[bot]`,
    writerRestLogins: Object.freeze([...new Set([config.writerLogin, `${app.slug}[bot]`].filter(Boolean))]),
    writerGraphQlLogins: Object.freeze([...new Set([configuredGraphQlLogin, app.slug].filter(Boolean))]),
  });
}

function validateInstallationIdentity(installation, config) {
  if (String(installation?.app_id) !== String(config.appId) ||
      installation?.account?.login !== config.owner || installation?.account?.type !== 'User') {
    fail('Writer App installation identity mismatch');
  }
}

function targetInstallations(installations, config) {
  return installations.filter((installation) => String(installation?.app_id) === String(config.appId) &&
    installation?.account?.login === config.owner && installation?.account?.type === 'User');
}

export function identifyManagedPull(pull, config) {
  const author = pull?.user?.login;
  const writerRestLogins = config.writerRestLogins ?? [config.writerLogin];
  const writerOwned = writerRestLogins.includes(author);
  const legacySyncOwned = LEGACY_SYNC_REST_WRITERS.has(author) &&
    pull?.head?.ref === config.syncBranch && pull?.head?.repo?.full_name === config.repository;
  if (!writerOwned && !legacySyncOwned) return null;
  if (pull?.state !== 'open' || !Number.isSafeInteger(pull?.number) || pull.number <= 0 ||
      typeof pull.node_id !== 'string' || pull.node_id.length === 0) {
    fail('Writer-owned pull request inventory is invalid');
  }
  const issue = typeof pull?.body === 'string' ? pull.body.match(ISSUE_MARKER)?.[1] : null;
  const isSync = pull?.head?.ref === config.syncBranch;
  return Object.freeze({
    number: pull.number,
    nodeId: pull.node_id,
    issue: issue ?? null,
    sync: isSync,
    author,
    legacySync: legacySyncOwned,
  });
}

function assertGovernanceIdentity(governance, candidate, config) {
  const writerGraphQlLogins = config.writerGraphQlLogins ?? [config.writerLogin?.replace(/\[bot\]$/, '')];
  const expectedAuthor = candidate.legacySync
    ? governance?.author?.__typename === 'Bot' && LEGACY_SYNC_GRAPHQL_WRITERS.has(governance?.author?.login)
    : governance?.author?.__typename === 'Bot' && writerGraphQlLogins.includes(governance?.author?.login);
  if (governance?.number !== candidate.number || governance?.id !== candidate.nodeId || !expectedAuthor) {
    fail(`managed pull request #${candidate.number} governance identity drifted`);
  }
  return governance.state === 'OPEN';
}

function pullInventorySignature(pulls) {
  const identities = pulls.map((pull) => {
    if (!Number.isSafeInteger(pull?.number) || pull.number <= 0 || typeof pull?.node_id !== 'string' || !pull.node_id) {
      fail('open pull request inventory is invalid');
    }
    return `${pull.number}:${pull.node_id}`;
  }).sort();
  if (new Set(identities).size !== identities.length) fail('open pull request inventory contains duplicates');
  return JSON.stringify(identities);
}

async function listStableOpenPulls(client, { signal } = {}) {
  const first = await client.listOpenPulls({ signal });
  const second = await client.listOpenPulls({ signal });
  if (pullInventorySignature(first) !== pullInventorySignature(second)) {
    fail('open pull request inventory moved during pagination');
  }
  return second;
}

async function listManagedPulls(client, config, { signal } = {}) {
  const candidates = (await listStableOpenPulls(client, { signal }))
    .map((pull) => identifyManagedPull(pull, config))
    .filter(Boolean);
  if (new Set(candidates.map((candidate) => candidate.number)).size !== candidates.length) {
    fail('managed pull request inventory contains duplicates');
  }
  return candidates;
}

function candidateSignature(candidates) {
  return JSON.stringify(candidates.map((candidate) => `${candidate.number}:${candidate.nodeId}`).sort());
}

const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function withAbortBudget(operation, milliseconds, message) {
  if (!Number.isSafeInteger(milliseconds) || milliseconds <= 0 || milliseconds > 300_000) {
    fail('cleanup budget is invalid');
  }
  const controller = new AbortController();
  let rejectOnAbort;
  const aborted = new Promise((resolve, reject) => { rejectOnAbort = reject; });
  const error = new AutonomyExpiryError(message);
  const onAbort = () => rejectOnAbort(error);
  controller.signal.addEventListener('abort', onAbort, { once: true });
  const timer = setTimeout(() => controller.abort(error), milliseconds);
  try {
    return await Promise.race([operation(controller.signal), aborted]);
  } finally {
    clearTimeout(timer);
    controller.signal.removeEventListener('abort', onAbort);
  }
}

async function disarmManagedPulls(client, config, { maximumPasses = 4, settle = wait, signal } = {}) {
  const observed = new Map();
  let previousSignature = null;
  for (let pass = 1; pass <= maximumPasses; pass += 1) {
    const candidates = await listManagedPulls(client, config, { signal });
    let mutations = 0;
    for (const candidate of candidates) {
      observed.set(candidate.number, candidate);
      const before = await client.getPullGovernance(candidate.number, { signal });
      if (!assertGovernanceIdentity(before, candidate, config)) continue;
      if (before.autoMergeRequest) {
        await client.disablePullRequestAutoMerge(candidate.nodeId, { signal });
        mutations += 1;
      }
      const after = await client.getPullGovernance(candidate.number, { signal });
      if (assertGovernanceIdentity(after, candidate, config) && after.autoMergeRequest) {
        fail(`auto-merge remains armed for managed pull request #${candidate.number}`);
      }
    }
    const signature = candidateSignature(candidates);
    if (signature === previousSignature && mutations === 0) {
      return [...observed.values()].sort((left, right) => left.number - right.number);
    }
    previousSignature = signature;
    if (pass < maximumPasses) await settle(1_000);
  }
  fail('managed pull request inventory did not converge');
}

async function resolveTargetInstallation(appClient, config) {
  if (config.installationId) {
    const configured = await appClient.getInstallationOrMissing(config.installationId);
    if (configured) {
      try {
        validateInstallationIdentity(configured, config);
        return { config, installation: configured };
      } catch {
        // A stale ID must not authorize deletion of a different account installation.
      }
    }
  }
  const matches = targetInstallations(await appClient.listInstallations(), config);
  if (matches.length > 1) fail('Writer App installation could not be uniquely discovered');
  if (matches.length === 0) return { config: Object.freeze({ ...config, installationId: null }), installation: null };
  const installationId = positiveInteger(matches[0].id, 'installation id');
  return { config: Object.freeze({ ...config, installationId }), installation: matches[0] };
}

export async function revokeAutonomy({
  appClient,
  governanceClient,
  config,
  dryRun = false,
  nowMs = Date.now(),
  safetyWindowSeconds = 2_700,
  force = false,
  settle = wait,
  preCleanupBudgetMilliseconds = 60_000,
  postCleanupBudgetMilliseconds = 180_000,
}) {
  const window = evaluateRevocationWindow({ expiresAt: config.expiresAt, nowMs, safetyWindowSeconds, force });
  if (!window.due && !dryRun) return Object.freeze({ action: 'not_due', window, managedPulls: 0, pullNumbers: [] });
  const app = await appClient.getApp();
  const liveConfig = validateAppIdentity(app, config);
  const repository = await governanceClient.getRepository();
  if (String(repository?.id) !== String(config.repositoryId) || repository?.full_name !== config.repository) {
    fail('revocation token is not bound to the target repository');
  }
  const resolved = await resolveTargetInstallation(appClient, liveConfig);
  const runtimeConfig = resolved.config;
  const installation = resolved.installation;

  if (dryRun) {
    const candidates = await withAbortBudget(
      (signal) => listManagedPulls(governanceClient, runtimeConfig, { signal }),
      postCleanupBudgetMilliseconds,
      'dry-run inventory budget exhausted',
    );
    return Object.freeze({ action: 'dry_run', window, managedPulls: candidates.length, pullNumbers: candidates.map((x) => x.number) });
  }

  if (!installation) {
    const candidates = await withAbortBudget(
      (signal) => disarmManagedPulls(governanceClient, runtimeConfig, { settle, signal }),
      postCleanupBudgetMilliseconds,
      'post-uninstall cleanup budget exhausted',
    );
    return Object.freeze({ action: 'already_uninstalled', window, managedPulls: candidates.length, pullNumbers: candidates.map((x) => x.number) });
  }

  let beforeUninstall = [];
  let preCleanupError = null;
  try {
    beforeUninstall = await withAbortBudget(
      (signal) => disarmManagedPulls(governanceClient, runtimeConfig, { settle, signal }),
      preCleanupBudgetMilliseconds,
      'pre-uninstall cleanup budget exhausted',
    );
  } catch (error) {
    preCleanupError = error;
  }

  let deletionError = null;
  try {
    await appClient.deleteInstallationOrMissing(runtimeConfig.installationId);
  } catch (error) {
    deletionError = error;
  }

  const completionErrors = [];
  let afterUninstall = [];
  try {
    afterUninstall = await withAbortBudget(
      (signal) => disarmManagedPulls(governanceClient, runtimeConfig, { settle, signal }),
      postCleanupBudgetMilliseconds,
      'post-uninstall cleanup budget exhausted',
    );
  } catch (error) {
    completionErrors.push(error);
  }

  let uninstallVerified = true;
  try {
    if (await appClient.getInstallationOrMissing(runtimeConfig.installationId) !== null) {
      fail('Writer App installation uninstall was not verified');
    }
  } catch (error) {
    uninstallVerified = false;
    completionErrors.push(error);
  }
  try {
    const remainingInstallations = targetInstallations(await appClient.listInstallations(), runtimeConfig);
    if (remainingInstallations.length !== 0) fail('Writer App installation was recreated during revocation');
  } catch (error) {
    uninstallVerified = false;
    completionErrors.push(error);
  }
  if (!uninstallVerified && deletionError) completionErrors.unshift(deletionError);
  if (completionErrors.length) {
    throw new AggregateError(
      preCleanupError ? [preCleanupError, ...completionErrors] : completionErrors,
      'Writer App revocation or managed pull request cleanup is incomplete',
    );
  }
  const observed = new Map([...beforeUninstall, ...afterUninstall].map((candidate) => [candidate.number, candidate]));
  const candidates = [...observed.values()].sort((left, right) => left.number - right.number);
  return Object.freeze({ action: 'uninstalled', window, managedPulls: candidates.length, pullNumbers: candidates.map((x) => x.number) });
}

function envConfig(environment) {
  const repository = exactRepository(environment.GITHUB_REPOSITORY, 'GITHUB_REPOSITORY');
  const appSlug = environment.AERIS_WRITER_APP_SLUG;
  if (typeof appSlug !== 'string' || !APP_SLUG_PATTERN.test(appSlug)) fail('Writer App slug is invalid');
  const installationId = environment.AERIS_WRITER_INSTALLATION_ID
    ? positiveInteger(Number(environment.AERIS_WRITER_INSTALLATION_ID), 'Writer installation id')
    : null;
  return {
    repository,
    repositoryId: positiveInteger(Number(environment.GITHUB_REPOSITORY_ID), 'repository id'),
    owner: repository.split('/')[0],
    defaultBranch: environment.AERIS_DEFAULT_BRANCH || 'main',
    writerLogin: environment.AERIS_WRITER_LOGIN || `${appSlug}[bot]`,
    branchPrefix: environment.AERIS_MANAGED_BRANCH_PREFIX || 'agent/issue-',
    syncBranch: environment.AERIS_SYNC_BRANCH || SYNC_BRANCH,
    appId: positiveInteger(Number(environment.AERIS_WRITER_APP_ID), 'Writer App ID'),
    appSlug,
    installationId,
    expiresAt: environment.AERIS_AUTONOMY_EXPIRES_AT,
  };
}

function publishOutputs(environment, result) {
  if (environment.GITHUB_OUTPUT) {
    fs.appendFileSync(environment.GITHUB_OUTPUT, `action=${result.action}\nmanaged_pulls=${result.managedPulls}\n`);
  }
  return result;
}

function assertForceIsQuiescent(environment, force) {
  if (!force) return;
  const enabled = ['AERIS_AGENTS_ENABLED', 'AERIS_WRITER_ENABLED', 'AERIS_UPSTREAM_SYNC_ENABLED', 'AERIS_AUTONOMOUS_MERGE_ENABLED']
    .filter((name) => ['1', 'true'].includes(String(environment[name] ?? '').toLowerCase()));
  if (enabled.length) fail(`force revocation requires disabled production switches: ${enabled.join(', ')}`);
}

function cleanupBudgetMilliseconds(environment, name, defaultSeconds) {
  const seconds = Number(environment[name] || defaultSeconds);
  if (!Number.isSafeInteger(seconds) || seconds <= 0 || seconds > 300) fail(`${name} is invalid`);
  return seconds * 1_000;
}

export async function main(environment = process.env, { nowMs = Date.now(), fetchImpl = globalThis.fetch, settle = wait } = {}) {
  const config = envConfig(environment);
  const force = environment.AERIS_FORCE === 'true' || environment.AERIS_FORCE === '1';
  const dryRun = environment.AERIS_DRY_RUN === 'true' || environment.AERIS_DRY_RUN === '1';
  const window = evaluateRevocationWindow({ expiresAt: config.expiresAt, nowMs, safetyWindowSeconds: Number(environment.AERIS_EXPIRY_SAFETY_WINDOW_SECONDS || 2_700), force });
  if (!window.due && environment.AERIS_DRY_RUN !== 'true' && environment.AERIS_DRY_RUN !== '1') {
    return publishOutputs(environment, { action: 'not_due', window, managedPulls: 0, pullNumbers: [] });
  }
  assertForceIsQuiescent(environment, force);
  const appJwt = createAppJwt({ appId: config.appId, privateKey: environment.AERIS_WRITER_APP_PRIVATE_KEY, nowMs });
  const appClient = new RevokerClient({ token: appJwt, repository: config.repository, apiUrl: environment.GITHUB_API_URL, fetchImpl });
  const governanceClient = new RevokerClient({ token: environment.AERIS_REVOCATION_TOKEN, repository: config.repository, apiUrl: environment.GITHUB_API_URL, fetchImpl });
  const result = await revokeAutonomy({
    appClient,
    governanceClient,
    config,
    dryRun,
    force,
    safetyWindowSeconds: Number(environment.AERIS_EXPIRY_SAFETY_WINDOW_SECONDS || 2_700),
    nowMs,
    settle,
    preCleanupBudgetMilliseconds: cleanupBudgetMilliseconds(environment, 'AERIS_PRE_UNINSTALL_CLEANUP_BUDGET_SECONDS', 60),
    postCleanupBudgetMilliseconds: cleanupBudgetMilliseconds(environment, 'AERIS_POST_UNINSTALL_CLEANUP_BUDGET_SECONDS', 180),
  });
  return publishOutputs(environment, result);
}

if (import.meta.url === `file://${process.argv[1]?.replaceAll('\\', '/')}`) {
  main().then((result) => console.log(JSON.stringify(result))).catch((error) => { console.error(error.message); process.exitCode = 1; });
}
