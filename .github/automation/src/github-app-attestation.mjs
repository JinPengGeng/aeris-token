import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const APP_SLUG = /^[a-z0-9][a-z0-9-]{0,99}$/;
const ACCOUNT_TYPES = new Set(['User', 'Organization']);
const REQUIRED_WRITER_PERMISSIONS = Object.freeze({
  administration: 'read',
  checks: 'write',
  contents: 'write',
  pull_requests: 'write',
});
const IMPLICIT_WRITER_PERMISSIONS = Object.freeze({ metadata: 'read' });
const MAXIMUM_PROOF_BYTES = 1024 * 1024;

export class GitHubAppAttestationError extends Error {
  constructor(message, status = null) {
    super(message);
    this.name = 'GitHubAppAttestationError';
    this.status = status;
  }
}

function reject(message, status = null) {
  throw new GitHubAppAttestationError(message, status);
}

function positiveInteger(value, name) {
  const parsed = typeof value === 'string' && /^[1-9][0-9]*$/.test(value) ? Number(value) : value;
  if (!Number.isSafeInteger(parsed) || parsed <= 0) reject(`${name} must be a positive integer`);
  return parsed;
}

function required(value, name, pattern = null) {
  if (typeof value !== 'string' || value.length === 0 || /[\u0000-\u001f\u007f]/.test(value) ||
      (pattern && !pattern.test(value))) {
    reject(`${name} is invalid`);
  }
  return value;
}

function multiline(value, name) {
  if (typeof value !== 'string' || value.length === 0 || /[\u0000\u007f]/.test(value)) reject(`${name} is invalid`);
  return value;
}

function object(value, name) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) reject(`${name} is invalid`);
  return value;
}

function account(value, name) {
  const candidate = object(value, name);
  const login = required(candidate.login, `${name} login`);
  const type = required(candidate.type, `${name} type`);
  if (!ACCOUNT_TYPES.has(type)) reject(`${name} type is invalid`);
  return Object.freeze({ login, type });
}

function writerPermissions(value, name) {
  const permissions = object(value, name);
  const allowed = new Set([...Object.keys(REQUIRED_WRITER_PERMISSIONS), ...Object.keys(IMPLICIT_WRITER_PERMISSIONS)]);
  for (const key of Object.keys(permissions)) {
    if (!allowed.has(key)) reject(`${name} contains an unapproved permission: ${key}`);
    if (typeof permissions[key] !== 'string' || !['read', 'write', 'admin'].includes(permissions[key])) {
      reject(`${name}.${key} is invalid`);
    }
  }
  const normalized = {};
  for (const [key, expected] of Object.entries(REQUIRED_WRITER_PERMISSIONS)) {
    if (permissions[key] !== expected) reject(`${name}.${key} must be ${expected}`);
    normalized[key] = expected;
  }
  if (permissions.metadata !== undefined && permissions.metadata !== IMPLICIT_WRITER_PERMISSIONS.metadata) {
    reject(`${name}.metadata must be read`);
  }
  normalized.metadata = IMPLICIT_WRITER_PERMISSIONS.metadata;
  return Object.freeze(normalized);
}

export function validateWriterPermissions(value, name = 'Writer permissions') {
  let parsed = value;
  if (typeof value === 'string') {
    try { parsed = JSON.parse(value); } catch { reject(`${name} is not valid JSON`); }
  }
  return writerPermissions(parsed, name);
}

export function createGitHubAppJwt({ appId, privateKey, nowMs = Date.now() }) {
  const issuer = positiveInteger(appId, 'Writer App id');
  const pem = multiline(privateKey, 'Writer App private key');
  const now = Math.floor(nowMs / 1000);
  if (!Number.isSafeInteger(now) || now <= 60) reject('JWT clock is invalid');

  let key;
  try {
    key = crypto.createPrivateKey(pem);
  } catch {
    reject('Writer App private key is invalid');
  }
  if (key.asymmetricKeyType !== 'rsa') reject('Writer App private key must be RSA');

  const encode = (value) => Buffer.from(JSON.stringify(value)).toString('base64url');
  const header = encode({ alg: 'RS256', typ: 'JWT' });
  const payload = encode({ iat: now - 60, exp: now + 540, iss: String(issuer) });
  const signingInput = `${header}.${payload}`;
  let signature;
  try {
    signature = crypto.sign('RSA-SHA256', Buffer.from(signingInput), key).toString('base64url');
  } catch {
    reject('Writer App JWT signing failed');
  }
  return `${signingInput}.${signature}`;
}

