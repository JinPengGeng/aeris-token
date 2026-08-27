import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import { executorDescriptorForRoute, validateExecutorRegistry } from './ai-executor-contract.mjs';
import { validateCandidateArtifact } from './autonomy-candidate.mjs';
import { GitHubApiError, GitHubClient } from './github-client.mjs';

const MANAGED_MARKER = '<!-- aeris-autonomy-managed -->';
const SHA = /^[0-9a-f]{40}$/;
const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const WRITER_LOGIN = /^[A-Za-z0-9][A-Za-z0-9-]{0,99}\[bot\]$/;
const DEFAULT_GIT_TIMEOUT_MS = 30_000;
const DEFAULT_REST_TIMEOUT_MS = 30_000;
const MAX_TIMEOUT_MS = 120_000;
const MAXIMUM_RESPONSE_BYTES = 4 * 1024 * 1024;
const MAXIMUM_EXECUTOR_REGISTRY_BYTES = 65_536;
const EXECUTOR_REGISTRY_PATH = '.github/ai-executors.json';

export class AutonomyPublisherError extends Error {
  constructor(message) {
    super(message);
    this.name = 'AutonomyPublisherError';
  }
}

function reject(message) {
  throw new AutonomyPublisherError(message);
}

function required(value, name, pattern = null) {
  if (typeof value !== 'string' || value.length === 0 || /[\u0000-\u001f\u007f]/.test(value)) {
    reject(`${name} is invalid`);
  }
  if (pattern && !pattern.test(value)) reject(`${name} format is invalid`);
  return value;
}

function positiveInteger(value, name) {
  const parsed = typeof value === 'string' && /^[1-9][0-9]*$/.test(value) ? Number(value) : value;
  if (!Number.isSafeInteger(parsed) || parsed <= 0) reject(`${name} must be a positive integer`);
  return parsed;
}

function boundedTimeout(value, name, defaultValue) {
  if (value === undefined || value === '') return defaultValue;
  const timeout = positiveInteger(value, name);
  if (timeout > MAX_TIMEOUT_MS) reject(`${name} must not exceed ${MAX_TIMEOUT_MS}`);
  return timeout;
}

function expectedFromEnvironment(environment) {
  const issueNumber = positiveInteger(environment.AERIS_ISSUE_NUMBER, 'AERIS_ISSUE_NUMBER');
  return Object.freeze({
    repository: required(environment.GITHUB_REPOSITORY, 'GITHUB_REPOSITORY', REPOSITORY),
    repository_id: positiveInteger(environment.GITHUB_REPOSITORY_ID, 'GITHUB_REPOSITORY_ID'),
    task_id: `issue:${issueNumber}`,
    issue_number: issueNumber,
    base_ref: required(environment.AERIS_BASE_REF, 'AERIS_BASE_REF', /^refs\/heads\/main$/),
    base_sha: required(environment.AERIS_BASE_SHA, 'AERIS_BASE_SHA', SHA),
    trigger_run_id: required(environment.AERIS_TRIGGER_RUN_ID ?? environment.GITHUB_RUN_ID, 'AERIS_TRIGGER_RUN_ID', /^(?:0|[1-9][0-9]*)$/),
    trigger_run_attempt: positiveInteger(environment.AERIS_TRIGGER_RUN_ATTEMPT ?? environment.GITHUB_RUN_ATTEMPT, 'AERIS_TRIGGER_RUN_ATTEMPT'),
  });
}

function readArtifact(manifestPath, patchPath, expected) {
  let manifest;
  try {
    manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  } catch {
    reject('candidate manifest is missing or invalid JSON');
  }
  let patch;
  try {
    patch = fs.readFileSync(patchPath);
  } catch {
    reject('candidate patch is missing');
  }
  return { manifest, patch, verified: validateCandidateArtifact({ manifest, patch, expected }) };
}

