import { WRITER_OWNERSHIP_MARKER } from './writer-lifecycle.mjs';

const API_ORIGIN = 'https://api.github.com';
const MAX_RESPONSE_BYTES = 1_048_576;
const MAX_BODY_BYTES = 65_536;
const MAX_TOKEN_LENGTH = 8_192;
const MAX_PAGES = 3;
const PAGE_SIZE = 100;

const OWNER_PATTERN = /^[A-Za-z\d](?:[A-Za-z\d-]{0,37}[A-Za-z\d])?$/;
const REPOSITORY_NAME_PATTERN = /^[A-Za-z\d_.-]+$/;
const USERNAME_PATTERN = /^[A-Za-z\d](?:[A-Za-z\d-]{0,37}[A-Za-z\d])?$/;
const APP_SLUG_PATTERN = /^[a-z\d](?:[a-z\d-]{0,98}[a-z\d])?$/;
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

function markedBody(value) {
  if (value.includes(WRITER_OWNERSHIP_MARKER)) return value;
  return `${value}${value.length === 0 || value.endsWith('\n') ? '' : '\n\n'}${WRITER_OWNERSHIP_MARKER}`;
}

function validateMetadata(metadata, { requireBoth = false } = {}) {
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
    result.body = validateText(markedBody(validateText(metadata.body, 'Pull request body', {
      maximumBytes: MAX_BODY_BYTES,
    })), 'Pull request body', { maximumBytes: MAX_BODY_BYTES });
  }
  if (Buffer.byteLength(JSON.stringify(result), 'utf8') > MAX_BODY_BYTES) {
    fail('Pull request metadata exceeds the configured limit');
  }
  return result;
}

async function boundedText(response) {
  const text = await response.text();
  if (Buffer.byteLength(text, 'utf8') > MAX_RESPONSE_BYTES) {
    throw new WriterGitHubApiError('GitHub API response exceeds the configured limit');
  }
  return text;
}

export class WriterGitHubClient {
  #token;
  #repository;
  #repositoryId;
  #owner;
  #writerApp;
  #fetchImpl;

  constructor(options = {}) {
    if (!options || typeof options !== 'object' || Array.isArray(options)) fail('Writer GitHub client options are invalid');
    const allowed = new Set(['token', 'repository', 'repositoryId', 'writerApp', 'fetchImpl']);
    if (Object.keys(options).some((key) => !allowed.has(key))) fail('Writer GitHub client options contain unsupported fields');
    const {
      token,
      repository,
      repositoryId,
      writerApp,
      fetchImpl = globalThis.fetch,
    } = options;
    validateText(token, 'GitHub token', { maximumBytes: MAX_TOKEN_LENGTH, required: true, singleLine: true });
    validateRepository(repository);
    positiveId(repositoryId, 'GitHub repository ID');
    const normalizedWriterApp = validateWriterApp(writerApp);
    if (typeof fetchImpl !== 'function') fail('GitHub fetch implementation is invalid');

    this.#token = token;
    this.#repository = repository;
    this.#repositoryId = repositoryId;
    this.#owner = repository.split('/')[0];
    this.#writerApp = normalizedWriterApp;
    this.#fetchImpl = fetchImpl;
  }