export class GitHubAppAttestationClient {
  constructor({ jwt, apiUrl = 'https://api.github.com', fetchImpl = globalThis.fetch, timeoutMs = 15_000 }) {
    this.jwt = required(jwt, 'Writer App JWT');
    let parsed;
    try { parsed = new URL(apiUrl); } catch { reject('GitHub API URL is invalid'); }
    if (parsed.protocol !== 'https:' || parsed.username || parsed.password || parsed.search || parsed.hash) {
      reject('GitHub API URL must be a credential-free HTTPS URL');
    }
    if (typeof fetchImpl !== 'function') reject('GitHub fetch implementation is invalid');
    if (!Number.isSafeInteger(timeoutMs) || timeoutMs <= 0 || timeoutMs > 60_000) {
      reject('GitHub App attestation timeout is invalid');
    }
    this.apiUrl = parsed.toString().replace(/\/$/, '');
    this.fetchImpl = fetchImpl;
    this.timeoutMs = timeoutMs;
  }

  async request(pathname, { method = 'GET', body, expectedStatuses = [200] } = {}) {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), this.timeoutMs);
    try {
      const response = await this.fetchImpl(`${this.apiUrl}${pathname}`, {
        method,
        headers: {
          accept: 'application/vnd.github+json',
          authorization: `Bearer ${this.jwt}`,
          ...(body === undefined ? {} : { 'content-type': 'application/json' }),
          'x-github-api-version': '2022-11-28',
        },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
        signal: controller.signal,
      });
      if (!response || typeof response.status !== 'number' || typeof response.text !== 'function') {
        reject('GitHub App attestation response is invalid');
      }
      let text;
      try { text = await response.text(); } catch { reject('GitHub App attestation response body failed'); }
      if (Buffer.byteLength(text, 'utf8') > MAXIMUM_PROOF_BYTES) {
        reject('GitHub App attestation response is too large');
      }
      let value;
      try { value = JSON.parse(text); } catch { reject('GitHub App attestation returned invalid JSON', response.status); }
      if (!expectedStatuses.includes(response.status)) {
        reject(`GitHub App attestation returned HTTP ${response.status}`, response.status);
      }
      return object(value, 'GitHub App attestation response');
    } catch (error) {
      if (error instanceof GitHubAppAttestationError) throw error;
      reject('GitHub App attestation request failed');
    } finally {
      clearTimeout(timeout);
    }
  }

  getApp() { return this.request('/app'); }

  getInstallation(installationId) {
    return this.request(`/app/installations/${positiveInteger(installationId, 'Writer installation id')}`);
  }

  createReadOnlyInstallationInventoryToken(installationId) {
    return this.request(
      `/app/installations/${positiveInteger(installationId, 'Writer installation id')}/access_tokens`,
      {
        method: 'POST',
        body: { permissions: { contents: 'read' } },
        expectedStatuses: [201],
      },
    );
  }
}

export class GitHubInstallationTokenProofClient {
  constructor({ token, apiUrl = 'https://api.github.com', fetchImpl = globalThis.fetch, timeoutMs = 15_000 }) {
    this.token = required(token, 'Writer installation token');
    let parsed;
    try { parsed = new URL(apiUrl); } catch { reject('GitHub API URL is invalid'); }
    if (parsed.protocol !== 'https:' || parsed.username || parsed.password || parsed.search || parsed.hash) {
      reject('GitHub API URL must be a credential-free HTTPS URL');
    }
    if (typeof fetchImpl !== 'function') reject('GitHub fetch implementation is invalid');
    if (!Number.isSafeInteger(timeoutMs) || timeoutMs <= 0 || timeoutMs > 60_000) {
      reject('GitHub installation token proof timeout is invalid');
    }
    this.apiUrl = parsed.toString().replace(/\/$/, '');
    this.fetchImpl = fetchImpl;
    this.timeoutMs = timeoutMs;
  }

