import { policyCheckConclusion } from './policy-gate.mjs';
import { validatePolicyEvaluationArtifact } from './policy-phase-contract.mjs';

const API_ORIGIN = 'https://api.github.com';
const MAX_RESPONSE_BYTES = 2 * 1024 * 1024;
const MAX_TOKEN_BYTES = 8192;
const PAGE_SIZE = 100;
const SHA = /^[0-9a-f]{40}$/;
const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const APP_SLUG = /^[a-z0-9](?:[a-z0-9-]{0,98}[a-z0-9])?$/;

export class PolicyGitHubApiError extends Error {
  constructor(message, status = null) {
    super(message);
    this.name = 'PolicyGitHubApiError';
    this.status = status;
  }
}

function fail(message) {
  throw new PolicyGitHubApiError(message);
}

function requireCondition(condition, message) {
  if (!condition) fail(message);
}

function positiveInteger(value, name) {
  requireCondition(Number.isSafeInteger(value) && value > 0, `${name} must be a positive safe integer`);
  return value;
}

function string(value, name, maximumBytes, pattern = null) {
  requireCondition(typeof value === 'string' && value.length > 0, `${name} is invalid`);
  requireCondition(!/[\u0000-\u001f\u007f]/.test(value), `${name} contains control characters`);
  requireCondition(Buffer.byteLength(value, 'utf8') <= maximumBytes, `${name} exceeds the configured limit`);
  if (pattern) requireCondition(pattern.test(value), `${name} format is invalid`);
  return value;
}

function validateRepository(value) {
  string(value, 'GitHub repository', 200, REPOSITORY);
  const [owner, name] = value.split('/');
  requireCondition(owner !== '.' && owner !== '..' && name !== '.' && name !== '..', 'GitHub repository is invalid');
  return { owner, name };
}

function validateApp(value) {
  requireCondition(value && typeof value === 'object' && !Array.isArray(value), 'Policy App identity is invalid');
  requireCondition(Object.keys(value).length === 2 && Object.hasOwn(value, 'id') && Object.hasOwn(value, 'slug'), 'Policy App identity has unexpected keys');
  return Object.freeze({
    id: positiveInteger(value.id, 'Policy App ID'),
    slug: string(value.slug, 'Policy App slug', 100, APP_SLUG),
  });
}

function validateGeneration(value) {
  requireCondition(value && typeof value === 'object' && !Array.isArray(value), 'Policy check generation is invalid');
  requireCondition(
    Object.keys(value).length === 6 &&
      ['repository_id', 'repository', 'pull_number', 'head_sha', 'base_sha', 'policy_sha'].every((key) => Object.hasOwn(value, key)),
    'Policy check generation has unexpected keys',
  );
  return Object.freeze({
    repository_id: positiveInteger(value.repository_id, 'Policy generation repository ID'),
    repository: string(value.repository, 'Policy generation repository', 200, REPOSITORY),
    pull_number: positiveInteger(value.pull_number, 'Policy generation pull request number'),
    head_sha: string(value.head_sha, 'Policy generation head SHA', 40, SHA),
    base_sha: string(value.base_sha, 'Policy generation base SHA', 40, SHA),
    policy_sha: string(value.policy_sha, 'Policy generation policy SHA', 40, SHA),
  });
}

function generationFromArtifact(artifact) {
  return validateGeneration({
    repository_id: artifact.repository_id,
    repository: artifact.repository,
    pull_number: artifact.pull_number,
    head_sha: artifact.head_sha,
    base_sha: artifact.base_sha,
    policy_sha: artifact.policy_sha,
  });
}

function validateDetailsUrl(value) {
  if (value === null) return null;
  const parsed = new URL(string(value, 'Policy check details URL', 2048));
  requireCondition(
    parsed.protocol === 'https:' && parsed.hostname === 'github.com' && parsed.username === '' && parsed.password === '',
    'Policy check details URL is invalid',
  );
  return value;
}

async function boundedText(response) {
  const text = response.status === 204 ? '' : await response.text();
  if (Buffer.byteLength(text, 'utf8') > MAX_RESPONSE_BYTES) {
    throw new PolicyGitHubApiError('GitHub API response exceeds the configured limit');
  }
  return text;
}

function normalizedRepository(raw) {
  return raw && typeof raw.full_name === 'string' ? { full_name: raw.full_name } : { full_name: null };
}