  async #request(method, path, body = undefined) {
    const response = await this.#fetchImpl(`${API_ORIGIN}${path}`, {
      method,
      redirect: 'error',
      headers: {
        accept: 'application/vnd.github+json',
        authorization: `Bearer ${this.#token}`,
        'content-type': 'application/json',
        'x-github-api-version': '2022-11-28',
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = response.status === 204 ? '' : await boundedText(response);
    if (!response.ok) throw new WriterGitHubApiError(`GitHub API returned HTTP ${response.status}`, response.status);
    if (!text) return null;
    try {
      return JSON.parse(text);
    } catch {
      throw new WriterGitHubApiError('GitHub API returned invalid JSON', response.status);
    }
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
    if (typeof pull.body !== 'string' || !pull.body.includes(WRITER_OWNERSHIP_MARKER)) fail('Managed Draft PR ownership marker is missing');
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

  getIssue(number) {
    return this.#request('GET', `/repos/${this.#repository}/issues/${positiveNumber(number, 'Issue number')}`);
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

  async listPullsForHead(ref) {
    const head = `${this.#owner}:${validatedAgentRef(ref)}`;
    const pulls = await this.#list(`/repos/${this.#repository}/pulls?state=all&head=${encodeURIComponent(head)}`);
    return pulls.map((pull) => this.#normalizePull(pull));
  }

  async createDraftPull(issueNumber, metadata) {
    const verifiedIssueNumber = positiveNumber(issueNumber, 'Issue number');
    const normalized = validateMetadata(metadata, { requireBoth: true });
    const currentRef = await this.#readAgentRef(verifiedIssueNumber);
    const expectedHeadSha = validateSha(currentRef?.object?.sha, 'Writer ref SHA');
    this.#verifyRef(currentRef, verifiedIssueNumber, expectedHeadSha);
    const created = await this.#request('POST', `/repos/${this.#repository}/pulls`, {
      ...normalized,
      base: 'main',
      head: agentRef(verifiedIssueNumber),
      draft: true,
    });
    const pullNumber = positiveNumber(created?.number, 'Created Draft PR number');
    const persisted = await this.#request('GET', `/repos/${this.#repository}/pulls/${pullNumber}`);
    return this.#verifyManagedPull(persisted, {
      issueNumber: verifiedIssueNumber,
      pullNumber,
      expectedHeadSha,
      metadata: normalized,
    });
  }

  async updateManagedDraftPull(issueNumber, pullNumber, expectedHeadSha, metadata) {
    const normalized = validateMetadata(metadata);
    const verifiedIssueNumber = positiveNumber(issueNumber, 'Issue number');
    const verifiedPullNumber = positiveNumber(pullNumber, 'Pull request number');
    const verifiedHeadSha = validateSha(expectedHeadSha, 'Expected head SHA');
    const path = `/repos/${this.#repository}/pulls/${verifiedPullNumber}`;
    const before = await this.#request('GET', path);
    this.#verifyManagedPull(before, {
      issueNumber: verifiedIssueNumber,
      pullNumber: verifiedPullNumber,
      expectedHeadSha: verifiedHeadSha,
    });
    await this.#request('PATCH', path, normalized);
    const after = await this.#request('GET', path);
    return this.#verifyManagedPull(after, {
      issueNumber: verifiedIssueNumber,
      pullNumber: verifiedPullNumber,
      expectedHeadSha: verifiedHeadSha,
      metadata: normalized,
    });
  }

  async createAgentRef(issueNumber, sha) {
    const verifiedIssueNumber = positiveNumber(issueNumber, 'Issue number');
    const verifiedSha = validateSha(sha, 'Ref SHA');
    try {
      await this.#readAgentRef(verifiedIssueNumber);
      fail('Writer ref already exists');
    } catch (error) {
      if (!(error instanceof WriterGitHubApiError) || error.status !== 404) throw error;
    }
    const created = await this.#request('POST', `/repos/${this.#repository}/git/refs`, {
      ref: `refs/heads/${agentRef(verifiedIssueNumber)}`,
      sha: verifiedSha,
    });
    this.#verifyRef(created, verifiedIssueNumber, verifiedSha);
    return this.#verifyRef(await this.#readAgentRef(verifiedIssueNumber), verifiedIssueNumber, verifiedSha);
  }

  async advanceAgentRef(issueNumber, expectedOldSha, newSha) {
    const verifiedIssueNumber = positiveNumber(issueNumber, 'Issue number');
    const verifiedOldSha = validateSha(expectedOldSha, 'Expected old ref SHA');
    const verifiedNewSha = validateSha(newSha, 'New ref SHA');
    if (verifiedOldSha === verifiedNewSha) fail('New ref SHA must differ from the expected old SHA');
    this.#verifyRef(await this.#readAgentRef(verifiedIssueNumber), verifiedIssueNumber, verifiedOldSha);
    const updated = await this.#request(
      'PATCH',
      `/repos/${this.#repository}/git/refs/heads/${encodeURIComponent(agentRef(verifiedIssueNumber))}`,
      { sha: verifiedNewSha, force: false },
    );
    this.#verifyRef(updated, verifiedIssueNumber, verifiedNewSha);
    return this.#verifyRef(await this.#readAgentRef(verifiedIssueNumber), verifiedIssueNumber, verifiedNewSha);
  }
}
