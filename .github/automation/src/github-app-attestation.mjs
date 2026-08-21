import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const APP_SLUG = /^[a-z0-9][a-z0-9-]{0,99}$/;
const ACCOUNT_TYPES = new Set(['User', 'Organization']);

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

  async request(pathname) {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), this.timeoutMs);
    try {
      const response = await this.fetchImpl(`${this.apiUrl}${pathname}`, {
        method: 'GET',
        headers: {
          accept: 'application/vnd.github+json',
          authorization: `Bearer ${this.jwt}`,
          'x-github-api-version': '2022-11-28',
        },
        signal: controller.signal,
      });
      if (!response || typeof response.status !== 'number' || typeof response.text !== 'function') {
        reject('GitHub App attestation response is invalid');
      }
      let text;
      try { text = await response.text(); } catch { reject('GitHub App attestation response body failed'); }
      let value;
      try { value = JSON.parse(text); } catch { reject('GitHub App attestation returned invalid JSON', response.status); }
      if (!response.ok) reject(`GitHub App attestation returned HTTP ${response.status}`, response.status);
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
}

export function validateGitHubAppAttestation({ app, installation, expected }) {
  const trusted = object(expected, 'Writer App attestation expectation');
  const ownerLogin = required(trusted.owner_login, 'Writer App owner login');
  const appId = positiveInteger(trusted.app_id, 'Writer App id');
  const appSlug = required(trusted.app_slug, 'Writer App slug', APP_SLUG);
  const installationId = positiveInteger(trusted.installation_id, 'Writer installation id');

  const liveApp = object(app, 'authenticated Writer App');
  const appOwner = account(liveApp.owner, 'authenticated Writer App owner');
  if (positiveInteger(liveApp.id, 'authenticated Writer App id') !== appId ||
      required(liveApp.slug, 'authenticated Writer App slug', APP_SLUG) !== appSlug ||
      appOwner.login !== ownerLogin) {
    reject('authenticated Writer App identity does not match configuration');
  }

  const liveInstallation = object(installation, 'authenticated Writer App installation');
  const installationAccount = account(liveInstallation.account, 'authenticated Writer App installation account');
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
    app_owner_login: appOwner.login,
    app_owner_type: appOwner.type,
    installation_id: installationId,
    installation_account_login: installationAccount.login,
    installation_account_type: installationAccount.type,
    repository_selection: 'selected',
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

export async function runGitHubAppAttestation(environment = process.env, dependencies = {}) {
  const repository = required(environment.GITHUB_REPOSITORY, 'GITHUB_REPOSITORY', REPOSITORY);
  const expected = Object.freeze({
    owner_login: repository.split('/')[0],
    app_id: positiveInteger(environment.AERIS_WRITER_APP_ID, 'AERIS_WRITER_APP_ID'),
    app_slug: required(environment.AERIS_WRITER_APP_SLUG, 'AERIS_WRITER_APP_SLUG', APP_SLUG),
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
      `app_owner_login=${result.app_owner_login}`,
      `app_owner_type=${result.app_owner_type}`,
      `installation_id=${result.installation_id}`,
      `installation_account_login=${result.installation_account_login}`,
      `installation_account_type=${result.installation_account_type}`,
      `repository_selection=${result.repository_selection}`,
      '',
    ].join('\n'));
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  await runGitHubAppAttestation();
}