  async request(pathname, { method = 'GET', body } = {}) {
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
        reject('GitHub installation token proof response is invalid');
      }
      let text;
      try { text = await response.text(); } catch { reject('GitHub installation token proof response body failed'); }
      if (Buffer.byteLength(text, 'utf8') > MAXIMUM_PROOF_BYTES) {
        reject('GitHub installation token proof response is too large');
      }
      let value;
      try { value = JSON.parse(text); } catch {
        reject('GitHub installation token proof returned invalid JSON', response.status);
      }
      if (!response.ok) reject(`GitHub installation token proof returned HTTP ${response.status}`, response.status);
      return object(value, 'GitHub installation token proof response');
    } catch (error) {
      if (error instanceof GitHubAppAttestationError) throw error;
      reject('GitHub installation token proof request failed');
    } finally {
      clearTimeout(timeout);
    }
  }

  getInstallationRepositories() {
    return this.request('/installation/repositories?per_page=2&page=1');
  }

  async getCompleteInstallationRepositoryInventory() {
    const repositories = [];
    let totalCount = null;
    for (let page = 1; page <= 100; page += 1) {
      const response = await this.request(`/installation/repositories?per_page=100&page=${page}`);
      if (!Number.isSafeInteger(response.total_count) || response.total_count < 0 ||
          response.total_count > 10_000 || !Array.isArray(response.repositories) ||
          response.repositories.length > 100) {
        reject('GitHub installation repository inventory page is invalid');
      }
      if (totalCount === null) totalCount = response.total_count;
      if (response.total_count !== totalCount) {
        reject('GitHub installation repository inventory total drifted during pagination');
      }
      repositories.push(...response.repositories);
      if (repositories.length === totalCount) {
        return Object.freeze({ total_count: totalCount, repositories: Object.freeze(repositories) });
      }
      if (repositories.length > totalCount || response.repositories.length === 0) {
        reject('GitHub installation repository inventory pagination is inconsistent');
      }
    }
    reject('GitHub installation repository inventory exceeds the bounded pagination limit');
  }

  getBot(login) {
    return this.request(`/users/${encodeURIComponent(required(login, 'Writer Bot login'))}`);
  }

  async getGraphQlBot(nodeId) {
    const value = await this.request('/graphql', {
      method: 'POST',
      body: {
        query: `query WriterTokenBot($id: ID!) {
          node(id: $id) {
            __typename
            ... on Bot { id databaseId login }
          }
        }`,
        variables: { id: required(nodeId, 'Writer Bot REST node id') },
      },
    });
    if (Array.isArray(value.errors) && value.errors.length > 0) {
      reject('GitHub installation token GraphQL proof returned errors');
    }
    return object(object(value.data, 'GitHub installation token GraphQL data').node, 'Writer GraphQL Bot');
  }
}

export function validateGitHubAppAttestation({ app, installation, expected }) {
  const trusted = object(expected, 'Writer App attestation expectation');
  const ownerLogin = required(trusted.owner_login, 'Writer App owner login');
  const ownerDatabaseId = positiveInteger(trusted.owner_database_id, 'Writer App owner database id');
  const appId = positiveInteger(trusted.app_id, 'Writer App id');
  const appSlug = required(trusted.app_slug, 'Writer App slug', APP_SLUG);
  const appNodeId = required(trusted.app_node_id, 'Writer App node id');
  const installationId = positiveInteger(trusted.installation_id, 'Writer installation id');

  const liveApp = object(app, 'authenticated Writer App');
  const appOwner = account(liveApp.owner, 'authenticated Writer App owner');
  const appOwnerDatabaseId = positiveInteger(liveApp.owner.id, 'authenticated Writer App owner database id');
  const appPermissions = writerPermissions(liveApp.permissions, 'authenticated Writer App permissions');
  if (positiveInteger(liveApp.id, 'authenticated Writer App id') !== appId ||
      required(liveApp.slug, 'authenticated Writer App slug', APP_SLUG) !== appSlug ||
      required(liveApp.node_id, 'authenticated Writer App node id') !== appNodeId ||
      appOwner.login !== ownerLogin || appOwnerDatabaseId !== ownerDatabaseId) {
    reject('authenticated Writer App identity does not match configuration');
  }

  const liveInstallation = object(installation, 'authenticated Writer App installation');
  const installationAccount = account(liveInstallation.account, 'authenticated Writer App installation account');
  const installationPermissions = writerPermissions(
    liveInstallation.permissions,
    'authenticated Writer App installation permissions',
  );
  if (positiveInteger(liveInstallation.id, 'authenticated Writer installation id') !== installationId ||
      positiveInteger(liveInstallation.app_id, 'authenticated Writer installation App id') !== appId ||
      required(liveInstallation.app_slug, 'authenticated Writer installation App slug', APP_SLUG) !== appSlug ||
      installationAccount.login !== ownerLogin || installationAccount.type !== appOwner.type ||
      liveInstallation.repository_selection !== 'selected' || liveInstallation.suspended_at !== null) {
    reject('authenticated Writer App installation identity does not match configuration');
  }

  return Object.freeze({
    app_id: appId,
    app_slug: appSlug,
    app_node_id: appNodeId,
    app_owner_login: appOwner.login,
    app_owner_database_id: appOwnerDatabaseId,
    app_owner_type: appOwner.type,
    installation_id: installationId,
    installation_account_login: installationAccount.login,
    installation_account_type: installationAccount.type,
    repository_selection: 'selected',
    app_permissions: appPermissions,
    installation_permissions: installationPermissions,
  });
}