function command(repositoryRoot, args, options = {}) {
  const environment = { ...process.env, ...options.env };
  delete environment.AERIS_WRITER_TOKEN;
  delete environment.GIT_ASKPASS;
  delete environment.GIT_ASKPASS_REQUIRE;
  if (options.writerToken !== undefined) environment.AERIS_WRITER_TOKEN = options.writerToken;
  if (options.askpass !== undefined) {
    environment.GIT_ASKPASS = options.askpass;
    environment.GIT_ASKPASS_REQUIRE = 'force';
  }
  try {
    return (options.execFileImpl ?? execFileSync)('git', args, {
      cwd: repositoryRoot,
      encoding: options.encoding ?? 'utf8',
      env: { ...environment, GIT_TERMINAL_PROMPT: '0', GIT_CONFIG_NOSYSTEM: '1' },
      maxBuffer: 4 * 1024 * 1024,
      stdio: ['ignore', 'pipe', 'pipe'],
      timeout: options.timeoutMs ?? DEFAULT_GIT_TIMEOUT_MS,
      killSignal: 'SIGKILL',
    });
  } catch (error) {
    const stderr = Buffer.isBuffer(error?.stderr) ? error.stderr.toString('utf8').trim() : String(error?.stderr ?? '').trim();
    const timedOut = error?.code === 'ETIMEDOUT' || error?.signal === 'SIGKILL';
    reject(`git ${args[0]} ${timedOut ? 'timed out' : 'failed'}${stderr ? `: ${stderr}` : ''}`);
  }
}

function stagedPaths(repositoryRoot, options) {
  const output = command(repositoryRoot, ['diff', '--cached', '--name-only', '-z', 'HEAD', '--'], {
    ...options,
    encoding: 'buffer',
  });
  return output.toString('utf8').split('\0').filter(Boolean);
}

function executorProvenance(executor) {
  return [
    `Aeris-Autonomy-Executor-ID: ${executor.id}`,
    `Aeris-Autonomy-Executor-Protocol: ${executor.protocol}`,
    `Aeris-Autonomy-Executor-Action-SHA: ${executor.action_sha}`,
    `Aeris-Autonomy-Executor-Tool-Version: ${executor.tool_version}`,
  ].join('\n');
}

export class LocalGitPublisher {
  constructor({ repositoryRoot, token, repository, gitTimeoutMs = DEFAULT_GIT_TIMEOUT_MS, execFileImpl = execFileSync }) {
    this.repositoryRoot = path.resolve(repositoryRoot);
    this.token = token;
    this.repository = repository;
    this.gitTimeoutMs = boundedTimeout(gitTimeoutMs, 'gitTimeoutMs', DEFAULT_GIT_TIMEOUT_MS);
    this.execFileImpl = execFileImpl;
  }

  command(args, options = {}) {
    return command(this.repositoryRoot, args, {
      ...options,
      timeoutMs: this.gitTimeoutMs,
      execFileImpl: this.execFileImpl,
    });
  }

  verifyBase(baseSha) {
    const head = this.command(['rev-parse', 'HEAD']).trim();
    if (head !== baseSha) reject('publisher checkout does not match candidate base SHA');
    const dirty = this.command(['status', '--porcelain=v1', '--untracked-files=all']).trim();
    if (dirty) reject('publisher checkout is not clean before applying the candidate');
  }

  trustedCandidateExecutorAtBase(baseSha) {
    this.verifyBase(baseSha);
    const tree = this.command(['ls-tree', '-z', baseSha, '--', EXECUTOR_REGISTRY_PATH], {
      encoding: 'buffer',
    });
    const treeText = tree.toString('utf8');
    if (!Buffer.from(treeText, 'utf8').equals(tree)) reject('trusted candidate executor registry is not UTF-8');
    const entry = /^100644 blob ([0-9a-f]{40})\t\.github\/ai-executors\.json\0$/.exec(treeText);
    if (!entry) reject('trusted candidate executor registry is missing or not a regular blob');
    const blob = this.command(['cat-file', 'blob', entry[1]], { encoding: 'buffer' });
    if (blob.length === 0 || blob.length > MAXIMUM_EXECUTOR_REGISTRY_BYTES) {
      reject('trusted candidate executor registry size is invalid');
    }
    const text = blob.toString('utf8');
    if (!Buffer.from(text, 'utf8').equals(blob)) reject('trusted candidate executor registry is not UTF-8');
    let registry;
    try {
      registry = JSON.parse(text);
    } catch {
      reject('trusted candidate executor registry is invalid JSON');
    }
    try {
      return executorDescriptorForRoute(validateExecutorRegistry(registry), 'candidate');
    } catch {
      reject('trusted candidate executor registry is invalid');
    }
  }

