import { randomUUID } from 'node:crypto';
import { execFile } from 'node:child_process';
import path from 'node:path';
import { promisify } from 'node:util';

import {
  hasCanonicalWriterOwnershipMarker,
  writerPullLifecycleAttestation,
  WRITER_OWNERSHIP_MARKER,
} from './writer-lifecycle.mjs';

const API_ORIGIN = 'https://api.github.com';
const MAX_RESPONSE_BYTES = 1_048_576;
// The body limit applies after internal create-attempt and ownership markers are appended.
const MAX_BODY_BYTES = 65_536;
const MAX_METADATA_BYTES = 524_288;
const MAX_TOKEN_LENGTH = 8_192;
const WRITER_CREATE_ATTEMPT_PREFIX = '<!-- aeris-writer-create-attempt:';
const WRITER_INSTALLATION_PERMISSIONS = Object.freeze({
  contents: 'write',
  pull_requests: 'write',
});
const MAX_PAGES = 3;
const PAGE_SIZE = 100;
const DEFAULT_TOTAL_TIMEOUT_MS = 30_000;
const DEFAULT_HEADERS_TIMEOUT_MS = 10_000;
const DEFAULT_BODY_TIMEOUT_MS = 15_000;
const MAX_INSTALLATION_TOKEN_LIFETIME_MS = 70 * 60 * 1_000;
const execFileAsync = promisify(execFile);

const OWNER_PATTERN = /^[A-Za-z\d](?:[A-Za-z\d-]{0,37}[A-Za-z\d])?$/;
const REPOSITORY_NAME_PATTERN = /^[A-Za-z\d_.-]+$/;
const USERNAME_PATTERN = /^[A-Za-z\d](?:[A-Za-z\d-]{0,37}[A-Za-z\d])?$/;
const APP_SLUG_PATTERN = /^[a-z\d](?:[a-z\d-]{0,98}[a-z\d])?$/;
const APP_JWT_PATTERN = /^[A-Za-z\d_-]+\.[A-Za-z\d_-]+\.[A-Za-z\d_-]+$/;
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const SHA_PATTERN = /^[0-9a-f]{40}$/;
const GITHUB_TIMESTAMP_PATTERN = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?Z$/;

export class WriterGitHubApiError extends Error {
  constructor(message, status = null) {
    super(message);
    this.name = 'WriterGitHubApiError';
    this.status = status;
  }
}

function fail(message) {
  throw new WriterGitHubApiError(message);
}

function positiveNumber(value, name) {
  if (!Number.isSafeInteger(value) || value < 1 || value > 2_147_483_647) {
    fail(`${name} must be a positive GitHub number`);
  }
  return value;
}

function positiveId(value, name) {
  if (!Number.isSafeInteger(value) || value < 1) fail(`${name} must be a positive GitHub ID`);
  return value;
}

function validateSha(value, name) {
  if (typeof value !== 'string' || !SHA_PATTERN.test(value)) fail(`${name} must be a lowercase 40-character SHA`);
  return value;
}

function validGitHubTimestamp(value) {
  if (typeof value !== 'string' || !GITHUB_TIMESTAMP_PATTERN.test(value)) return false;
  const milliseconds = Date.parse(value);
  if (!Number.isFinite(milliseconds)) return false;
  return new Date(milliseconds).toISOString().slice(0, 19) === value.slice(0, 19);
}

function agentRef(issueNumber) {
  return `agent/issue-${positiveNumber(issueNumber, 'Issue number')}`;
}

export function writerRefPushArguments(issueNumber, expectedOldSha, newSha) {
  const ref = `refs/heads/${agentRef(issueNumber)}`;
  const verifiedNewSha = validateSha(newSha, 'New ref SHA');
  const verifiedOldSha = expectedOldSha === null ? '' : validateSha(expectedOldSha, 'Expected old ref SHA');
  if (verifiedOldSha === verifiedNewSha) fail('New ref SHA must differ from the expected old SHA');
  return [
    'push', '--porcelain', `--force-with-lease=${ref}:${verifiedOldSha}`, 'origin',
    `${verifiedNewSha}:${ref}`,
  ];
}

function validatedAgentRef(value) {
  if (typeof value !== 'string' || !/^agent\/issue-[1-9]\d*$/.test(value)) fail('Writer ref is invalid');
  const issueNumber = Number(value.slice('agent/issue-'.length));
  if (!Number.isSafeInteger(issueNumber) || issueNumber > 2_147_483_647) fail('Writer ref is invalid');
  return value;
}