function normalizedPull(raw) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) fail('GitHub pull response is invalid');
  return {
    number: raw.number,
    state: raw.state,
    draft: raw.draft,
    mergeable: raw.mergeable,
    labels: Array.isArray(raw.labels) ? raw.labels.map((label) => ({ name: label?.name })) : raw.labels,
    head: {
      sha: raw.head?.sha,
      ref: raw.head?.ref,
      repo: normalizedRepository(raw.head?.repo),
    },
    base: {
      sha: raw.base?.sha,
      ref: raw.base?.ref,
      repo: normalizedRepository(raw.base?.repo),
    },
  };
}

function normalizedCheck(raw) {
  return {
    id: raw?.id,
    name: raw?.name,
    status: raw?.status,
    conclusion: raw?.conclusion,
    head_sha: raw?.head_sha,
    external_id: raw?.external_id,
    started_at: raw?.started_at,
    completed_at: raw?.completed_at,
    app: raw?.app == null ? null : { id: raw.app.id, slug: raw.app.slug },
    output: raw?.output == null ? null : {
      title: raw.output.title,
      summary: raw.output.summary,
    },
    html_url: raw?.html_url,
  };
}

function externalId(artifact) {
  return [
    'aeris-policy',
    'v1',
    artifact.repository_id,
    artifact.pull_number,
    artifact.head_sha,
    artifact.policy_sha,
  ].join(':');
}

function renderPendingCheck(generation) {
  return {
    title: 'Policy evaluation in progress',
    summary: [
      'Policy inputs are being revalidated.',
      `Head SHA: ${generation.head_sha}`,
      `Policy SHA: ${generation.policy_sha}`,
    ].join('\n'),
  };
}

function renderCheck(artifact) {
  const result = artifact.result;
  const title = `Policy ${result.mode}: ${result.verdict}`;
  const lines = [
    `Mode: ${result.mode}`,
    `Verdict: ${result.verdict}`,
    `Enforcement: ${result.enforcement}`,
    `Automatic merge eligible: ${result.eligible_for_automatic_merge ? 'yes' : 'no'}`,
    `Head SHA: ${artifact.head_sha}`,
    `Policy SHA: ${artifact.policy_sha}`,
    `Snapshot SHA: ${artifact.snapshot_sha}`,
    `Changed files: ${result.changed_file_count}`,
  ];
  if (result.reason_codes.length > 0) lines.push(`Reasons: ${result.reason_codes.join(', ')}`);
  if (result.unsuccessful_checks.length > 0) lines.push(`Unsuccessful checks: ${result.unsuccessful_checks.join(', ')}`);
  if (result.human_review_paths.length > 0) lines.push(`Human-review paths: ${result.human_review_paths.join(', ')}`);
  const summary = lines.join('\n');
  requireCondition(Buffer.byteLength(title, 'utf8') <= 255, 'Policy check title exceeds the configured limit');
  requireCondition(Buffer.byteLength(summary, 'utf8') <= 32_768, 'Policy check summary exceeds the configured limit');
  return { title, summary };
}

export class PolicyGitHubClient {
  #token;
  #repository;
  #repositoryId;
  #owner;
  #name;
  #policyApp;
  #fetchImpl;

  constructor(options = {}) {
    requireCondition(options && typeof options === 'object' && !Array.isArray(options), 'Policy GitHub client options are invalid');
    const allowed = new Set(['token', 'repository', 'repositoryId', 'policyApp', 'fetchImpl']);
    requireCondition(Object.keys(options).every((key) => allowed.has(key)), 'Policy GitHub client options have unexpected keys');
    string(options.token, 'GitHub token', MAX_TOKEN_BYTES);
    const { owner, name } = validateRepository(options.repository);
    positiveInteger(options.repositoryId, 'GitHub repository ID');
    if (options.policyApp !== null && options.policyApp !== undefined) validateApp(options.policyApp);
    requireCondition(typeof (options.fetchImpl ?? globalThis.fetch) === 'function', 'GitHub fetch implementation is invalid');
    this.#token = options.token;
    this.#repository = options.repository;
    this.#repositoryId = options.repositoryId;
    this.#owner = owner;
    this.#name = name;
    this.#policyApp = options.policyApp == null ? null : validateApp(options.policyApp);
    this.#fetchImpl = options.fetchImpl ?? globalThis.fetch;
  }

