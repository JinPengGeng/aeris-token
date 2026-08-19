import {
  GITHUB_ACTIONS_APP,
  REVIEW_ATTESTATION_CHECK_NAMES,
  lifecycleFromGraphqlPull,
} from './review-attestation-contract.mjs';

const API_ORIGIN = 'https://api.github.com';
const SHA = /^[0-9a-f]{40}$/;
const MAXIMUM_RESPONSE_BYTES = 2 * 1024 * 1024;

function requireCondition(condition, message) { if (!condition) throw new Error(message); }
function positive(value, name) { requireCondition(Number.isSafeInteger(value) && value > 0, `${name} is invalid`); return value; }

async function boundedText(response) {
  const declared = Number(response.headers.get('content-length'));
  requireCondition(!Number.isFinite(declared) || declared <= MAXIMUM_RESPONSE_BYTES, 'GitHub response is too large');
  const reader = response.body?.getReader();
  if (!reader) return '';
  const chunks = [];
  let size = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > MAXIMUM_RESPONSE_BYTES) {
      await reader.cancel();
      throw new Error('GitHub response is too large');
    }
    chunks.push(value);
  }
  const combined = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { combined.set(chunk, offset); offset += chunk.byteLength; }
  return new TextDecoder().decode(combined);
}

function normalizeCheck(raw) {
  return {
    id: raw?.id,
    name: raw?.name,
    head_sha: raw?.head_sha,
    status: raw?.status,
    conclusion: raw?.conclusion,
    external_id: raw?.external_id,
    details_url: raw?.details_url,
    app: raw?.app == null ? null : { id: raw.app.id, slug: raw.app.slug },
    output: raw?.output == null ? null : { title: raw.output.title, summary: raw.output.summary },
  };
}