function validateRepository(value) {
  if (typeof value !== 'string') fail('GitHub repository identifier is invalid');
  const components = value.split('/');
  if (components.length !== 2) fail('GitHub repository identifier is invalid');
  const [owner, name] = components;
  if (!OWNER_PATTERN.test(owner) || owner.includes('--')) fail('GitHub repository owner is invalid');
  if (!REPOSITORY_NAME_PATTERN.test(name) || name.length > 100 || name === '.' || name === '..') {
    fail('GitHub repository name is invalid');
  }
  return value;
}

function validateWriterApp(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) fail('Writer App identity is invalid');
  if (Object.keys(value).some((key) => key !== 'id' && key !== 'slug') || Object.keys(value).length !== 2) {
    fail('Writer App identity must contain only id and slug');
  }
  const id = positiveId(value.id, 'Writer App ID');
  if (typeof value.slug !== 'string' || !APP_SLUG_PATTERN.test(value.slug)) fail('Writer App slug is invalid');
  return Object.freeze({ id, slug: value.slug });
}

function validateText(value, name, { maximumBytes, required = false, singleLine = false } = {}) {
  if (typeof value !== 'string') fail(`${name} must be a string`);
  if (required && value.trim().length === 0) fail(`${name} must not be empty`);
  if (singleLine && /[\u0000-\u001f\u007f]/.test(value)) fail(`${name} must be a single line`);
  if (Buffer.byteLength(value, 'utf8') > maximumBytes) fail(`${name} exceeds the configured limit`);
  return value;
}

function validateAppJwt(value) {
  validateText(value, 'Writer App JWT', { maximumBytes: MAX_TOKEN_LENGTH, required: true, singleLine: true });
  if (!APP_JWT_PATTERN.test(value)) fail('Writer App JWT is invalid');
  return value;
}

function validateCreateAttemptId(value) {
  if (typeof value !== 'string' || !UUID_PATTERN.test(value)) fail('Writer create attempt ID is invalid');
  return value;
}

function validateInstallationPermissions(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) fail('Writer installation permissions are invalid');
  const keys = Object.keys(value);
  if (
    keys.some((key) => key !== 'contents' && key !== 'pull_requests' && key !== 'metadata') ||
    value.contents !== 'write' || value.pull_requests !== 'write' ||
    (Object.hasOwn(value, 'metadata') && value.metadata !== 'read')
  ) fail('Writer installation permissions exceed the configured boundary');
  return value;
}

function createAttemptMarker(value) {
  return `${WRITER_CREATE_ATTEMPT_PREFIX}${validateCreateAttemptId(value)} -->`;
}

function markedBody(value, createAttemptId = null) {
  if (value.includes(WRITER_OWNERSHIP_MARKER)) fail('Pull request body contains the reserved Writer ownership marker');
  if (value.includes(WRITER_CREATE_ATTEMPT_PREFIX)) fail('Pull request body contains the reserved Writer create-attempt marker');
  const suffix = createAttemptId === null
    ? WRITER_OWNERSHIP_MARKER
    : `${createAttemptMarker(createAttemptId)}\n\n${WRITER_OWNERSHIP_MARKER}`;
  const body = value.length === 0 ? suffix : `${value}\n\n${suffix}`;
  return validateText(body, 'Pull request body', { maximumBytes: MAX_BODY_BYTES });
}

function validateMetadata(metadata, { requireBoth = false, createAttemptId = null } = {}) {
  if (!metadata || typeof metadata !== 'object' || Array.isArray(metadata)) fail('Pull request metadata must be an object');
  const keys = Object.keys(metadata);
  if (keys.some((key) => key !== 'title' && key !== 'body')) fail('Pull request metadata contains unsupported fields');
  if (keys.length === 0 || (requireBoth && (keys.length !== 2 || !Object.hasOwn(metadata, 'title') || !Object.hasOwn(metadata, 'body')))) {
    fail('Pull request metadata must include title and body');
  }

  const result = {};
  if (Object.hasOwn(metadata, 'title')) {
    result.title = validateText(metadata.title, 'Pull request title', {
      maximumBytes: 256,
      required: true,
      singleLine: true,
    });
  }
  if (Object.hasOwn(metadata, 'body')) {
    result.body = markedBody(validateText(metadata.body, 'Pull request body', {
      maximumBytes: MAX_BODY_BYTES,
    }), createAttemptId);
  }
  if (Buffer.byteLength(JSON.stringify(result), 'utf8') > MAX_METADATA_BYTES) {
    fail('Pull request metadata exceeds the configured limit');
  }
  return result;
}