export async function attestGitHubApp({ client, expected }) {
  const installationId = positiveInteger(
    object(expected, 'Writer App attestation expectation').installation_id,
    'Writer installation id',
  );
  const app = await client.getApp();
  const installation = await client.getInstallation(installationId);
  return validateGitHubAppAttestation({ app, installation, expected });
}

export function validateGitHubInstallationTokenProof({ repositories, bot, graphQlBot: graphQlIdentity, expected }) {
  const trusted = object(expected, 'Writer token proof expectation');
  const repositoryName = required(trusted.repository, 'Writer repository', REPOSITORY);
  const repositoryId = positiveInteger(trusted.repository_id, 'Writer repository id');
  const ownerLogin = repositoryName.split('/')[0];
  const appSlug = required(trusted.app_slug, 'Writer App slug', APP_SLUG);
  const installationId = positiveInteger(trusted.installation_id, 'Writer installation id');

  const accessible = object(repositories, 'Writer installation repositories');
  if (accessible.total_count !== 1 || !Array.isArray(accessible.repositories) || accessible.repositories.length !== 1) {
    reject('Writer installation token repository scope is not exact');
  }
  const repository = object(accessible.repositories[0], 'Writer installation repository');
  if (positiveInteger(repository.id, 'Writer installation repository id') !== repositoryId ||
      required(repository.full_name, 'Writer installation repository full_name', REPOSITORY) !== repositoryName ||
      required(object(repository.owner, 'Writer installation repository owner').login, 'Writer repository owner login') !== ownerLogin) {
    reject('Writer installation token repository identity is invalid');
  }

  const restBot = object(bot, 'Writer Bot REST identity');
  const expectedRestLogin = `${appSlug}[bot]`;
  if (restBot.type !== 'Bot' || required(restBot.login, 'Writer Bot REST login') !== expectedRestLogin ||
      restBot.site_admin !== false) {
    reject('Writer Bot REST identity is invalid');
  }
  const databaseId = positiveInteger(restBot.id, 'Writer Bot REST database id');
  const nodeId = required(restBot.node_id, 'Writer Bot REST node id');

  const graphQlBot = object(graphQlIdentity, 'Writer Bot GraphQL identity');
  if (graphQlBot.__typename !== 'Bot' || required(graphQlBot.login, 'Writer Bot GraphQL login') !== appSlug ||
      positiveInteger(graphQlBot.databaseId, 'Writer Bot GraphQL database id') !== databaseId ||
      required(graphQlBot.id, 'Writer Bot GraphQL node id') !== nodeId) {
    reject('Writer Bot GraphQL identity is invalid');
  }

  return Object.freeze({
    app_slug: appSlug,
    installation_id: installationId,
    repository: repositoryName,
    repository_id: repositoryId,
    rest_login: expectedRestLogin,
    graphql_login: appSlug,
    bot_database_id: databaseId,
    bot_node_id: nodeId,
  });
}

export async function proveGitHubInstallationToken({ client, expected }) {
  const appSlug = required(object(expected, 'Writer token proof expectation').app_slug, 'Writer App slug', APP_SLUG);
  const [repositories, bot] = await Promise.all([
    client.getInstallationRepositories(),
    client.getBot(`${appSlug}[bot]`),
  ]);
  const graphQlBot = await client.getGraphQlBot(required(object(bot, 'Writer Bot REST identity').node_id, 'Writer Bot REST node id'));
  return validateGitHubInstallationTokenProof({ repositories, bot, graphQlBot, expected });
}

