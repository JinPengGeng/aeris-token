import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import {
  createGitHubAppJwt,
  GitHubAppAttestationClient,
  GitHubInstallationTokenProofClient,
  validateWriterPermissions,
} from './github-app-attestation.mjs';

const TRUST = Object.freeze({
  repository: 'JinPengGeng/aeris-token',
  repository_id: 1316750512,
  default_branch: 'main',
  owner_login: 'JinPengGeng',
  owner_database_id: 36217715,
  owner_type: 'User',
  app_id: 4667256,
  app_slug: 'aeris-token-writer',
  installation_id: 155342531,
});
const PRODUCTION_SWITCHES = Object.freeze([
  'AERIS_AGENTS_ENABLED',
  'AERIS_CANDIDATE_AGENTS_ENABLED',
  'AERIS_WRITER_ENABLED',
  'AERIS_UPSTREAM_SYNC_ENABLED',
  'AERIS_AUTONOMOUS_MERGE_ENABLED',
]);
const BOOTSTRAP_FLAG = 'AERIS_WRITER_IDENTITY_BOOTSTRAP_ENABLED';
const CANARY_FLAG = 'AERIS_WRITER_GOVERNANCE_CANARY_ENABLED';
const CONTROL_VARIABLES = Object.freeze([...PRODUCTION_SWITCHES, BOOTSTRAP_FLAG, CANARY_FLAG]);
const NODE_ID_VARIABLE = 'AERIS_WRITER_APP_NODE_ID';
const OWNER_ID_VARIABLE = 'AERIS_WRITER_APP_OWNER_DATABASE_ID';
const IDENTITY_VARIABLES = Object.freeze([NODE_ID_VARIABLE, OWNER_ID_VARIABLE]);
const NODE_ID = /^[A-Za-z0-9_+=/-]{8,256}$/;
const COMMIT_SHA = /^[0-9a-f]{40}$/;
const MAXIMUM_RESPONSE_BYTES = 1024 * 1024;
const MAXIMUM_INSTALLATION_TOKEN_LIFETIME_MS = 65 * 60 * 1000;
const MAXIMUM_CONTROL_TOKEN_REMAINING_LIFETIME_MS = 7 * 24 * 60 * 60 * 1000;

export class WriterIdentityBootstrapError extends Error {
  constructor(message, status = null) {
    super(message);
    this.name = 'WriterIdentityBootstrapError';
    this.status = status;
  }
}

function reject(message, status = null) {
  throw new WriterIdentityBootstrapError(message, status);
}

function object(value, name) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) reject(`${name} is invalid`);
  return value;
}

function required(value, name, pattern = null) {
  if (typeof value !== 'string' || value.length === 0 || value.length > 512 ||
      /[\u0000-\u001f\u007f]/.test(value) || (pattern && !pattern.test(value))) {
    reject(`${name} is invalid`);
  }
  return value;
}

function secretPresent(value, name) {
  if (typeof value !== 'string' || value.length === 0 || value.length > 64 * 1024 || /[\u0000\u007f]/.test(value)) {
    reject(`${name} is missing or invalid`);
  }
}

function positiveInteger(value, name) {
  const parsed = typeof value === 'string' && /^[1-9][0-9]*$/.test(value) ? Number(value) : value;
  if (!Number.isSafeInteger(parsed) || parsed <= 0) reject(`${name} must be a positive integer`);
  return parsed;
}

function exactEmptyEvents(value, name) {
  if (!Array.isArray(value) || value.length !== 0) reject(`${name} must be empty`);
  return Object.freeze([]);
}

function exactWriterPermissions(value, name) {
  try {
    return validateWriterPermissions(value, name);
  } catch {
    reject(`${name} is not the approved minimal permission set`);
  }
}

function exactFalse(value, name) {
  if (value !== 'false') reject(`${name} must be exactly false during identity bootstrap`);
}