function boundedTimeout(value, name, fallback) {
  const candidate = value ?? fallback;
  if (!Number.isSafeInteger(candidate) || candidate < 1 || candidate > 120_000) {
    fail(`${name} must be a positive bounded timeout`);
  }
  return candidate;
}

function deadline(promise, milliseconds, message, controller = null) {
  let timer;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => {
      controller?.abort();
      reject(new WriterGitHubApiError(message));
    }, milliseconds);
  });
  return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
}

function isAmbiguousMutationError(error) {
  if (!(error instanceof WriterGitHubApiError)) return true;
  const { status } = error;
  if (!Number.isInteger(status)) return true;
  return status === 408 || (status >= 200 && status <= 299) || (status >= 500 && status <= 599);
}

async function requireMutationBoundary(callback) {
  if (typeof callback !== 'function') return;
  const decision = await callback();
  if (decision?.action !== 'write') fail(`Writer mutation boundary rejected: ${decision?.reason ?? 'unknown'}`);
}

async function boundedText(response, bodyTimeoutMs, controller) {
  if (response.body === null) return '';
  const reader = response.body.getReader();
  const chunks = [];
  let bytes = 0;
  let timer;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => {
      controller.abort();
      void reader.cancel().catch(() => {});
      reject(new WriterGitHubApiError('GitHub API response body timed out', response.status));
    }, bodyTimeoutMs);
  });
  const readAll = (async () => {
    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        if (!(value instanceof Uint8Array)) throw new WriterGitHubApiError('GitHub API response body is invalid', response.status);
        bytes += value.byteLength;
        if (bytes > MAX_RESPONSE_BYTES) {
          controller.abort();
          void reader.cancel().catch(() => {});
          throw new WriterGitHubApiError('GitHub API response exceeds the configured limit', response.status);
        }
        chunks.push(value);
      }
      return Buffer.concat(chunks, bytes).toString('utf8');
    } catch (error) {
      if (error instanceof WriterGitHubApiError) throw error;
      throw new WriterGitHubApiError('GitHub API response body could not be read', response.status);
    }
  })();
  try {
    return await Promise.race([readAll, timeout]);
  } finally {
    clearTimeout(timer);
    try {
      reader.releaseLock();
    } catch {
      // A hostile stream may leave a read pending even after cancellation.
    }
  }
}

export class WriterGitHubClient {
  #appJwt;
  #token = null;
  #repository;
  #repositoryId;
  #owner;
  #writerApp;
  #fetchImpl;
  #totalTimeoutMs;
  #headersTimeoutMs;
  #bodyTimeoutMs;
  #randomUUIDImpl;
  #execFileImpl;
  #identityVerified = false;

  constructor(options = {}) {
    if (!options || typeof options !== 'object' || Array.isArray(options)) fail('Writer GitHub client options are invalid');
    const allowed = new Set([
      'appJwt', 'repository', 'repositoryId', 'writerApp', 'fetchImpl',
      'totalTimeoutMs', 'headersTimeoutMs', 'bodyTimeoutMs', 'randomUUIDImpl', 'execFileImpl',
    ]);
    if (Object.keys(options).some((key) => !allowed.has(key))) fail('Writer GitHub client options contain unsupported fields');
    const {
      appJwt,
      repository,
      repositoryId,
      writerApp,
      fetchImpl = globalThis.fetch,
      totalTimeoutMs,
      headersTimeoutMs,
      bodyTimeoutMs,
      randomUUIDImpl = randomUUID,
      execFileImpl = execFileAsync,
    } = options;
    validateAppJwt(appJwt);
    validateRepository(repository);
    positiveId(repositoryId, 'GitHub repository ID');
    const normalizedWriterApp = validateWriterApp(writerApp);
    if (typeof fetchImpl !== 'function') fail('GitHub fetch implementation is invalid');
    if (typeof randomUUIDImpl !== 'function') fail('Writer UUID implementation is invalid');
    if (typeof execFileImpl !== 'function') fail('Writer subprocess implementation is invalid');

    this.#appJwt = appJwt;
    this.#repository = repository;
    this.#repositoryId = repositoryId;
    this.#owner = repository.split('/')[0];
    this.#writerApp = normalizedWriterApp;
    this.#fetchImpl = fetchImpl;
    this.#randomUUIDImpl = randomUUIDImpl;
    this.#execFileImpl = execFileImpl;
    this.#totalTimeoutMs = boundedTimeout(totalTimeoutMs, 'Total timeout', DEFAULT_TOTAL_TIMEOUT_MS);
    this.#headersTimeoutMs = boundedTimeout(headersTimeoutMs, 'Headers timeout', DEFAULT_HEADERS_TIMEOUT_MS);
    this.#bodyTimeoutMs = boundedTimeout(bodyTimeoutMs, 'Body timeout', DEFAULT_BODY_TIMEOUT_MS);
    if (this.#headersTimeoutMs >= this.#totalTimeoutMs || this.#bodyTimeoutMs >= this.#totalTimeoutMs) {
      fail('Headers and body timeouts must be shorter than the total timeout');
    }
  }

