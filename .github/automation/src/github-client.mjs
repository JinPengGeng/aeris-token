export class GitHubApiError extends Error {
  constructor(message, status = null) {
    super(message);
    this.name = 'GitHubApiError';
    this.status = status;
  }
}

async function boundedText(response, maximumBytes = 4_194_304) {
  const text = await response.text();
  if (Buffer.byteLength(text, 'utf8') > maximumBytes) {
    throw new GitHubApiError('GitHub API response exceeds the configured limit');
  }
  return text;
}

export class GitHubClient {
  constructor({ token, repository, apiUrl = 'https://api.github.com', fetchImpl = globalThis.fetch }) {
    if (!token) throw new GitHubApiError('GitHub token is not configured');
    if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) {
      throw new GitHubApiError('GitHub repository identifier is invalid');
    }
    const parsed = new URL(apiUrl);
    if (parsed.protocol !== 'https:') throw new GitHubApiError('GitHub API URL must use HTTPS');
    this.token = token;
    this.repository = repository;
    this.apiUrl = parsed.toString().replace(/\/$/, '');
    this.fetchImpl = fetchImpl;
  }

  async request(method, path, { body = undefined, accept = 'application/vnd.github+json' } = {}) {
    const response = await this.fetchImpl(`${this.apiUrl}${path}`, {
      method,
      headers: {
        accept,
        authorization: `Bearer ${this.token}`,
        'content-type': 'application/json',
        'x-github-api-version': '2022-11-28',
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = response.status === 204 ? '' : await boundedText(response);
    if (!response.ok) {
      throw new GitHubApiError(`GitHub API returned HTTP ${response.status}`, response.status);
    }
    if (!text) return null;
    try {
      return JSON.parse(text);
    } catch {
      throw new GitHubApiError('GitHub API returned invalid JSON', response.status);
    }
  }

  async list(path, maximumPages = 10) {
    const items = [];
    for (let page = 1; page <= maximumPages; page += 1) {
      const separator = path.includes('?') ? '&' : '?';
      const batch = await this.request('GET', `${path}${separator}per_page=100&page=${page}`);
      if (!Array.isArray(batch)) throw new GitHubApiError('GitHub list response is invalid');
      items.push(...batch);
      if (batch.length < 100) return { items, truncated: false };
    }
    return { items, truncated: true };
  }

  getIssue(number) {
    return this.request('GET', `/repos/${this.repository}/issues/${number}`);
  }

  getIssueComment(commentId) {
    return this.request('GET', `/repos/${this.repository}/issues/comments/${commentId}`);
  }

  getPull(number) {
    return this.request('GET', `/repos/${this.repository}/pulls/${number}`);
  }

  async listIssueComments(number) {
    const result = await this.list(`/repos/${this.repository}/issues/${number}/comments`, 10);
    if (result.truncated) throw new GitHubApiError('Issue has too many comments for safe managed-comment lookup');
    return result.items;
  }

  async listRepositoryLabels() {
    const result = await this.list(`/repos/${this.repository}/labels`, 10);
    if (result.truncated) throw new GitHubApiError('Repository has too many labels for safe validation');
    return result.items.map((label) => label.name).filter((name) => typeof name === 'string');
  }

  async listPullFiles(number) {
    const result = await this.list(`/repos/${this.repository}/pulls/${number}/files`, 3);
    return {
      files: result.items.map((file) => ({
        filename: file.filename,
        status: file.status,
        additions: file.additions,
        deletions: file.deletions,
        changes: file.changes,
        patch: typeof file.patch === 'string' ? file.patch : null,
      })),
      truncated: result.truncated,
    };
  }

  async listCheckRunsForRef(ref) {
    const checkRuns = [];
    const encodedRef = encodeURIComponent(ref);
    for (let page = 1; page <= 10; page += 1) {
      const value = await this.request(
        'GET',
        `/repos/${this.repository}/commits/${encodedRef}/check-runs?filter=all&per_page=100&page=${page}`,
      );
      if (!value || !Array.isArray(value.check_runs)) {
        throw new GitHubApiError('GitHub check-runs response is invalid');
      }
      checkRuns.push(...value.check_runs);
      if (value.check_runs.length < 100) return checkRuns;
    }
    throw new GitHubApiError('Commit has too many check runs for safe required-check evaluation');
  }

  async listCommitStatuses(ref) {
    const result = await this.list(
      `/repos/${this.repository}/commits/${encodeURIComponent(ref)}/statuses`,
      10,
    );
    if (result.truncated) {
      throw new GitHubApiError('Commit has too many statuses for safe required-check evaluation');
    }
    return result.items;
  }

  createIssueComment(number, body) {
    return this.request('POST', `/repos/${this.repository}/issues/${number}/comments`, { body: { body } });
  }

  updateIssueComment(commentId, body) {
    return this.request('PATCH', `/repos/${this.repository}/issues/comments/${commentId}`, {
      body: { body },
    });
  }

  async getCollaboratorPermission(username) {
    const value = await this.request(
      'GET',
      `/repos/${this.repository}/collaborators/${encodeURIComponent(username)}/permission`,
    );
    return value?.permission ?? null;
  }
}