export class AiReviewGitHubClient {
  constructor({ token, repository, repositoryId, fetchImpl = globalThis.fetch, requestTimeoutMs = 30_000 }) {
    requireCondition(typeof token === 'string' && token.length > 0, 'GitHub token is missing');
    requireCondition(typeof repository === 'string' && /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository), 'GitHub repository is invalid');
    positive(repositoryId, 'GitHub repository ID');
    const [owner, name] = repository.split('/');
    this.token = token;
    this.repository = repository;
    this.repositoryId = repositoryId;
    this.owner = owner;
    this.name = name;
    this.fetchImpl = fetchImpl;
    this.requestTimeoutMs = requestTimeoutMs;
  }

  async request(method, apiPath, body = undefined) {
    requireCondition(apiPath.startsWith('/') && !apiPath.startsWith('//'), 'GitHub API path is invalid');
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(new Error('timeout')), this.requestTimeoutMs);
    try {
      const response = await this.fetchImpl(`${API_ORIGIN}${apiPath}`, {
        method,
        redirect: 'error',
        signal: controller.signal,
        headers: {
          accept: 'application/vnd.github+json',
          authorization: `Bearer ${this.token}`,
          'content-type': 'application/json',
          'x-github-api-version': '2022-11-28',
        },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      const text = await boundedText(response);
      if (!response.ok) throw new Error(`GitHub API returned HTTP ${response.status}`);
      if (!text) return null;
      try { return JSON.parse(text); } catch { throw new Error('GitHub API returned invalid JSON'); }
    } catch (error) {
      if (controller.signal.aborted) throw new Error('GitHub API request timed out');
      throw error;
    } finally { clearTimeout(timeout); }
  }

  async getRepository() {
    const value = await this.request('GET', `/repos/${this.repository}`);
    requireCondition(value?.id === this.repositoryId && value?.full_name === this.repository && typeof value.default_branch === 'string', 'GitHub repository identity changed');
    return { id: value.id, full_name: value.full_name, default_branch: value.default_branch };
  }

  async getBranchHead(branch) {
    requireCondition(typeof branch === 'string' && /^[A-Za-z0-9._/-]+$/.test(branch) && !branch.includes('..'), 'GitHub branch is invalid');
    const value = await this.request('GET', `/repos/${this.repository}/git/ref/heads/${encodeURIComponent(branch)}`);
    requireCondition(value?.ref === `refs/heads/${branch}` && SHA.test(value?.object?.sha ?? ''), 'GitHub branch response is invalid');
    return value.object.sha;
  }

  async getPull(pullNumber) {
    positive(pullNumber, 'pull number');
    const value = await this.request('GET', `/repos/${this.repository}/pulls/${pullNumber}`);
    requireCondition(value?.number === pullNumber, 'GitHub pull response is invalid');
    return {
      number: value.number, state: value.state, draft: value.draft, title: value.title, body: value.body ?? '', changed_files: value.changed_files,
      author: { login: value.user?.login, id: value.user?.id, type: value.user?.type },
      head: { ref: value.head?.ref, sha: value.head?.sha, repo: { id: value.head?.repo?.id, full_name: value.head?.repo?.full_name } },
      base: { ref: value.base?.ref, sha: value.base?.sha, repo: { id: value.base?.repo?.id, full_name: value.base?.repo?.full_name } },
    };
  }

  async listPullFiles(pullNumber) {
    positive(pullNumber, 'pull number');
    const files = [];
    for (let page = 1; page <= 3; page += 1) {
      const batch = await this.request('GET', `/repos/${this.repository}/pulls/${pullNumber}/files?per_page=100&page=${page}`);
      requireCondition(Array.isArray(batch), 'GitHub pull files response is invalid');
      files.push(...batch.map((file) => ({
        filename: file?.filename,
        sha: file?.sha,
        previous_filename: file?.previous_filename ?? null,
        status: file?.status,
        additions: file?.additions,
        deletions: file?.deletions,
        changes: file?.changes,
        patch: file?.patch ?? null,
      })));
      if (batch.length < 100) return { files, truncated: false };
    }
    return { files, truncated: true };
  }

  async getPullLifecycle(pullNumber) {
    const query = `query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){databaseId pullRequest(number:$number){id number createdAt state headRefOid baseRefOid timelineItems(last:100,itemTypes:[CLOSED_EVENT,REOPENED_EVENT]){nodes{__typename ... on ClosedEvent{databaseId createdAt} ... on ReopenedEvent{databaseId createdAt}}pageInfo{hasPreviousPage}}}}}`;
    const value = await this.request('POST', '/graphql', { query, variables: { owner: this.owner, name: this.name, number: pullNumber } });
    requireCondition(!Array.isArray(value?.errors) || value.errors.length === 0, 'GitHub lifecycle query returned errors');
    const repository = value?.data?.repository;
    const pull = repository?.pullRequest;
    requireCondition(repository?.databaseId === this.repositoryId && pull?.number === pullNumber && SHA.test(pull?.headRefOid ?? '') && SHA.test(pull?.baseRefOid ?? ''), 'GitHub lifecycle response is invalid');
    return { head_sha: pull.headRefOid, base_sha: pull.baseRefOid, lifecycle_epoch: lifecycleFromGraphqlPull(pull) };
  }

  async listCheckRunsForRef(headSha) {
    requireCondition(SHA.test(headSha), 'check run head SHA is invalid');
    const checks = [];
    for (let page = 1; page <= 10; page += 1) {
      const value = await this.request('GET', `/repos/${this.repository}/commits/${headSha}/check-runs?filter=all&per_page=100&page=${page}`);
      requireCondition(Array.isArray(value?.check_runs), 'GitHub check-runs response is invalid');
      checks.push(...value.check_runs.map(normalizeCheck));
      if (value.check_runs.length < 100) return checks;
    }
    throw new Error('GitHub check-runs response is truncated');
  }

  async getCheckRun(checkId) {
    positive(checkId, 'check run ID');
    return normalizeCheck(await this.request('GET', `/repos/${this.repository}/check-runs/${checkId}`));
  }

  async publishCompletedReviewCheck({ role, headSha, conclusion, externalId, output, detailsUrl, completedAt }) {
    const name = REVIEW_ATTESTATION_CHECK_NAMES[role];
    requireCondition(typeof name === 'string', 'review check role is invalid');
    requireCondition(['success', 'failure', 'cancelled', 'timed_out'].includes(conclusion), 'review check conclusion is invalid');
    const before = await this.listCheckRunsForRef(headSha);
    const named = before.filter((check) => check.name === name && check.head_sha === headSha);
    requireCondition(named.every((check) => check.app?.id === GITHUB_ACTIONS_APP.id && check.app?.slug === GITHUB_ACTIONS_APP.slug), `non-GitHub-Actions App used ${name}`);
    const created = normalizeCheck(await this.request('POST', `/repos/${this.repository}/check-runs`, {
      name,
      head_sha: headSha,
      status: 'completed',
      conclusion,
      external_id: externalId,
      completed_at: completedAt,
      details_url: detailsUrl,
      output,
    }));
    const persisted = await this.getCheckRun(created.id);
    requireCondition(
      persisted.name === name && persisted.head_sha === headSha && persisted.status === 'completed' && persisted.conclusion === conclusion &&
        persisted.external_id === externalId && persisted.app?.id === GITHUB_ACTIONS_APP.id && persisted.app?.slug === GITHUB_ACTIONS_APP.slug &&
        persisted.output?.title === output.title && persisted.output?.summary === output.summary,
      'persisted review check does not match the completed publication',
    );
    const after = (await this.listCheckRunsForRef(headSha)).filter((check) => check.name === name && check.head_sha === headSha);
    requireCondition(after.every((check) => check.app?.id === GITHUB_ACTIONS_APP.id && check.app?.slug === GITHUB_ACTIONS_APP.slug), `review check owner changed after publication`);
    requireCondition(after.reduce((highest, check) => Math.max(highest, check.id), 0) === persisted.id, 'published review check is not the latest role check');
    return persisted;
  }
}