  async #requestWithCredential(credential, method, path, body = undefined, expectedStatus = null) {
    const controller = new AbortController();
    const operation = (async () => {
      const response = await deadline(
        Promise.resolve(this.#fetchImpl(`${API_ORIGIN}${path}`, {
          method,
          redirect: 'error',
          signal: controller.signal,
          headers: {
            accept: 'application/vnd.github+json',
            authorization: `Bearer ${credential}`,
            'content-type': 'application/json',
            'x-github-api-version': '2022-11-28',
          },
          body: body === undefined ? undefined : JSON.stringify(body),
        })),
        this.#headersTimeoutMs,
        'GitHub API response headers timed out',
        controller,
      );
      const text = response.status === 204 ? '' : await boundedText(response, this.#bodyTimeoutMs, controller);
      if (!response.ok) throw new WriterGitHubApiError(`GitHub API returned HTTP ${response.status}`, response.status);
      if (expectedStatus !== null && response.status !== expectedStatus) {
        throw new WriterGitHubApiError(
          `GitHub API returned HTTP ${response.status}; expected ${expectedStatus}`,
          response.status,
        );
      }
      if (!text) return null;
      try {
        return JSON.parse(text);
      } catch {
        throw new WriterGitHubApiError('GitHub API returned invalid JSON', response.status);
      }
    })();
    return deadline(operation, this.#totalTimeoutMs, 'GitHub API request timed out', controller);
  }

  #request(method, path, body = undefined, expectedStatus = null) {
    if (this.#token === null) fail('Writer installation token has not been minted and verified');
    return this.#requestWithCredential(this.#token, method, path, body, expectedStatus);
  }

  #requestAsApp(method, path, body = undefined, expectedStatus = null) {
    return this.#requestWithCredential(this.#appJwt, method, path, body, expectedStatus);
  }

  async verifyInstallationIdentity() {
    this.#identityVerified = false;
    this.#token = null;
    const app = await this.#requestAsApp('GET', '/app');
    if (app?.id !== this.#writerApp.id || app?.slug !== this.#writerApp.slug) {
      fail('Writer App JWT identity does not match configuration');
    }
    const installation = await this.#requestAsApp('GET', `/repos/${this.#repository}/installation`);
    const installationId = positiveId(installation?.id, 'Writer installation ID');
    if (
      installation?.app_id !== this.#writerApp.id || installation?.app_slug !== this.#writerApp.slug ||
      installation?.repository_selection !== 'selected' ||
      typeof installation?.account?.login !== 'string' ||
      installation.account.login.toLowerCase() !== this.#owner.toLowerCase()
    ) fail('Writer App installation does not match the configured repository owner and App');
    validateInstallationPermissions(installation.permissions);

    const access = await this.#requestAsApp(
      'POST',
      `/app/installations/${installationId}/access_tokens`,
      { permissions: WRITER_INSTALLATION_PERMISSIONS },
      201,
    );
    const token = validateText(access?.token, 'Writer installation token', {
      maximumBytes: MAX_TOKEN_LENGTH,
      required: true,
      singleLine: true,
    });
    if (token === this.#appJwt) fail('Writer App JWT and installation token must be distinct');
    const tokenExpiresAt = Date.parse(access?.expires_at);
    const now = Date.now();
    if (
      !validGitHubTimestamp(access?.expires_at) || tokenExpiresAt <= now ||
      tokenExpiresAt - now > MAX_INSTALLATION_TOKEN_LIFETIME_MS
    ) {
      fail('Writer installation token expiry is invalid');
    }
    if (access?.repository_selection !== 'selected') fail('Writer installation token repository selection is invalid');
    validateInstallationPermissions(access.permissions);
    if (Object.hasOwn(access, 'repositories')) {
      if (
        !Array.isArray(access.repositories) || access.repositories.length !== 1 ||
        !this.#sameRepository(access.repositories[0])
      ) fail('Writer installation token repository list is invalid');
    }

    const repositories = await this.#requestWithCredential(
      token,
      'GET',
      '/installation/repositories?per_page=2&page=1',
    );
    if (
      (Object.hasOwn(repositories ?? {}, 'repository_selection') && repositories.repository_selection !== 'selected') ||
      repositories?.total_count !== 1 ||
      !Array.isArray(repositories.repositories) ||
      repositories.repositories.length !== 1 ||
      !this.#sameRepository(repositories.repositories[0])
    ) fail('Writer installation token must authorize exactly the configured repository');
    this.#token = token;
    this.#identityVerified = true;
    return { appId: app.id, appSlug: app.slug, installationId, repositoryId: this.#repositoryId };
  }

  #requireVerifiedIdentity() {
    if (!this.#identityVerified) fail('Writer installation token identity is not verified');
  }

  #createAttemptId() {
    let value;
    try {
      value = this.#randomUUIDImpl();
    } catch {
      fail('Writer could not generate a create attempt ID');
    }
    return validateCreateAttemptId(value);
  }

  async #list(path) {
    const items = [];
    for (let page = 1; page <= MAX_PAGES; page += 1) {
      const batch = await this.#request('GET', `${path}${path.includes('?') ? '&' : '?'}per_page=${PAGE_SIZE}&page=${page}`);
      if (!Array.isArray(batch)) throw new WriterGitHubApiError('GitHub list response is invalid');
      items.push(...batch);
      if (batch.length < PAGE_SIZE) return items;
    }
    throw new WriterGitHubApiError('GitHub list response exceeds the configured page limit');
  }

  #normalizedAuthor(pull) {
    const user = pull?.user;
    const app = pull?.performed_via_github_app;
    const expectedBotLogin = `${this.#writerApp.slug}[bot]`;
    if (user?.type !== 'Bot' || user.login !== expectedBotLogin) return null;
    if (app != null && (app?.id !== this.#writerApp.id || app.slug !== this.#writerApp.slug)) return null;
    return { type: 'App', id: this.#writerApp.id };
  }

  #normalizePull(pull) {
    if (!pull || typeof pull !== 'object' || Array.isArray(pull)) return pull;
    let merged = pull.merged;
    if (typeof merged !== 'boolean') {
      const validMergedAt = pull.merged_at === null || (
        validGitHubTimestamp(pull.merged_at)
      );
      if (!validMergedAt || (pull.state !== 'open' && pull.state !== 'closed')) {
        merged = null;
      } else if (pull.state === 'open') {
        merged = pull.merged_at === null ? false : null;
      } else {
        merged = pull.merged_at !== null;
      }
    }
    return { ...pull, merged, author: this.#normalizedAuthor(pull) };
  }

  #sameRepository(repo) {
    return repo?.id === this.#repositoryId &&
      typeof repo.full_name === 'string' &&
      repo.full_name.toLowerCase() === this.#repository.toLowerCase();
  }

  #verifyManagedPull(rawPull, { issueNumber, pullNumber, expectedHeadSha, metadata = null }) {
    const expectedRef = agentRef(issueNumber);
    const pull = this.#normalizePull(rawPull);
    if (!pull || typeof pull !== 'object' || Array.isArray(pull) || pull.number !== pullNumber) fail('Managed Draft PR response is invalid');
    if (pull.state !== 'open') fail('Managed Draft PR is not open');
    if (pull.merged !== false) fail('Managed Draft PR merge state is invalid');
    if (pull.draft !== true) fail('Managed Draft PR is not a draft');
    if (pull.base?.ref !== 'main' || !this.#sameRepository(pull.base?.repo)) fail('Managed Draft PR base is invalid');
    if (pull.head?.ref !== expectedRef || !this.#sameRepository(pull.head?.repo)) fail('Managed Draft PR head is invalid');
    if (pull.head?.sha !== expectedHeadSha) fail('Managed Draft PR head SHA changed');
    if (!hasCanonicalWriterOwnershipMarker(pull.body)) fail('Managed Draft PR ownership marker is missing or non-canonical');
    if (pull.author?.type !== 'App' || pull.author.id !== this.#writerApp.id) fail('Managed Draft PR Writer App ownership is invalid');
    if (metadata) {
      if (Object.hasOwn(metadata, 'title') && pull.title !== metadata.title) fail('Managed Draft PR title update was not persisted');
      if (Object.hasOwn(metadata, 'body') && pull.body !== metadata.body) fail('Managed Draft PR body update was not persisted');
    }
    return pull;
  }

  #verifyRef(rawRef, issueNumber, expectedSha) {
    const expectedRef = `refs/heads/${agentRef(issueNumber)}`;
    if (
      !rawRef ||
      typeof rawRef !== 'object' ||
      Array.isArray(rawRef) ||
      rawRef.ref !== expectedRef ||
      rawRef.object?.type !== 'commit' ||
      rawRef.object.sha !== expectedSha
    ) fail('Writer ref response does not match the requested branch and SHA');
    return rawRef;
  }

  async #readAgentRef(issueNumber) {
    const ref = agentRef(issueNumber);
    return this.#request('GET', `/repos/${this.#repository}/git/ref/heads/${encodeURIComponent(ref)}`);
  }

  getRepository() {
    return this.#request('GET', `/repos/${this.#repository}`);
  }

  getIssue(number) {
    return this.#request('GET', `/repos/${this.#repository}/issues/${positiveNumber(number, 'Issue number')}`);
  }

  getIssueComment(commentId) {
    return this.#request('GET', `/repos/${this.#repository}/issues/comments/${positiveId(commentId, 'Issue comment ID')}`);
  }

  getCollaboratorPermission(username) {
    if (typeof username !== 'string' || !USERNAME_PATTERN.test(username) || username.includes('--')) fail('GitHub username is invalid');
    return this.#request('GET', `/repos/${this.#repository}/collaborators/${encodeURIComponent(username)}/permission`)
      .then((value) => value?.permission ?? null);
  }

  async getPull(number) {
    const pull = await this.#request('GET', `/repos/${this.#repository}/pulls/${positiveNumber(number, 'Pull request number')}`);
    return this.#normalizePull(pull);
  }

  getRef(ref) {
    return this.#request('GET', `/repos/${this.#repository}/git/ref/heads/${encodeURIComponent(validatedAgentRef(ref))}`);
  }

  getMainRef() {
    return this.#request('GET', `/repos/${this.#repository}/git/ref/heads/main`);
  }

  async listPullsForHead(ref) {
    const head = `${this.#owner}:${validatedAgentRef(ref)}`;
    const pulls = await this.#list(`/repos/${this.#repository}/pulls?state=all&head=${encodeURIComponent(head)}`);
    return Promise.all(pulls.map(async (rawPull) => {
      const pullNumber = positiveNumber(rawPull?.number, 'Pull request number');
      const pull = this.#normalizePull(await this.#request(
        'GET',
        `/repos/${this.#repository}/pulls/${pullNumber}`,
      ));
      if (pull?.number !== pullNumber) fail('Writer detailed pull response does not match the listed pull');
      const events = await this.#list(`/repos/${this.#repository}/issues/${pullNumber}/timeline`);
      const writerLifecycle = writerPullLifecycleAttestation(events);
      if (writerLifecycle === null) fail('Writer pull lifecycle timeline is invalid');
      return { ...pull, writer_lifecycle: writerLifecycle };
    }));
  }

  async createDraftPull(issueNumber, metadata, mutationBoundary = null) {
    const verifiedIssueNumber = positiveNumber(issueNumber, 'Issue number');
    const normalized = validateMetadata(metadata, {
      requireBoth: true,
      createAttemptId: this.#createAttemptId(),
    });
    this.#requireVerifiedIdentity();
    const currentRef = await this.#readAgentRef(verifiedIssueNumber);
    const expectedHeadSha = validateSha(currentRef?.object?.sha, 'Writer ref SHA');
    this.#verifyRef(currentRef, verifiedIssueNumber, expectedHeadSha);
    this.#verifyRef(await this.#readAgentRef(verifiedIssueNumber), verifiedIssueNumber, expectedHeadSha);
    await requireMutationBoundary(mutationBoundary);
    let pullNumber = null;
    try {
      const created = await this.#request('POST', `/repos/${this.#repository}/pulls`, {
        ...normalized,
        base: 'main',
        head: agentRef(verifiedIssueNumber),
        draft: true,
      }, 201);
      pullNumber = positiveNumber(created?.number, 'Created Draft PR number');
    } catch (error) {
      if (!isAmbiguousMutationError(error)) throw error;
    }

    if (pullNumber !== null) {
      try {
        const persisted = await this.#request('GET', `/repos/${this.#repository}/pulls/${pullNumber}`);
        const verified = this.#verifyManagedPull(persisted, {
          issueNumber: verifiedIssueNumber,
          pullNumber,
          expectedHeadSha,
          metadata: normalized,
        });
        this.#verifyRef(await this.#readAgentRef(verifiedIssueNumber), verifiedIssueNumber, expectedHeadSha);
        return verified;
      } catch {
        // Reconcile against the nonce below; no compensating mutation is safe.
      }
    }

    return this.#reconcileCreatedPull({
      issueNumber: verifiedIssueNumber,
      expectedHeadSha,
      metadata: normalized,
    });
  }

  #isAttributableCreatedPull(pull, issueNumber, expectedHeadSha, metadata, state) {
    const expectedRef = agentRef(issueNumber);
    return pull && typeof pull === 'object' && !Array.isArray(pull) &&
      Number.isSafeInteger(pull.number) && pull.number > 0 && pull.number <= 2_147_483_647 &&
      pull.state === state && pull.merged === false && pull.draft === true &&
      pull.base?.ref === 'main' && this.#sameRepository(pull.base?.repo) &&
      pull.head?.ref === expectedRef && this.#sameRepository(pull.head?.repo) &&
      typeof pull.head?.sha === 'string' && SHA_PATTERN.test(pull.head.sha) &&
      pull.head.sha === expectedHeadSha &&
      pull.title === metadata.title && pull.body === metadata.body &&
      hasCanonicalWriterOwnershipMarker(pull.body) &&
      pull.author?.type === 'App' && pull.author.id === this.#writerApp.id;
  }

  async #reconcileCreatedPull({ issueNumber, expectedHeadSha, metadata }) {
    validateSha(expectedHeadSha, 'Expected create-attempt head SHA');
    const markerStart = metadata.body.indexOf(WRITER_CREATE_ATTEMPT_PREFIX);
    const markerEnd = metadata.body.indexOf(' -->', markerStart);
    if (markerStart < 0 || markerEnd < 0) fail('Writer reconciliation metadata is missing its create-attempt marker');
    const attemptMarker = metadata.body.slice(markerStart, markerEnd + ' -->'.length);

    // GitHub does not offer an atomic "close only if head SHA still equals X"
    // precondition. Recovery is therefore read-only: either prove one unchanged
    // open PR belongs to this nonce, or leave fail-closed residue for later repair.
    const head = `${this.#owner}:${agentRef(issueNumber)}`;
    let pulls;
    try {
      pulls = (await this.#list(
        `/repos/${this.#repository}/pulls?state=all&base=main&head=${encodeURIComponent(head)}`,
      )).map((pull) => this.#normalizePull(pull));
    } catch {
      fail('Writer could not enumerate a uniquely attributable Draft PR after create; platform residue may remain');
    }
    const candidates = pulls.filter((pull) => typeof pull?.body === 'string' && pull.body.includes(attemptMarker));
    if (candidates.length !== 1) {
      fail('Writer could not uniquely attribute the created Draft PR; platform residue may remain');
    }
    const candidate = candidates[0];
    if (!this.#isAttributableCreatedPull(candidate, issueNumber, expectedHeadSha, metadata, 'open')) {
      fail('Writer refused to reconcile a changed or unverified created Draft PR; platform residue may remain');
    }

    let persisted;
    try {
      persisted = this.#normalizePull(await this.#request('GET', `/repos/${this.#repository}/pulls/${candidate.number}`));
    } catch {
      fail('Writer could not re-read the attributable Draft PR after create; platform residue may remain');
    }
    if (!this.#isAttributableCreatedPull(persisted, issueNumber, expectedHeadSha, metadata, 'open')) {
      fail('Writer refused to reconcile a Draft PR that changed after create; platform residue may remain');
    }

    try {
      this.#verifyRef(await this.#readAgentRef(issueNumber), issueNumber, expectedHeadSha);
    } catch {
      fail('Writer branch changed while reconciling the created Draft PR; platform residue may remain');
    }
    return persisted;
  }

  async verifyManagedDraftPullMetadata(issueNumber, pullNumber, expectedHeadSha, metadata, mutationBoundary = null) {
    const normalized = validateMetadata(metadata);
    const verifiedIssueNumber = positiveNumber(issueNumber, 'Issue number');
    const verifiedPullNumber = positiveNumber(pullNumber, 'Pull request number');
    const verifiedHeadSha = validateSha(expectedHeadSha, 'Expected head SHA');
    this.#requireVerifiedIdentity();
    const path = `/repos/${this.#repository}/pulls/${verifiedPullNumber}`;
    this.#verifyManagedPull(await this.#request('GET', path), {
      issueNumber: verifiedIssueNumber,
      pullNumber: verifiedPullNumber,
      expectedHeadSha: verifiedHeadSha,
      metadata: normalized,
    });
    await requireMutationBoundary(mutationBoundary);
    return this.#verifyManagedPull(await this.#request('GET', path), {
      issueNumber: verifiedIssueNumber,
      pullNumber: verifiedPullNumber,
      expectedHeadSha: verifiedHeadSha,
      metadata: normalized,
    });
  }

  async compareCommits(baseSha, headSha) {
    const verifiedBaseSha = validateSha(baseSha, 'Compare base SHA');
    const verifiedHeadSha = validateSha(headSha, 'Compare head SHA');
    this.#requireVerifiedIdentity();
    return this.#request('GET', `/repos/${this.#repository}/compare/${verifiedBaseSha}...${verifiedHeadSha}`);
  }

  async pushAgentRefFromRepository(issueNumber, expectedOldSha, newSha, repositoryPath, mutationBoundary = null) {
    const verifiedIssueNumber = positiveNumber(issueNumber, 'Issue number');
    const verifiedNewSha = validateSha(newSha, 'New ref SHA');
    const verifiedOldSha = expectedOldSha === null ? null : validateSha(expectedOldSha, 'Expected old ref SHA');
    if (verifiedOldSha === verifiedNewSha) fail('New ref SHA must differ from the expected old SHA');
    if (typeof repositoryPath !== 'string' || repositoryPath.length === 0 || !path.isAbsolute(repositoryPath)) {
      fail('Writer repository path must be absolute');
    }
    this.#requireVerifiedIdentity();
    await this.#execFileImpl('git', ['cat-file', '-e', `${verifiedNewSha}^{commit}`], {
      cwd: repositoryPath,
      timeout: 30_000,
      windowsHide: true,
      maxBuffer: 1024 * 1024,
    });
    const remote = await this.#execFileImpl('git', ['remote', 'get-url', '--push', 'origin'], {
      cwd: repositoryPath,
      timeout: 30_000,
      windowsHide: true,
      maxBuffer: 4096,
    });
    const expectedRemote = `https://github.com/${this.#repository}.git`;
    if (remote.stdout.trim().toLowerCase() !== expectedRemote.toLowerCase()) {
      fail('Writer push remote does not match the configured repository');
    }

    const readExpected = async () => {
      try {
        const ref = await this.#readAgentRef(verifiedIssueNumber);
        if (verifiedOldSha === null) fail('Writer ref already exists');
        return this.#verifyRef(ref, verifiedIssueNumber, verifiedOldSha);
      } catch (error) {
        if (verifiedOldSha === null && error instanceof WriterGitHubApiError && error.status === 404) return null;
        throw error;
      }
    };
    await readExpected();
    await readExpected();
    await requireMutationBoundary(mutationBoundary);

    const basic = Buffer.from(`x-access-token:${this.#token}`, 'utf8').toString('base64');
    let mutationError = null;
    try {
      await this.#execFileImpl('git', writerRefPushArguments(verifiedIssueNumber, verifiedOldSha, verifiedNewSha), {
        cwd: repositoryPath,
        timeout: 120_000,
        windowsHide: true,
        maxBuffer: 1024 * 1024,
        env: {
          ...process.env,
          GIT_TERMINAL_PROMPT: '0',
          GIT_CONFIG_COUNT: '3',
          GIT_CONFIG_KEY_0: 'http.https://github.com/.extraHeader',
          GIT_CONFIG_VALUE_0: `Authorization: Basic ${basic}`,
          GIT_CONFIG_KEY_1: 'credential.helper',
          GIT_CONFIG_VALUE_1: '',
          GIT_CONFIG_KEY_2: 'core.hooksPath',
          GIT_CONFIG_VALUE_2: '.git/aeris-disabled-hooks',
        },
      });
    } catch (error) {
      mutationError = error;
    }
    try {
      return this.#verifyRef(await this.#readAgentRef(verifiedIssueNumber), verifiedIssueNumber, verifiedNewSha);
    } catch (error) {
      if (mutationError) {
        throw new AggregateError(
          [new WriterGitHubApiError('Writer git push result was ambiguous'), error],
          'Writer could not reconcile an ambiguous lease-protected agent ref push; platform state may have changed',
        );
      }
      throw error;
    }
  }
}