  async #request(method, path, body = undefined) {
    requireCondition(typeof path === 'string' && path.startsWith('/') && !path.startsWith('//'), 'GitHub API path is invalid');
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
    const text = await boundedText(response);
    if (!response.ok) throw new PolicyGitHubApiError(`GitHub API returned HTTP ${response.status}`, response.status);
    if (!text) return null;
    try {
      return JSON.parse(text);
    } catch {
      throw new PolicyGitHubApiError('GitHub API returned invalid JSON', response.status);
    }
  }

  async #list(path, maximumPages) {
    const items = [];
    for (let page = 1; page <= maximumPages; page += 1) {
      const separator = path.includes('?') ? '&' : '?';
      const batch = await this.#request('GET', `${path}${separator}per_page=${PAGE_SIZE}&page=${page}`);
      if (!Array.isArray(batch)) fail('GitHub list response is invalid');
      items.push(...batch);
      if (batch.length < PAGE_SIZE) return { items, truncated: false };
    }
    return { items, truncated: true };
  }

  async getPull(number) {
    positiveInteger(number, 'Pull request number');
    const pull = normalizedPull(await this.#request('GET', `/repos/${this.#repository}/pulls/${number}`));
    requireCondition(pull.number === number, 'GitHub pull response is not bound to the requested pull request number');
    return pull;
  }

  async listPullFiles(number) {
    positiveInteger(number, 'Pull request number');
    const response = await this.#list(`/repos/${this.#repository}/pulls/${number}/files`, 3);
    return {
      files: response.items.map((file) => ({
        filename: file?.filename,
        status: file?.status,
        previous_filename: file?.previous_filename ?? null,
      })),
      truncated: response.truncated,
    };
  }

  async listCheckRunsForRef(ref) {
    string(ref, 'Check ref', 80, SHA);
    const checkRuns = [];
    for (let page = 1; page <= 10; page += 1) {
      const response = await this.#request(
        'GET',
        `/repos/${this.#repository}/commits/${ref}/check-runs?filter=all&per_page=${PAGE_SIZE}&page=${page}`,
      );
      if (!response || !Array.isArray(response.check_runs)) fail('GitHub check-runs response is invalid');
      checkRuns.push(...response.check_runs.map(normalizedCheck));
      if (response.check_runs.length < PAGE_SIZE) return checkRuns;
    }
    fail('GitHub check-runs response exceeds the configured page limit');
  }

  async compare(baseSha, headSha) {
    string(baseSha, 'Comparison base SHA', 40, SHA);
    string(headSha, 'Comparison head SHA', 40, SHA);
    const response = await this.#request('GET', `/repos/${this.#repository}/compare/${baseSha}...${headSha}`);
    if (!response || typeof response !== 'object' || Array.isArray(response)) fail('GitHub comparison response is invalid');
    const responseBase = response.base_commit?.sha;
    const responseHead = response.commits?.at?.(-1)?.sha ?? (response.status === 'identical' ? responseBase : null);
    if (responseBase !== baseSha || responseHead !== headSha) fail('GitHub comparison response is not bound to the requested SHAs');
    return { base_sha: baseSha, head_sha: headSha, status: response.status ?? 'unknown' };
  }

  async listReviewThreads(number) {
    positiveInteger(number, 'Pull request number');
    let cursor = null;
    let unresolved = 0;
    for (let page = 1; page <= 3; page += 1) {
      const query = `query($owner:String!,$name:String!,$number:Int!,$cursor:String){repository(owner:$owner,name:$name){databaseId pullRequest(number:$number){number headRefOid baseRefOid reviewThreads(first:100,after:$cursor){nodes{isResolved}pageInfo{hasNextPage endCursor}}}}}`;
      const response = await this.#request('POST', '/graphql', {
        query,
        variables: { owner: this.#owner, name: this.#name, number, cursor },
      });
      if (Array.isArray(response?.errors) && response.errors.length > 0) fail('GitHub GraphQL returned errors');
      const repository = response?.data?.repository;
      const pull = repository?.pullRequest;
      const threads = pull?.reviewThreads;
      if (repository?.databaseId !== this.#repositoryId || pull?.number !== number || !Array.isArray(threads?.nodes)) {
        fail('GitHub review-threads response is invalid');
      }
      for (const thread of threads.nodes) {
        if (typeof thread?.isResolved !== 'boolean') fail('GitHub review thread is invalid');
        if (!thread.isResolved) unresolved += 1;
      }
      if (threads.pageInfo?.hasNextPage === false) {
        return {
          unresolved,
          truncated: false,
          head_sha: pull.headRefOid,
          base_sha: pull.baseRefOid,
        };
      }
      if (threads.pageInfo?.hasNextPage !== true || typeof threads.pageInfo.endCursor !== 'string') {
        fail('GitHub review-threads pagination is invalid');
      }
      cursor = threads.pageInfo.endCursor;
    }
    return { unresolved, truncated: true, head_sha: null, base_sha: null };
  }

  async getRepository() {
    const response = await this.#request('GET', `/repos/${this.#repository}`);
    if (response?.id !== this.#repositoryId || response?.full_name !== this.#repository) {
      fail('GitHub repository identity does not match the trusted configuration');
    }
    return { id: response.id, full_name: response.full_name, default_branch: response.default_branch };
  }

  async getBranchHead(branch) {
    string(branch, 'Branch name', 255, /^[A-Za-z0-9._/-]+$/);
    requireCondition(!branch.startsWith('/') && !branch.endsWith('/') && !branch.includes('..'), 'Branch name is invalid');
    const response = await this.#request('GET', `/repos/${this.#repository}/git/ref/heads/${encodeURIComponent(branch)}`);
    if (response?.ref !== `refs/heads/${branch}` || typeof response?.object?.sha !== 'string' || !SHA.test(response.object.sha)) {
      fail('GitHub branch response is invalid');
    }
    return response.object.sha;
  }

  async #getCheckRun(id) {
    positiveInteger(id, 'Check run ID');
    return normalizedCheck(await this.#request('GET', `/repos/${this.#repository}/check-runs/${id}`));
  }

  #verifyPersistedCheck(check, expected) {
    if (!this.#policyApp) fail('Policy App identity is required to publish checks');
    requireCondition(check.id === expected.id, 'Persisted policy check ID changed');
    requireCondition(check.name === expected.name, 'Persisted policy check name changed');
    requireCondition(check.head_sha === expected.head_sha, 'Persisted policy check head SHA changed');
    requireCondition(check.external_id === expected.external_id, 'Persisted policy check external ID changed');
    requireCondition(check.status === expected.status, 'Persisted policy check status changed');
    if (expected.status === 'in_progress') {
      requireCondition(check.conclusion === null, 'Persisted in-progress policy check has a conclusion');
    } else {
      requireCondition(check.status === 'completed' && check.conclusion === expected.conclusion, 'Persisted policy check result changed');
    }
    requireCondition(
      check.app?.id === this.#policyApp.id && check.app?.slug === this.#policyApp.slug,
      'Persisted policy check is not owned by the Policy App',
    );
    requireCondition(
      check.output?.title === expected.output.title && check.output?.summary === expected.output.summary,
      'Persisted policy check output changed',
    );
    return check;
  }

  async beginPolicyCheck(rawGeneration, checkName, detailsUrl = null) {
    if (!this.#policyApp) fail('Policy App identity is required to publish checks');
    const generation = validateGeneration(rawGeneration);
    requireCondition(
      generation.repository === this.#repository && generation.repository_id === this.#repositoryId,
      'Policy generation repository binding is invalid',
    );
    string(checkName, 'Policy check name', 160);
    validateDetailsUrl(detailsUrl);
    const liveBefore = await this.getPull(generation.pull_number);
    requireCondition(
      liveBefore.state === 'open' && liveBefore.head.sha === generation.head_sha && liveBefore.base.sha === generation.base_sha,
      'Policy generation is stale before check publication',
    );

    const id = externalId(generation);
    const output = renderPendingCheck(generation);
    const allChecks = await this.listCheckRunsForRef(generation.head_sha);
    const impersonators = allChecks.filter((check) => check.name === checkName && check.external_id === id && (
      check.app?.id !== this.#policyApp.id || check.app?.slug !== this.#policyApp.slug
    ));
    requireCondition(impersonators.length === 0, 'A non-Policy App check reused the managed external ID');
    const existing = allChecks.filter((check) => check.name === checkName && check.external_id === id && (
      check.app?.id === this.#policyApp.id && check.app?.slug === this.#policyApp.slug
    ));
    requireCondition(existing.length <= 1, 'Multiple managed policy checks exist for the same generation');

    const body = {
      name: checkName,
      head_sha: generation.head_sha,
      status: 'in_progress',
      external_id: id,
      output,
      ...(detailsUrl === null ? {} : { details_url: detailsUrl }),
    };
    let response;
    if (existing.length === 1) {
      const { head_sha: ignored, ...update } = body;
      void ignored;
      response = await this.#request('PATCH', `/repos/${this.#repository}/check-runs/${existing[0].id}`, update);
    } else {
      response = await this.#request('POST', `/repos/${this.#repository}/check-runs`, body);
    }
    const created = normalizedCheck(response);
    positiveInteger(created.id, 'Published check run ID');
    const expected = {
      id: created.id,
      name: checkName,
      head_sha: generation.head_sha,
      external_id: id,
      status: 'in_progress',
      conclusion: null,
      output,
    };
    const persisted = this.#verifyPersistedCheck(await this.#getCheckRun(created.id), expected);
    const liveAfter = await this.getPull(generation.pull_number);
    requireCondition(
      liveAfter.state === 'open' && liveAfter.head.sha === generation.head_sha && liveAfter.base.sha === generation.base_sha,
      'Pull request changed during policy check publication',
    );
    return persisted;
  }

  async restorePolicyCheckInProgress(checkRunId, rawGeneration, checkName, detailsUrl = null) {
    if (!this.#policyApp) fail('Policy App identity is required to publish checks');
    positiveInteger(checkRunId, 'Policy check run ID');
    const generation = validateGeneration(rawGeneration);
    requireCondition(
      generation.repository === this.#repository && generation.repository_id === this.#repositoryId,
      'Policy generation repository binding is invalid',
    );
    string(checkName, 'Policy check name', 160);
    validateDetailsUrl(detailsUrl);
    const id = externalId(generation);
    const output = renderPendingCheck(generation);
    const existing = await this.#getCheckRun(checkRunId);
    requireCondition(
      existing.id === checkRunId && existing.name === checkName && existing.head_sha === generation.head_sha &&
        existing.external_id === id && existing.app?.id === this.#policyApp.id && existing.app?.slug === this.#policyApp.slug &&
        ((existing.status === 'in_progress' && existing.conclusion === null) ||
          (existing.status === 'completed' && typeof existing.conclusion === 'string')),
      'Policy check is not the expected recoverable App-owned generation',
    );
    const response = normalizedCheck(await this.#request('PATCH', `/repos/${this.#repository}/check-runs/${checkRunId}`, {
      name: checkName,
      status: 'in_progress',
      external_id: id,
      output,
      ...(detailsUrl === null ? {} : { details_url: detailsUrl }),
    }));
    requireCondition(response.id === checkRunId, 'Restored policy check response ID changed');
    return this.#verifyPersistedCheck(await this.#getCheckRun(checkRunId), {
      id: checkRunId,
      name: checkName,
      head_sha: generation.head_sha,
      external_id: id,
      status: 'in_progress',
      conclusion: null,
      output,
    });
  }

  async completePolicyCheck(checkRunId, rawArtifact, checkName, detailsUrl = null) {
    if (!this.#policyApp) fail('Policy App identity is required to publish checks');
    positiveInteger(checkRunId, 'Policy check run ID');
    const artifact = validatePolicyEvaluationArtifact(rawArtifact);
    const generation = generationFromArtifact(artifact);
    requireCondition(
      artifact.repository === this.#repository && artifact.repository_id === this.#repositoryId,
      'Policy artifact repository binding is invalid',
    );
    string(checkName, 'Policy check name', 160);
    validateDetailsUrl(detailsUrl);
    const id = externalId(generation);
    const pending = await this.#getCheckRun(checkRunId);
    requireCondition(
      pending.id === checkRunId && pending.name === checkName && pending.head_sha === artifact.head_sha &&
        pending.external_id === id && pending.status === 'in_progress' && pending.conclusion === null &&
        pending.app?.id === this.#policyApp.id && pending.app?.slug === this.#policyApp.slug,
      'Policy check is not the expected in-progress App-owned generation',
    );
    const liveBefore = await this.getPull(artifact.pull_number);
    requireCondition(
      liveBefore.state === 'open' && liveBefore.head.sha === artifact.head_sha && liveBefore.base.sha === artifact.base_sha,
      'Policy artifact is stale before check completion',
    );
    const output = renderCheck(artifact);
    const conclusion = policyCheckConclusion(artifact.result);
    const response = normalizedCheck(await this.#request('PATCH', `/repos/${this.#repository}/check-runs/${checkRunId}`, {
      name: checkName,
      status: 'completed',
      conclusion,
      external_id: id,
      output,
      ...(detailsUrl === null ? {} : { details_url: detailsUrl }),
    }));
    requireCondition(response.id === checkRunId, 'Completed policy check response ID changed');
    const expected = {
      id: checkRunId,
      name: checkName,
      head_sha: artifact.head_sha,
      external_id: id,
      status: 'completed',
      conclusion,
      output,
    };
    const persisted = this.#verifyPersistedCheck(await this.#getCheckRun(checkRunId), expected);
    const liveAfter = await this.getPull(artifact.pull_number);
    requireCondition(
      liveAfter.state === 'open' && liveAfter.head.sha === artifact.head_sha && liveAfter.base.sha === artifact.base_sha,
      'Pull request changed during policy check completion',
    );
    return persisted;
  }
}