export async function runGitHubAppAttestation(environment = process.env, dependencies = {}) {
  const repository = required(environment.GITHUB_REPOSITORY, 'GITHUB_REPOSITORY', REPOSITORY);
  const expected = Object.freeze({
    owner_login: repository.split('/')[0],
    owner_database_id: positiveInteger(environment.AERIS_WRITER_APP_OWNER_DATABASE_ID, 'AERIS_WRITER_APP_OWNER_DATABASE_ID'),
    app_id: positiveInteger(environment.AERIS_WRITER_APP_ID, 'AERIS_WRITER_APP_ID'),
    app_slug: required(environment.AERIS_WRITER_APP_SLUG, 'AERIS_WRITER_APP_SLUG', APP_SLUG),
    app_node_id: required(environment.AERIS_WRITER_APP_NODE_ID, 'AERIS_WRITER_APP_NODE_ID'),
    installation_id: positiveInteger(environment.AERIS_WRITER_INSTALLATION_ID, 'AERIS_WRITER_INSTALLATION_ID'),
  });
  const client = dependencies.client ?? new GitHubAppAttestationClient({
    jwt: createGitHubAppJwt({
      appId: expected.app_id,
      privateKey: environment.AERIS_WRITER_APP_PRIVATE_KEY,
      nowMs: dependencies.nowMs,
    }),
    apiUrl: environment.GITHUB_API_URL,
    fetchImpl: dependencies.fetchImpl,
    timeoutMs: dependencies.timeoutMs,
  });
  const result = await attestGitHubApp({ client, expected });
  if (environment.GITHUB_OUTPUT) {
    fs.appendFileSync(environment.GITHUB_OUTPUT, [
      `app_id=${result.app_id}`,
      `app_slug=${result.app_slug}`,
      `app_node_id=${result.app_node_id}`,
      `app_owner_login=${result.app_owner_login}`,
      `app_owner_database_id=${result.app_owner_database_id}`,
      `app_owner_type=${result.app_owner_type}`,
      `app_permissions=${JSON.stringify(result.app_permissions)}`,
      `installation_id=${result.installation_id}`,
      `installation_account_login=${result.installation_account_login}`,
      `installation_account_type=${result.installation_account_type}`,
      `repository_selection=${result.repository_selection}`,
      `installation_permissions=${JSON.stringify(result.installation_permissions)}`,
      '',
    ].join('\n'));
  }
  return result;
}

export async function runGitHubInstallationTokenProof(environment = process.env, dependencies = {}) {
  const repository = required(environment.GITHUB_REPOSITORY, 'GITHUB_REPOSITORY', REPOSITORY);
  const appSlug = required(environment.AERIS_WRITER_APP_SLUG, 'AERIS_WRITER_APP_SLUG', APP_SLUG);
  const installationId = positiveInteger(environment.AERIS_WRITER_INSTALLATION_ID, 'AERIS_WRITER_INSTALLATION_ID');
  if (required(environment.AERIS_WRITER_TOKEN_APP_SLUG, 'AERIS_WRITER_TOKEN_APP_SLUG', APP_SLUG) !== appSlug ||
      positiveInteger(environment.AERIS_WRITER_TOKEN_INSTALLATION_ID, 'AERIS_WRITER_TOKEN_INSTALLATION_ID') !== installationId) {
    reject('Writer installation token metadata does not match the configured App installation');
  }
  const expected = Object.freeze({
    repository,
    repository_id: positiveInteger(environment.GITHUB_REPOSITORY_ID, 'GITHUB_REPOSITORY_ID'),
    app_slug: appSlug,
    installation_id: installationId,
  });
  const client = dependencies.client ?? new GitHubInstallationTokenProofClient({
    token: environment.AERIS_WRITER_TOKEN,
    apiUrl: environment.GITHUB_API_URL,
    fetchImpl: dependencies.fetchImpl,
    timeoutMs: dependencies.timeoutMs,
  });
  const result = await proveGitHubInstallationToken({ client, expected });
  if (environment.GITHUB_OUTPUT) {
    fs.appendFileSync(environment.GITHUB_OUTPUT, [
      `repository=${result.repository}`,
      `repository_id=${result.repository_id}`,
      `app_slug=${result.app_slug}`,
      `installation_id=${result.installation_id}`,
      `rest_login=${result.rest_login}`,
      `graphql_login=${result.graphql_login}`,
      `bot_database_id=${result.bot_database_id}`,
      `bot_node_id=${result.bot_node_id}`,
      '',
    ].join('\n'));
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  if (process.argv[2] === undefined) await runGitHubAppAttestation();
  else if (process.argv[2] === 'prove-token') await runGitHubInstallationTokenProof();
  else reject('GitHub App attestation command is invalid');
}