  prepareCommit({ patchPath, verified }) {
    this.verifyBase(verified.manifest.base_sha);
    this.command(['apply', '--check', '--index', '--whitespace=error-all', '--', patchPath]);
    this.command(['apply', '--index', '--whitespace=error-all', '--', patchPath]);
    this.command(['diff', '--cached', '--check']);
    const allowed = new Set(verified.paths);
    const actual = stagedPaths(this.repositoryRoot, {
      timeoutMs: this.gitTimeoutMs,
      execFileImpl: this.execFileImpl,
    });
    if (actual.length !== allowed.size || actual.some((candidate) => !allowed.has(candidate))) {
      reject('applied index paths do not match the verified candidate');
    }
    const manifest = verified.manifest;
    const subject = `chore(autonomy): update issue #${manifest.issue_number}`;
    const body = [
      'Aeris-Autonomy-Managed: true',
      `Aeris-Autonomy-Task: ${manifest.task_id}`,
      `Aeris-Autonomy-Patch: ${manifest.patch_sha256}`,
      `Aeris-Autonomy-Base: ${manifest.base_sha}`,
      `Aeris-Autonomy-Run: ${manifest.trigger_run_id}/${manifest.trigger_run_attempt}`,
      executorProvenance(manifest.executor),
    ].join('\n');
    this.command([
      '-c', 'user.name=aeris-autonomy[bot]',
      '-c', 'user.email=aeris-autonomy[bot]@users.noreply.github.com',
      '-c', 'commit.gpgSign=false',
      '-c', 'core.hooksPath=/dev/null',
      'commit', '--no-gpg-sign', '-m', subject, '-m', body,
    ], {
      env: {
        GIT_AUTHOR_DATE: manifest.created_at,
        GIT_COMMITTER_DATE: manifest.created_at,
      },
    });
    const sha = this.command(['rev-parse', 'HEAD']).trim();
    const tree = this.command(['rev-parse', 'HEAD^{tree}']).trim();
    if (!SHA.test(sha) || !SHA.test(tree)) reject('candidate commit identity is invalid');
    return Object.freeze({ sha, tree });
  }

  push(branch, expectedOldSha = null) {
    if (!this.token) reject('Writer token is not configured');
    const askpassDirectory = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-writer-askpass-'));
    const askpass = path.join(askpassDirectory, 'askpass.sh');
    fs.writeFileSync(askpass, '#!/usr/bin/env bash\ncase "$1" in *Username*) printf "%s\\n" x-access-token ;; *) printf "%s\\n" "$AERIS_WRITER_TOKEN" ;; esac\n', { mode: 0o700 });
    const lease = `--force-with-lease=refs/heads/${branch}:${expectedOldSha ?? ''}`;
    try {
      this.command([
        '-c', 'credential.helper=',
        '-c', 'http.https://github.com/.extraheader=',
        'push',
        lease,
        `https://github.com/${this.repository}.git`,
        `HEAD:refs/heads/${branch}`,
      ], {
        writerToken: this.token,
        askpass,
      });
    } finally {
      fs.rmSync(askpassDirectory, { recursive: true, force: true });
    }
  }
}

function bindExpectedCandidateExecutor({ expected, repositoryRoot, gitTimeoutMs = DEFAULT_GIT_TIMEOUT_MS, execFileImpl = execFileSync }) {
  const verifier = new LocalGitPublisher({
    repositoryRoot,
    token: null,
    repository: expected.repository,
    gitTimeoutMs,
    execFileImpl,
  });
  const executor = verifier.trustedCandidateExecutorAtBase(expected.base_sha);
  return Object.freeze({
    expected: Object.freeze({ ...expected, executor }),
    verifier,
  });
}

export function trustedCandidateExecutorForBase({ repositoryRoot, baseSha, repository, gitTimeoutMs = DEFAULT_GIT_TIMEOUT_MS, execFileImpl = execFileSync }) {
  const verifier = new LocalGitPublisher({ repositoryRoot, token: null, repository, gitTimeoutMs, execFileImpl });
  return verifier.trustedCandidateExecutorAtBase(required(baseSha, 'baseSha', SHA));
}

export class WriterGitHubClient extends GitHubClient {
  constructor({ requestTimeoutMs = DEFAULT_REST_TIMEOUT_MS, ...options }) {
    super(options);
    this.requestTimeoutMs = boundedTimeout(requestTimeoutMs, 'requestTimeoutMs', DEFAULT_REST_TIMEOUT_MS);
  }