function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  if (value && typeof value === 'object') {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(',')}}`;
  }
  return JSON.stringify(value);
}

function sha256(value) {
  return crypto.createHash('sha256').update(value, 'utf8').digest('hex');
}

function validateRuntime(environment) {
  const workflowSha = required(environment.GITHUB_SHA, 'GITHUB_SHA', COMMIT_SHA);
  if (environment.GITHUB_REPOSITORY !== TRUST.repository ||
      positiveInteger(environment.GITHUB_REPOSITORY_ID, 'GITHUB_REPOSITORY_ID') !== TRUST.repository_id ||
      environment.AERIS_DEFAULT_BRANCH !== TRUST.default_branch ||
      environment.GITHUB_REF !== `refs/heads/${TRUST.default_branch}` ||
      environment.GITHUB_ACTOR !== TRUST.owner_login) {
    reject('identity bootstrap repository, branch, or actor is not trusted');
  }
  if (environment[BOOTSTRAP_FLAG] !== 'true') reject(`${BOOTSTRAP_FLAG} must be exactly true`);
  if (environment[CANARY_FLAG] !== 'true') {
    reject(`${CANARY_FLAG} must be exactly true so the called governance canary cannot be skipped`);
  }
  for (const name of PRODUCTION_SWITCHES) exactFalse(environment[name], name);
  secretPresent(environment.AERIS_WRITER_APP_PRIVATE_KEY, 'AERIS_WRITER_APP_PRIVATE_KEY');
  secretPresent(environment.AERIS_IDENTITY_BOOTSTRAP_TOKEN, 'AERIS_IDENTITY_BOOTSTRAP_TOKEN');
  return workflowSha;
}

export function normalizeWriterIdentityBootstrapSnapshot({ app, installation }) {
  const liveApp = object(app, 'authenticated Writer App');
  const owner = object(liveApp.owner, 'authenticated Writer App owner');
  const ownerId = positiveInteger(owner.id, 'authenticated Writer App owner database id');
  const appNodeId = required(liveApp.node_id, 'authenticated Writer App node id', NODE_ID);
  if (positiveInteger(liveApp.id, 'authenticated Writer App id') !== TRUST.app_id ||
      liveApp.slug !== TRUST.app_slug || owner.login !== TRUST.owner_login ||
      ownerId !== TRUST.owner_database_id || owner.type !== TRUST.owner_type) {
    reject('authenticated Writer App identity does not match bootstrap trust');
  }

  const liveInstallation = object(installation, 'authenticated Writer App installation');
  const account = object(liveInstallation.account, 'authenticated Writer App installation account');
  const accountId = positiveInteger(account.id, 'authenticated Writer App installation account database id');
  if (positiveInteger(liveInstallation.id, 'authenticated Writer installation id') !== TRUST.installation_id ||
      positiveInteger(liveInstallation.app_id, 'authenticated Writer installation App id') !== TRUST.app_id ||
      liveInstallation.app_slug !== TRUST.app_slug || account.login !== TRUST.owner_login ||
      account.type !== TRUST.owner_type || accountId !== ownerId ||
      positiveInteger(liveInstallation.target_id, 'authenticated Writer installation target id') !== ownerId ||
      liveInstallation.target_type !== TRUST.owner_type || liveInstallation.repository_selection !== 'selected' ||
      liveInstallation.suspended_at !== null || liveInstallation.suspended_by !== null) {
    reject('authenticated Writer App installation does not match bootstrap trust');
  }

  return Object.freeze({
    app_id: TRUST.app_id,
    app_slug: TRUST.app_slug,
    app_node_id: appNodeId,
    app_owner_login: TRUST.owner_login,
    app_owner_database_id: ownerId,
    app_owner_type: TRUST.owner_type,
    app_permissions: exactWriterPermissions(liveApp.permissions, 'authenticated Writer App permissions'),
    app_events: exactEmptyEvents(liveApp.events, 'authenticated Writer App events'),
    installation_id: TRUST.installation_id,
    installation_account_login: TRUST.owner_login,
    installation_account_database_id: accountId,
    installation_account_type: TRUST.owner_type,
    repository: TRUST.repository,
    repository_id: TRUST.repository_id,
    repository_selection: 'selected',
    installation_permissions: exactWriterPermissions(
      liveInstallation.permissions,
      'authenticated Writer App installation permissions',
    ),
    installation_events: exactEmptyEvents(
      liveInstallation.events,
      'authenticated Writer App installation events',
    ),
    suspended: false,
  });
}

async function readSnapshot(client) {
  const app = await client.getApp();
  const installation = await client.getInstallation(TRUST.installation_id);
  return normalizeWriterIdentityBootstrapSnapshot({ app, installation });
}

export async function doubleReadWriterIdentity(client) {
  if (!client || typeof client.getApp !== 'function' || typeof client.getInstallation !== 'function') {
    reject('identity bootstrap App client is invalid');
  }
  const first = await readSnapshot(client);
  const second = await readSnapshot(client);
  const firstCanonical = canonicalJson(first);
  const secondCanonical = canonicalJson(second);
  if (firstCanonical !== secondCanonical) reject('Writer App identity snapshot drifted between reads');
  return Object.freeze({ snapshot: second, canonical: secondCanonical, sha256: sha256(secondCanonical) });
}

function normalizeReadOnlyInventoryToken(value, nowMs) {
  const response = object(value, 'read-only Writer installation token response');
  secretPresent(response.token, 'read-only Writer installation token');
  const expiresAt = required(response.expires_at, 'read-only Writer installation token expiration');
  const expiresAtMs = Date.parse(expiresAt);
  if (!Number.isFinite(nowMs) || !Number.isFinite(expiresAtMs) || expiresAtMs <= nowMs ||
      expiresAtMs - nowMs > MAXIMUM_INSTALLATION_TOKEN_LIFETIME_MS) {
    reject('read-only Writer installation token expiration is invalid');
  }
  const permissions = object(response.permissions, 'read-only Writer installation token permissions');
  const keys = Object.keys(permissions).sort();
  if (!keys.includes('contents') || keys.some((key) => !['contents', 'metadata'].includes(key)) ||
      permissions.contents !== 'read' ||
      (permissions.metadata !== undefined && permissions.metadata !== 'read')) {
    reject('read-only Writer installation token permissions are not exact');
  }
  if (response.repository_selection !== undefined && response.repository_selection !== 'selected') {
    reject('read-only Writer installation token repository selection is invalid');
  }
  return Object.freeze({ token: response.token, expires_at: new Date(expiresAtMs).toISOString() });
}

function normalizeInstallationRepositoryInventory(value) {
  const inventory = object(value, 'Writer installation repository inventory');
  if (inventory.total_count !== 1 || !Array.isArray(inventory.repositories) ||
      inventory.repositories.length !== inventory.total_count) {
    reject('Writer installation repository inventory is not complete and exact');
  }
  const repository = object(inventory.repositories[0], 'Writer installation inventory repository');
  const owner = object(repository.owner, 'Writer installation inventory repository owner');
  if (positiveInteger(repository.id, 'Writer installation inventory repository id') !== TRUST.repository_id ||
      repository.full_name !== TRUST.repository || repository.name !== TRUST.repository.split('/')[1] ||
      repository.default_branch !== TRUST.default_branch || owner.login !== TRUST.owner_login ||
      positiveInteger(owner.id, 'Writer installation inventory repository owner database id') !==
        TRUST.owner_database_id || owner.type !== TRUST.owner_type) {
    reject('Writer installation repository inventory identity is invalid');
  }
  return Object.freeze({
    total_count: 1,
    repository: TRUST.repository,
    repository_id: TRUST.repository_id,
    repository_owner_login: TRUST.owner_login,
    repository_owner_database_id: TRUST.owner_database_id,
    repository_owner_type: TRUST.owner_type,
    default_branch: TRUST.default_branch,
  });
}

export async function doubleReadInstallationRepositoryInventory(client) {
  if (!client || typeof client.getCompleteInstallationRepositoryInventory !== 'function') {
    reject('identity bootstrap installation inventory client is invalid');
  }
  const first = normalizeInstallationRepositoryInventory(
    await client.getCompleteInstallationRepositoryInventory(),
  );
  const second = normalizeInstallationRepositoryInventory(
    await client.getCompleteInstallationRepositoryInventory(),
  );
  const firstCanonical = canonicalJson(first);
  const secondCanonical = canonicalJson(second);
  if (firstCanonical !== secondCanonical) {
    reject('Writer installation repository inventory drifted between complete reads');
  }
  return Object.freeze({ snapshot: second, canonical: secondCanonical, sha256: sha256(secondCanonical) });
}

function responseHeader(headers, name, requiredHeader = false) {
  if (!headers || typeof headers.get !== 'function' || typeof headers.has !== 'function') {
    reject('identity bootstrap control response headers are invalid');
  }
  if (!headers.has(name)) {
    if (requiredHeader) reject(`identity bootstrap control response is missing ${name}`);
    return null;
  }
  const value = headers.get(name);
  if (typeof value !== 'string' || /[\u0000\r\n\u007f]/.test(value)) {
    reject(`identity bootstrap control response ${name} is invalid`);
  }
  return value;
}

export class WriterIdentityBootstrapControlClient {
  constructor({ token, repository = TRUST.repository, apiUrl = 'https://api.github.com', fetchImpl = globalThis.fetch,
    timeoutMs = 15_000 }) {
    this.token = required(token, 'identity bootstrap control token');
    if (repository !== TRUST.repository) reject('identity bootstrap control repository is not trusted');
    let parsed;
    try { parsed = new URL(apiUrl); } catch { reject('GitHub API URL is invalid'); }
    if (parsed.protocol !== 'https:' || parsed.username || parsed.password || parsed.search || parsed.hash) {
      reject('GitHub API URL must be a credential-free HTTPS URL');
    }
    if (typeof fetchImpl !== 'function') reject('identity bootstrap fetch implementation is invalid');
    if (!Number.isSafeInteger(timeoutMs) || timeoutMs <= 0 || timeoutMs > 60_000) {
      reject('identity bootstrap timeout is invalid');
    }
    this.repository = repository;
    this.apiUrl = parsed.toString().replace(/\/$/, '');
    this.fetchImpl = fetchImpl;
    this.timeoutMs = timeoutMs;
  }

  async request(method, pathname, { body, expectedStatuses = [200], includeHeaders = false } = {}) {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), this.timeoutMs);
    try {
      const response = await this.fetchImpl(`${this.apiUrl}${pathname}`, {
        method,
        headers: {
          accept: 'application/vnd.github+json',
          authorization: `Bearer ${this.token}`,
          ...(body === undefined ? {} : { 'content-type': 'application/json' }),
          'x-github-api-version': '2022-11-28',
        },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
        signal: controller.signal,
      });
      if (!response || typeof response.status !== 'number' || typeof response.text !== 'function') {
        reject('identity bootstrap control response is invalid');
      }
      const text = await response.text();
      if (Buffer.byteLength(text, 'utf8') > MAXIMUM_RESPONSE_BYTES) {
        reject('identity bootstrap control response is too large');
      }
      if (!expectedStatuses.includes(response.status)) {
        reject(`identity bootstrap control request returned HTTP ${response.status}`, response.status);
      }
      let value = null;
      if (text.length > 0) {
        try { value = object(JSON.parse(text), 'identity bootstrap control response'); } catch {
          reject('identity bootstrap control request returned invalid JSON', response.status);
        }
      }
      if (includeHeaders) {
        return Object.freeze({ value, headers: response.headers });
      }
      return value;
    } catch (error) {
      if (error instanceof WriterIdentityBootstrapError) throw error;
      reject('identity bootstrap control request failed');
    } finally {
      clearTimeout(timeout);
    }
  }

  async getAuthenticatedUser(nowMs) {
    const { value, headers } = await this.request('GET', '/user', { includeHeaders: true });
    const oauthScopes = responseHeader(headers, 'x-oauth-scopes');
    if (oauthScopes !== null && oauthScopes !== '') {
      reject('identity bootstrap control token exposes classic OAuth scopes');
    }
    const expirationHeader = responseHeader(headers, 'github-authentication-token-expiration');
    let tokenExpiration = null;
    if (expirationHeader !== null && expirationHeader !== '') {
      const expirationMs = Date.parse(expirationHeader);
      if (!Number.isFinite(nowMs) || !Number.isFinite(expirationMs) || expirationMs <= nowMs ||
          expirationMs - nowMs > MAXIMUM_CONTROL_TOKEN_REMAINING_LIFETIME_MS) {
        reject('identity bootstrap control token expiration header is invalid');
      }
      tokenExpiration = new Date(expirationMs).toISOString();
    }
    return Object.freeze({ user: value, token_expiration: tokenExpiration });
  }

  getRepository() {
    return this.request('GET', `/repos/${this.repository}`);
  }

  getDefaultBranchRef() {
    return this.request('GET', `/repos/${this.repository}/git/ref/heads/${TRUST.default_branch}`);
  }

  getRepositoryVariable(name) {
    required(name, 'repository variable name', /^[A-Z][A-Z0-9_]{0,99}$/);
    return this.request('GET', `/repos/${this.repository}/actions/variables/${name}`);
  }

  async assertRepositoryVariable(name, expectedValue) {
    const variable = object(await this.getRepositoryVariable(name), `${name} repository variable`);
    if (variable.name !== name || variable.value !== expectedValue) {
      reject(`${name} repository variable write did not read back exactly`);
    }
  }

  async upsertRepositoryVariable(name, value) {
    required(name, 'repository variable name', /^[A-Z][A-Z0-9_]{0,99}$/);
    required(value, 'repository variable value');
    const base = `/repos/${this.repository}/actions/variables`;
    let existing;
    try {
      existing = await this.request('GET', `${base}/${name}`);
    } catch (error) {
      if (!(error instanceof WriterIdentityBootstrapError) || error.status !== 404) throw error;
    }
    if (existing) {
      await this.request('PATCH', `${base}/${name}`, { body: { name, value }, expectedStatuses: [204] });
    } else {
      await this.request('POST', base, { body: { name, value }, expectedStatuses: [201] });
    }
  }

  updateExistingRepositoryVariable(name, value) {
    required(name, 'repository variable name', /^[A-Z][A-Z0-9_]{0,99}$/);
    required(value, 'repository variable value');
    return this.request('PATCH', `/repos/${this.repository}/actions/variables/${name}`, {
      body: { name, value },
      expectedStatuses: [204],
    });
  }

  doubleReadTrustedState(options) {
    return doubleReadWriterIdentityControlState(this, options);
  }
}

function expectedControlVariables(bootstrapEnabled) {
  const variables = {};
  for (const name of PRODUCTION_SWITCHES) variables[name] = 'false';
  variables[BOOTSTRAP_FLAG] = bootstrapEnabled ? 'true' : 'false';
  variables[CANARY_FLAG] = 'true';
  return Object.freeze(variables);
}

function normalizeExpectedControlVariables(value) {
  const expected = object(value, 'identity bootstrap expected control variables');
  const keys = Object.keys(expected).sort();
  if (canonicalJson(keys) !== canonicalJson([...CONTROL_VARIABLES].sort())) {
    reject('identity bootstrap expected control variables are not exact');
  }
  const normalized = {};
  for (const name of CONTROL_VARIABLES) {
    if (!['true', 'false'].includes(expected[name])) reject(`${name} expected value is invalid`);
    if (PRODUCTION_SWITCHES.includes(name) && expected[name] !== 'false') {
      reject(`${name} expected value must remain false`);
    }
    if (name === CANARY_FLAG && expected[name] !== 'true') reject(`${CANARY_FLAG} expected value must remain true`);
    normalized[name] = expected[name];
  }
  return Object.freeze(normalized);
}

function normalizeExpectedIdentityVariables(value) {
  if (value === undefined || value === null) return null;
  const expected = object(value, 'identity bootstrap expected identity variables');
  const keys = Object.keys(expected).sort();
  if (canonicalJson(keys) !== canonicalJson([...IDENTITY_VARIABLES].sort())) {
    reject('identity bootstrap expected identity variables are not exact');
  }
  const nodeId = required(expected[NODE_ID_VARIABLE], NODE_ID_VARIABLE, NODE_ID);
  const ownerId = String(positiveInteger(expected[OWNER_ID_VARIABLE], OWNER_ID_VARIABLE));
  if (ownerId !== String(TRUST.owner_database_id)) {
    reject(`${OWNER_ID_VARIABLE} expected value is not trusted`);
  }
  return Object.freeze({
    [NODE_ID_VARIABLE]: nodeId,
    [OWNER_ID_VARIABLE]: ownerId,
  });
}

async function readWriterIdentityControlState(client, {
  expectedVariables,
  expectedIdentityVariables,
  trustedSha,
  nowMs,
}) {
  if (!client || typeof client.getAuthenticatedUser !== 'function' ||
      typeof client.getRepository !== 'function' || typeof client.getDefaultBranchRef !== 'function' ||
      typeof client.getRepositoryVariable !== 'function') {
    reject('identity bootstrap control state client is invalid');
  }
  const expected = normalizeExpectedControlVariables(expectedVariables);
  const expectedIdentity = normalizeExpectedIdentityVariables(expectedIdentityVariables);
  const sha = required(trustedSha, 'trusted workflow SHA', COMMIT_SHA);
  const timestamp = nowMs ?? Date.now();
  if (!Number.isFinite(timestamp) || timestamp <= 0) reject('identity bootstrap control proof time is invalid');

  const variableNames = Object.freeze([
    ...CONTROL_VARIABLES,
    ...(expectedIdentity === null ? [] : IDENTITY_VARIABLES),
  ]);

  const [authentication, repository, branchRef, ...variables] = await Promise.all([
    client.getAuthenticatedUser(timestamp),
    client.getRepository(),
    client.getDefaultBranchRef(),
    ...variableNames.map((name) => client.getRepositoryVariable(name)),
  ]);

  const authenticated = object(authentication, 'identity bootstrap control authentication');
  const user = object(authenticated.user, 'identity bootstrap control user');
  if (user.login !== TRUST.owner_login ||
      positiveInteger(user.id, 'identity bootstrap control user database id') !== TRUST.owner_database_id ||
      user.type !== TRUST.owner_type || user.site_admin !== false) {
    reject('identity bootstrap control token identity is not trusted');
  }
  if (authenticated.token_expiration !== null && typeof authenticated.token_expiration !== 'string') {
    reject('identity bootstrap control token expiration proof is invalid');
  }

  const liveRepository = object(repository, 'identity bootstrap control repository');
  const repositoryOwner = object(liveRepository.owner, 'identity bootstrap control repository owner');
  if (positiveInteger(liveRepository.id, 'identity bootstrap control repository id') !== TRUST.repository_id ||
      liveRepository.full_name !== TRUST.repository || liveRepository.name !== TRUST.repository.split('/')[1] ||
      liveRepository.default_branch !== TRUST.default_branch || repositoryOwner.login !== TRUST.owner_login ||
      positiveInteger(repositoryOwner.id, 'identity bootstrap control repository owner database id') !==
        TRUST.owner_database_id || repositoryOwner.type !== TRUST.owner_type) {
    reject('identity bootstrap control token target repository is not trusted');
  }

  const liveRef = object(branchRef, 'identity bootstrap default branch ref');
  const refObject = object(liveRef.object, 'identity bootstrap default branch ref object');
  if (liveRef.ref !== `refs/heads/${TRUST.default_branch}` || refObject.type !== 'commit' ||
      required(refObject.sha, 'identity bootstrap default branch head SHA', COMMIT_SHA) !== sha) {
    reject('identity bootstrap default branch head moved from the trusted workflow SHA');
  }

  const normalizedVariables = {};
  for (let index = 0; index < CONTROL_VARIABLES.length; index += 1) {
    const name = CONTROL_VARIABLES[index];
    const variable = object(variables[index], `${name} repository variable`);
    if (variable.name !== name || variable.value !== expected[name]) {
      reject(`${name} live value does not match the guarded closed state`);
    }
    normalizedVariables[name] = variable.value;
  }

  const normalizedIdentityVariables = {};
  if (expectedIdentity !== null) {
    for (let index = 0; index < IDENTITY_VARIABLES.length; index += 1) {
      const name = IDENTITY_VARIABLES[index];
      const variable = object(variables[CONTROL_VARIABLES.length + index], `${name} repository variable`);
      if (variable.name !== name || variable.value !== expectedIdentity[name]) {
        reject(`${name} live value does not match the bound Writer identity`);
      }
      normalizedIdentityVariables[name] = variable.value;
    }
  }

  return Object.freeze({
    control_login: TRUST.owner_login,
    control_database_id: TRUST.owner_database_id,
    control_type: TRUST.owner_type,
    control_token_expiration: authenticated.token_expiration,
    repository: TRUST.repository,
    repository_id: TRUST.repository_id,
    default_branch: TRUST.default_branch,
    default_branch_head_sha: sha,
    variables: Object.freeze(normalizedVariables),
    identity_variables: Object.freeze(normalizedIdentityVariables),
  });
}

export async function doubleReadWriterIdentityControlState(client, options) {
  const first = await readWriterIdentityControlState(client, options);
  const second = await readWriterIdentityControlState(client, options);
  const firstCanonical = canonicalJson(first);
  const secondCanonical = canonicalJson(second);
  if (firstCanonical !== secondCanonical) {
    reject('identity bootstrap control state drifted between canonical reads');
  }
  return Object.freeze({ snapshot: second, canonical: secondCanonical, sha256: sha256(secondCanonical) });
}

async function guardControlState(client, expectedVariables, trustedSha, nowMs, expectedIdentityVariables) {
  if (!client || typeof client.doubleReadTrustedState !== 'function') {
    reject('identity bootstrap guarded control client is invalid');
  }
  return client.doubleReadTrustedState({ expectedVariables, expectedIdentityVariables, trustedSha, nowMs });
}

async function guardedControlAction({ client, expectedBefore, expectedAfter = expectedBefore, trustedSha,
  nowMs, action, verify, expectedIdentityBefore, expectedIdentityAfter = expectedIdentityBefore }) {
  await guardControlState(client, expectedBefore, trustedSha, nowMs, expectedIdentityBefore);
  await action();
  if (verify) await verify();
  return guardControlState(client, expectedAfter, trustedSha, nowMs, expectedIdentityAfter);
}

function publishOutputs(environment, proof, controlProof) {
  const summary = Object.freeze({
    app_id: proof.snapshot.app_id,
    app_slug: proof.snapshot.app_slug,
    app_owner_login: proof.snapshot.app_owner_login,
    app_owner_type: proof.snapshot.app_owner_type,
    installation_id: proof.snapshot.installation_id,
    repository: proof.snapshot.repository,
    installation_repository_count: proof.snapshot.installation_repository_count,
    repository_selection: proof.snapshot.repository_selection,
    app_permissions: proof.snapshot.app_permissions,
    installation_permissions: proof.snapshot.installation_permissions,
    trusted_default_branch_head_sha: controlProof.snapshot.default_branch_head_sha,
    control_token_expiration_header:
      controlProof.snapshot.control_token_expiration ?? 'not-provided-by-github',
    control_state_sha256: controlProof.sha256,
    events: [],
    suspended: false,
  });
  if (environment.GITHUB_OUTPUT) {
    fs.appendFileSync(environment.GITHUB_OUTPUT, [
      `app_node_id=${proof.snapshot.app_node_id}`,
      `app_owner_database_id=${proof.snapshot.app_owner_database_id}`,
      `snapshot_sha256=${proof.sha256}`,
      `snapshot_summary=${canonicalJson(summary)}`,
      '',
    ].join('\n'));
  }
  return summary;
}

export async function runWriterIdentityBootstrap(environment = process.env, dependencies = {}) {
  const trustedSha = validateRuntime(environment);
  const nowMs = dependencies.nowMs ?? Date.now();
  if (!Number.isFinite(nowMs) || nowMs <= 0) reject('identity bootstrap proof time is invalid');
  const appClient = dependencies.appClient ?? new GitHubAppAttestationClient({
    jwt: createGitHubAppJwt({
      appId: TRUST.app_id,
      privateKey: environment.AERIS_WRITER_APP_PRIVATE_KEY,
      nowMs: dependencies.nowMs,
    }),
    apiUrl: environment.GITHUB_API_URL,
    fetchImpl: dependencies.appFetchImpl,
    timeoutMs: dependencies.timeoutMs,
  });
  const identityProof = await doubleReadWriterIdentity(appClient);
  if (typeof appClient.createReadOnlyInstallationInventoryToken !== 'function') {
    reject('identity bootstrap App client cannot mint a read-only inventory token');
  }
  const inventoryToken = normalizeReadOnlyInventoryToken(
    await appClient.createReadOnlyInstallationInventoryToken(TRUST.installation_id),
    nowMs,
  );
  const inventoryClient = dependencies.inventoryClientFactory
    ? dependencies.inventoryClientFactory(inventoryToken.token)
    : new GitHubInstallationTokenProofClient({
      token: inventoryToken.token,
      apiUrl: environment.GITHUB_API_URL,
      fetchImpl: dependencies.installationFetchImpl,
      timeoutMs: dependencies.timeoutMs,
    });
  const inventoryProof = await doubleReadInstallationRepositoryInventory(inventoryClient);
  const snapshot = Object.freeze({
    ...identityProof.snapshot,
    installation_repository_count: inventoryProof.snapshot.total_count,
  });
  const snapshotCanonical = canonicalJson(snapshot);
  const proof = Object.freeze({ snapshot, canonical: snapshotCanonical, sha256: sha256(snapshotCanonical) });
  const controlClient = dependencies.controlClient ?? new WriterIdentityBootstrapControlClient({
    token: environment.AERIS_IDENTITY_BOOTSTRAP_TOKEN,
    apiUrl: environment.GITHUB_API_URL,
    fetchImpl: dependencies.controlFetchImpl,
    timeoutMs: dependencies.timeoutMs,
  });
  const enabledState = expectedControlVariables(true);
  const disabledState = expectedControlVariables(false);
  await guardControlState(controlClient, enabledState, trustedSha, nowMs);

  await guardedControlAction({
    client: controlClient,
    expectedBefore: enabledState,
    trustedSha,
    nowMs,
    action: () => controlClient.upsertRepositoryVariable(NODE_ID_VARIABLE, proof.snapshot.app_node_id),
    verify: () => controlClient.assertRepositoryVariable(NODE_ID_VARIABLE, proof.snapshot.app_node_id),
  });
  await guardedControlAction({
    client: controlClient,
    expectedBefore: enabledState,
    trustedSha,
    nowMs,
    action: () => controlClient.upsertRepositoryVariable(
      OWNER_ID_VARIABLE,
      String(proof.snapshot.app_owner_database_id),
    ),
    verify: () => controlClient.assertRepositoryVariable(
      OWNER_ID_VARIABLE,
      String(proof.snapshot.app_owner_database_id),
    ),
  });
  await guardedControlAction({
    client: controlClient,
    expectedBefore: enabledState,
    expectedIdentityBefore: Object.freeze({
      [NODE_ID_VARIABLE]: proof.snapshot.app_node_id,
      [OWNER_ID_VARIABLE]: String(proof.snapshot.app_owner_database_id),
    }),
    expectedAfter: disabledState,
    trustedSha,
    nowMs,
    action: () => controlClient.updateExistingRepositoryVariable(BOOTSTRAP_FLAG, 'false'),
  });
  const expectedIdentityVariables = Object.freeze({
    [NODE_ID_VARIABLE]: proof.snapshot.app_node_id,
    [OWNER_ID_VARIABLE]: String(proof.snapshot.app_owner_database_id),
  });
  const finalControlProof = await guardControlState(
    controlClient,
    disabledState,
    trustedSha,
    nowMs,
    expectedIdentityVariables,
  );
  publishOutputs(environment, proof, finalControlProof);
  return Object.freeze({
    app_node_id: proof.snapshot.app_node_id,
    app_owner_database_id: proof.snapshot.app_owner_database_id,
    snapshot_sha256: proof.sha256,
    installation_repository_count: proof.snapshot.installation_repository_count,
    final_control_state_sha256: finalControlProof.sha256,
    bootstrap_disabled: true,
    identity_variables_verified: true,
  });
}

function isMain() {
  return process.argv[1] && pathToFileURL(path.resolve(process.argv[1])).href === import.meta.url;
}

if (isMain()) {
  runWriterIdentityBootstrap().catch((error) => {
    console.error(`Writer identity bootstrap failed: ${error.message}`);
    process.exitCode = 1;
  });
}

export const WRITER_IDENTITY_BOOTSTRAP_TRUST = TRUST;