  async request(method, requestPath, { body = undefined, accept = 'application/vnd.github+json' } = {}) {
    const controller = new AbortController();
    let timeout;
    const deadline = new Promise((_, rejectDeadline) => {
      timeout = setTimeout(() => {
        controller.abort();
        rejectDeadline(new GitHubApiError('GitHub API request timed out'));
      }, this.requestTimeoutMs);
    });
    const operation = async () => {
      const response = await this.fetchImpl(`${this.apiUrl}${requestPath}`, {
        method,
        signal: controller.signal,
        headers: {
          accept,
          authorization: `Bearer ${this.token}`,
          'content-type': 'application/json',
          'x-github-api-version': '2022-11-28',
        },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      const text = response.status === 204 ? '' : await response.text();
      if (Buffer.byteLength(text, 'utf8') > MAXIMUM_RESPONSE_BYTES) {
        throw new GitHubApiError('GitHub API response exceeds the configured limit');
      }
      if (!response.ok) throw new GitHubApiError(`GitHub API returned HTTP ${response.status}`, response.status);
      if (!text) return null;
      try {
        return JSON.parse(text);
      } catch {
        throw new GitHubApiError('GitHub API returned invalid JSON', response.status);
      }
    };
    try {
      return await Promise.race([operation(), deadline]);
    } catch (error) {
      if (controller.signal.aborted) throw new GitHubApiError('GitHub API request timed out');
      throw error;
    } finally {
      clearTimeout(timeout);
    }
  }

  async getBranch(branch) {
    try {
      return await this.request('GET', `/repos/${this.repository}/git/ref/heads/${encodeURIComponent(branch)}`);
    } catch (error) {
      if (error instanceof GitHubApiError && error.status === 404) return null;
      throw error;
    }
  }

  getCommit(sha) {
    if (!SHA.test(sha)) reject('commit SHA is invalid');
    return this.request('GET', `/repos/${this.repository}/git/commits/${sha}`);
  }

  async listBranchPulls(owner, branch) {
    const result = await this.list(
      `/repos/${this.repository}/pulls?state=all&sort=created&direction=asc&head=${encodeURIComponent(`${owner}:${branch}`)}`,
      2,
    );
    if (result.truncated) reject('managed branch has too many historical pull requests');
    return result.items;
  }

  createPull(body) {
    return this.request('POST', `/repos/${this.repository}/pulls`, { body });
  }

  updatePull(number, body) {
    return this.request('PATCH', `/repos/${this.repository}/pulls/${number}`, { body });
  }
}

function managedBody(manifest, runUrl) {
  return `${MANAGED_MARKER}
<!-- aeris-autonomy-task:${manifest.task_id} -->
<!-- aeris-autonomy-patch:${manifest.patch_sha256} -->
<!-- aeris-autonomy-executor-id:${manifest.executor.id} -->
<!-- aeris-autonomy-executor-protocol:${manifest.executor.protocol} -->
<!-- aeris-autonomy-executor-action-sha:${manifest.executor.action_sha} -->
<!-- aeris-autonomy-executor-tool-version:${manifest.executor.tool_version} -->
Candidate generated for #${manifest.issue_number} from base \`${manifest.base_sha}\`.

- Agent run: ${runUrl}
- Artifact digest: \`${manifest.patch_sha256}\`
- Candidate executor: \`${manifest.executor.id}\` (\`${manifest.executor.protocol}\`)

This pull request remains draft until deterministic Policy marks it eligible.`;
}

function validateManagedPull(pull, { repository, branch, baseBranch, writerLogin, taskId }) {
  if (pull?.state !== 'open' || pull?.base?.ref !== baseBranch || pull?.head?.ref !== branch) {
    reject('managed pull request refs or state are invalid');
  }
  if (pull?.head?.repo?.full_name !== repository || pull?.user?.login !== writerLogin) {
    reject('managed pull request identity is invalid');
  }
  if (pull?.draft !== true || pull?.auto_merge !== null) {
    reject('managed pull request must remain draft with auto-merge disabled before update');
  }
  if (typeof pull?.body !== 'string' || !pull.body.includes(MANAGED_MARKER) || !pull.body.includes(`aeris-autonomy-task:${taskId}`)) {
    reject('managed pull request ownership marker is invalid');
  }
  if (!SHA.test(pull?.head?.sha ?? '')) reject('managed pull request head SHA is invalid');
}

function branchSha(branchRef, branch, { allowMissing = false } = {}) {
  if (branchRef === null && allowMissing) return null;
  if (
    branchRef?.ref !== `refs/heads/${branch}` ||
    branchRef?.object?.type !== 'commit' ||
    !SHA.test(branchRef?.object?.sha ?? '')
  ) {
    reject('managed branch ref is invalid');
  }
  return branchRef.object.sha;
}

function assertExpectedBase(branchRef, baseBranch, expectedSha) {
  if (branchSha(branchRef, baseBranch) !== expectedSha) reject('base branch drifted during publication');
}

function validatePullHistory(pulls, { repository, branch }) {
  if (!Array.isArray(pulls)) reject('managed pull request history is incomplete');
  const numbers = new Set();
  for (const pull of pulls) {
    if (
      !Number.isSafeInteger(pull?.number) || pull.number <= 0 ||
      numbers.has(pull.number) ||
      !['open', 'closed'].includes(pull?.state) ||
      pull?.head?.ref !== branch ||
      pull?.head?.repo?.full_name !== repository
    ) {
      reject('managed pull request history is incomplete');
    }
    numbers.add(pull.number);
  }
}

function validatePublishedPull(pull, { repository, branch, baseBranch, writerLogin, taskId, commitSha, title, body }) {
  validateManagedPull(pull, { repository, branch, baseBranch, writerLogin, taskId });
  if (pull.head.sha !== commitSha || pull.title !== title || pull.body !== body) {
    reject('GitHub did not persist the exact published candidate');
  }
  const expectedUrl = `https://github.com/${repository}/pull/${pull.number}`;
  if (!Number.isSafeInteger(pull.number) || pull.number <= 0 || pull.html_url !== expectedUrl) {
    reject('published pull request identity is invalid');
  }
}

function validatePublishedCommit(commit, { sha, tree, manifest }) {
  if (commit?.sha !== sha || commit?.tree?.sha !== tree || typeof commit?.message !== 'string') {
    reject('GitHub did not persist the exact managed candidate commit');
  }
  if (!commit.message.includes(executorProvenance(manifest.executor))) {
    reject('GitHub did not persist candidate executor provenance');
  }
}

export async function publishCandidate({ artifact, expected, client, gitPublisher, writerLogin, runUrl }) {
  const repository = expected.repository;
  const [owner] = repository.split('/');
  const branch = `agent/issue-${expected.issue_number}`;
  const baseBranch = expected.base_ref.slice('refs/heads/'.length);
  if (!WRITER_LOGIN.test(writerLogin)) reject('Writer App bot login is invalid');
  const { verified } = artifact;
  const commit = gitPublisher.prepareCommit({ patchPath: artifact.patchPath, verified });
  const [baseRef, branchRef, pulls] = await Promise.all([
    client.getBranch(baseBranch),
    client.getBranch(branch),
    client.listBranchPulls(owner, branch),
  ]);
  assertExpectedBase(baseRef, baseBranch, expected.base_sha);
  validatePullHistory(pulls, { repository, branch });
  const open = pulls.filter((pull) => pull.state === 'open');
  if (open.length > 1) reject('managed branch has multiple open pull requests');
  if (pulls.some((pull) => pull.state === 'closed')) {
    reject('managed branch is tombstoned by a closed pull request');
  }
  if (open.length === 1) validateManagedPull(open[0], { repository, branch, baseBranch, writerLogin, taskId: expected.task_id });

  const remoteSha = branchSha(branchRef, branch, { allowMissing: true });
  if (open.length === 1 && open[0].head.sha !== remoteSha) reject('managed branch and pull request heads disagree');
  if (open.length === 0 && remoteSha !== null && remoteSha !== commit.sha) {
    reject('unowned managed branch already exists');
  }
  if (remoteSha !== commit.sha) {
    gitPublisher.push(branch, remoteSha);
    const pushedSha = branchSha(await client.getBranch(branch), branch);
    if (pushedSha !== commit.sha) reject('GitHub did not persist the exact managed branch SHA');
  }
  validatePublishedCommit(await client.getCommit(commit.sha), {
    sha: commit.sha,
    tree: commit.tree,
    manifest: verified.manifest,
  });

  const body = managedBody(verified.manifest, runUrl);
  const title = `chore(autonomy): implement #${expected.issue_number}`;
  assertExpectedBase(await client.getBranch(baseBranch), baseBranch, expected.base_sha);
  let mutation;
  if (open.length === 1) {
    mutation = await client.updatePull(open[0].number, { title, body });
  } else {
    mutation = await client.createPull({
      title,
      head: branch,
      base: baseBranch,
      body,
      draft: true,
    });
  }
  const pullNumber = positiveInteger(mutation?.number, 'published pull request number');
  const pull = await client.getPull(pullNumber);
  validatePublishedPull(pull, {
    repository,
    branch,
    baseBranch,
    writerLogin,
    taskId: expected.task_id,
    commitSha: commit.sha,
    title,
    body,
  });
  const [confirmedBase, confirmedBranch] = await Promise.all([
    client.getBranch(baseBranch),
    client.getBranch(branch),
  ]);
  assertExpectedBase(confirmedBase, baseBranch, expected.base_sha);
  const confirmedSha = branchSha(confirmedBranch, branch);
  if (confirmedSha !== commit.sha || pull.head.sha !== confirmedSha) {
    reject('managed branch and pull request heads disagree after publication');
  }
  return Object.freeze({
    branch,
    head_sha: commit.sha,
    pull_number: pullNumber,
    pull_url: pull.html_url,
    action: open.length === 1 ? 'updated' : 'created',
  });
}

export function verifyCandidateForPublication({ manifestPath, patchPath, expected, repositoryRoot, gitTimeoutMs = DEFAULT_GIT_TIMEOUT_MS }) {
  const bound = bindExpectedCandidateExecutor({ expected, repositoryRoot, gitTimeoutMs });
  const artifact = readArtifact(manifestPath, patchPath, bound.expected);
  const verifier = bound.verifier;
  verifier.command(['apply', '--check', '--index', '--whitespace=error-all', '--', patchPath]);
  return Object.freeze({ ...artifact, expected: bound.expected });
}

export async function runAutonomyPublisher(environment = process.env) {
  const expected = expectedFromEnvironment(environment);
  const manifestPath = required(environment.AERIS_CANDIDATE_MANIFEST, 'AERIS_CANDIDATE_MANIFEST');
  const patchPath = required(environment.AERIS_CANDIDATE_PATCH, 'AERIS_CANDIDATE_PATCH');
  const repositoryRoot = required(environment.GITHUB_WORKSPACE, 'GITHUB_WORKSPACE');
  const gitTimeoutMs = boundedTimeout(
    environment.AERIS_PUBLISHER_GIT_TIMEOUT_MS,
    'AERIS_PUBLISHER_GIT_TIMEOUT_MS',
    DEFAULT_GIT_TIMEOUT_MS,
  );
  const verifiedArtifact = verifyCandidateForPublication({ manifestPath, patchPath, expected, repositoryRoot, gitTimeoutMs });
  if (environment.AERIS_VERIFY_ONLY === 'true') {
    return Object.freeze({ verified: true, digest: verifiedArtifact.verified.manifest.patch_sha256 });
  }
  const token = required(environment.AERIS_WRITER_TOKEN, 'AERIS_WRITER_TOKEN');
  const slug = required(environment.AERIS_WRITER_APP_SLUG, 'AERIS_WRITER_APP_SLUG', /^[a-z0-9][a-z0-9-]{0,99}$/);
  const writerLogin = `${slug}[bot]`;
  const client = new WriterGitHubClient({
    token,
    repository: expected.repository,
    apiUrl: environment.GITHUB_API_URL,
    requestTimeoutMs: boundedTimeout(
      environment.AERIS_PUBLISHER_REST_TIMEOUT_MS,
      'AERIS_PUBLISHER_REST_TIMEOUT_MS',
      DEFAULT_REST_TIMEOUT_MS,
    ),
  });
  const gitPublisher = new LocalGitPublisher({
    repositoryRoot,
    token,
    repository: expected.repository,
    gitTimeoutMs,
  });
  const result = await publishCandidate({
    artifact: { ...verifiedArtifact, patchPath },
    expected: verifiedArtifact.expected,
    client,
    gitPublisher,
    writerLogin,
    runUrl: required(environment.AERIS_RUN_URL, 'AERIS_RUN_URL', /^https:\/\/github\.com\//),
  });
  if (environment.GITHUB_OUTPUT) {
    fs.appendFileSync(environment.GITHUB_OUTPUT, `pull_number=${result.pull_number}\npull_url=${result.pull_url}\nhead_sha=${result.head_sha}\naction=${result.action}\n`);
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  await runAutonomyPublisher();
}
