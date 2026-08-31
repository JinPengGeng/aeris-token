import { classifyAutonomyPolicy } from './autonomy-policy.mjs';
import { GitHubClient } from './github-client.mjs';

const SHA = /^[0-9a-f]{40}$/;
const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const CONTROL_CHARACTERS = /[\u0000-\u001f\u007f]/;
const DEFAULT_MAXIMUM_FILES = 100;
const DEFAULT_PAGE_SIZE = 100;
const DEFAULT_MAXIMUM_LABELS = 100;
const DEFAULT_LABEL_PAGE_SIZE = 100;

export class AutonomyPolicyRuntimeError extends Error {
  constructor(message) {
    super(message);
    this.name = 'AutonomyPolicyRuntimeError';
  }
}

function fail(message) {
  throw new AutonomyPolicyRuntimeError(message);
}

function string(value, name, pattern = null, maximumLength = 512) {
  if (typeof value !== 'string' || value.length === 0 || value.length > maximumLength || CONTROL_CHARACTERS.test(value)) {
    fail(`${name} is invalid`);
  }
  if (pattern && !pattern.test(value)) fail(`${name} format is invalid`);
  return value;
}

function positiveInteger(value, name) {
  if (!Number.isSafeInteger(value) || value <= 0) fail(`${name} must be a positive safe integer`);
  return value;
}

function exactRepository(value, name) {
  return string(value, name, REPOSITORY, 256);
}

function exactSha(value, name) {
  return string(value, name, SHA, 40);
}

function branch(value, name) {
  const result = string(value, name, null, 255);
  if (result.startsWith('refs/') || result.startsWith('/') || result.endsWith('/') ||
      result.includes('..') || /[~^:?*[\\]/.test(result) || result.endsWith('.lock')) {
    fail(`${name} is invalid`);
  }
  return result;
}

function relativePath(value, name) {
  const result = string(value, name, null, 1024);
  if (result.startsWith('/') || /^[A-Za-z]:/.test(result) || result.includes('\\')) fail(`${name} is not relative`);
  const segments = result.split('/');
  if (segments.some((segment) => segment.length === 0 || segment === '.' || segment === '..')) {
    fail(`${name} escapes the repository`);
  }
  return result;
}

function safeCount(value, name) {
  if (!Number.isSafeInteger(value) || value < 0) fail(`${name} must be a non-negative safe integer`);
  return value;
}

function normalizeTrigger(trigger) {
  if (!trigger || typeof trigger !== 'object' || Array.isArray(trigger)) fail('policy trigger must be an object');
  return Object.freeze({
    pull_number: positiveInteger(trigger.pull_number, 'trigger pull_number'),
    head_sha: exactSha(trigger.head_sha, 'trigger head_sha'),
  });
}

function normalizeTrust(trust) {
  if (!trust || typeof trust !== 'object' || Array.isArray(trust)) fail('policy trust context must be an object');
  return Object.freeze({
    repository: exactRepository(trust.repository, 'trusted repository'),
    repository_id: positiveInteger(trust.repository_id, 'trusted repository_id'),
    default_branch: branch(trust.default_branch, 'trusted default_branch'),
    policy_ref: branch(trust.policy_ref, 'trusted policy_ref'),
    policy_sha: exactSha(trust.policy_sha, 'trusted policy_sha'),
  });
}

function normalizeLimits(limits = {}) {
  const maximumFiles = limits.maximumFiles ?? DEFAULT_MAXIMUM_FILES;
  const pageSize = limits.pageSize ?? DEFAULT_PAGE_SIZE;
  const maximumLabels = limits.maximumLabels ?? DEFAULT_MAXIMUM_LABELS;
  const labelPageSize = limits.labelPageSize ?? DEFAULT_LABEL_PAGE_SIZE;
  if (!Number.isSafeInteger(maximumFiles) || maximumFiles <= 0 || maximumFiles > DEFAULT_MAXIMUM_FILES) {
    fail(`maximumFiles must be between 1 and ${DEFAULT_MAXIMUM_FILES}`);
  }
  if (!Number.isSafeInteger(pageSize) || pageSize <= 0 || pageSize > DEFAULT_PAGE_SIZE) {
    fail(`pageSize must be between 1 and ${DEFAULT_PAGE_SIZE}`);
  }
  if (!Number.isSafeInteger(maximumLabels) || maximumLabels <= 0 || maximumLabels > DEFAULT_MAXIMUM_LABELS) {
    fail(`maximumLabels must be between 1 and ${DEFAULT_MAXIMUM_LABELS}`);
  }
  if (!Number.isSafeInteger(labelPageSize) || labelPageSize <= 0 || labelPageSize > DEFAULT_LABEL_PAGE_SIZE) {
    fail(`labelPageSize must be between 1 and ${DEFAULT_LABEL_PAGE_SIZE}`);
  }
  return Object.freeze({ maximumFiles, pageSize, maximumLabels, labelPageSize });
}

function pullIdentity(pull) {
  const number = positiveInteger(pull?.number, 'pull number');
  if (pull?.state !== 'open') fail('pull request is not open');
  if (pull?.head?.repo?.full_name === null || pull?.base?.repo?.full_name === null) fail('pull request repository is missing');
  return Object.freeze({
    number,
    author: string(pull?.user?.login, 'pull author', null, 256),
    head_ref: branch(pull?.head?.ref, 'pull head ref'),
    head_sha: exactSha(pull?.head?.sha, 'pull head sha'),
    head_repository: exactRepository(pull?.head?.repo?.full_name, 'pull head repository'),
    base_ref: branch(pull?.base?.ref, 'pull base ref'),
    base_sha: exactSha(pull?.base?.sha, 'pull base sha'),
    base_repository: exactRepository(pull?.base?.repo?.full_name, 'pull base repository'),
  });
}

function samePull(left, right) {
  return Object.keys(left).every((key) => left[key] === right[key]);
}

function validateRepository(repository, trust) {
  if (repository?.id !== trust.repository_id || repository?.full_name !== trust.repository) {
    fail('repository identity does not match the trusted context');
  }
  if (repository?.default_branch !== trust.default_branch) fail('repository default branch drifted');
  if (trust.policy_ref !== trust.default_branch) fail('policy implementation is not sourced from the default branch');
}

function validatePull(identity, trigger, trust, config) {
  if (identity.number !== trigger.pull_number) fail('pull request number does not match the trigger');
  if (identity.head_sha !== trigger.head_sha) fail('pull request head SHA does not match the trigger');
  if (identity.base_repository !== trust.repository) fail('pull request base repository is not trusted');
  if (identity.base_ref !== trust.default_branch || identity.base_ref !== config.base_ref) {
    fail('pull request base is not the trusted default branch');
  }
}

function normalizePullFile(file, index) {
  const name = `pull files[${index}]`;
  const status = string(file?.status, `${name}.status`, null, 32);
  return Object.freeze({
    filename: relativePath(file?.filename, `${name}.filename`),
    status,
    additions: safeCount(file?.additions, `${name}.additions`),
    deletions: safeCount(file?.deletions, `${name}.deletions`),
    changes: safeCount(file?.changes, `${name}.changes`),
    previous_filename: file?.previous_filename === undefined
      ? null
      : relativePath(file.previous_filename, `${name}.previous_filename`),
    binary: typeof file?.patch !== 'string',
  });
}

function normalizePullLabel(label, index) {
  const name = `pull labels[${index}]`;
  if (!label || typeof label !== 'object' || Array.isArray(label)) fail(`${name} must be an object`);
  return Object.freeze({
    id: positiveInteger(label.id, `${name}.id`),
    name: string(label.name, `${name}.name`, null, 50),
  });
}

function canonicalLabels(labels) {
  const ids = new Set();
  const names = new Set();
  for (const label of labels) {
    const foldedName = label.name.toLocaleLowerCase('en-US');
    if (ids.has(label.id)) fail('pull labels contain a duplicate id');
    if (names.has(foldedName)) fail('pull labels contain a duplicate name');
    ids.add(label.id);
    names.add(foldedName);
  }
  return Object.freeze([...labels].sort((left, right) => left.id - right.id || left.name.localeCompare(right.name, 'en-US')));
}

function sameLabels(left, right) {
  return left.length === right.length && left.every((label, index) =>
    label.id === right[index].id && label.name === right[index].name);
}

function validateTree(tree, expectedSha) {
  if (tree?.sha !== expectedSha || !Array.isArray(tree?.tree)) fail('Git tree response is invalid or drifted');
  if (tree.truncated !== false) fail('Git tree response is truncated');
  const entries = new Map();
  for (const entry of tree.tree) {
    const name = string(entry?.path, 'Git tree entry path', null, 1024);
    if (name.includes('/') || name === '.' || name === '..' || entries.has(name)) fail('Git tree contains an ambiguous entry');
    if (typeof entry.mode !== 'string' || !/^[0-9]{6}$/.test(entry.mode)) fail('Git tree entry mode is invalid');
    const type = string(entry.type, 'Git tree entry type', /^(?:blob|tree|commit)$/u, 16);
    entries.set(name, Object.freeze({ mode: entry.mode, type, sha: exactSha(entry.sha, 'Git tree entry sha') }));
  }
  return entries;
}

async function commitTreeSha(client, sha) {
  const commit = await client.getGitCommit(sha);
  if (commit?.sha !== sha) fail('Git commit response does not match the requested SHA');
  return exactSha(commit?.tree?.sha, 'Git commit tree sha');
}

function treeResolver(client) {
  const cache = new Map();
  const readTree = async (sha) => {
    if (!cache.has(sha)) {
      cache.set(sha, client.getGitTree(sha).then((tree) => validateTree(tree, sha)));
    }
    return cache.get(sha);
  };
  return async (rootSha, filename) => {
    let treeSha = rootSha;
    const segments = filename.split('/');
    for (let index = 0; index < segments.length; index += 1) {
      const entries = await readTree(treeSha);
      const entry = entries.get(segments[index]);
      if (!entry) fail(`changed path is missing from the exact Git tree: ${filename}`);
      const final = index === segments.length - 1;
      if (final) return entry;
      if (entry.type !== 'tree' || entry.mode !== '040000') fail(`changed path traverses a non-directory: ${filename}`);
      treeSha = entry.sha;
    }
    fail('changed path is empty');
  };
}

async function enrichFiles(client, files, headSha, baseSha) {
  const [headTree, baseTree] = await Promise.all([commitTreeSha(client, headSha), commitTreeSha(client, baseSha)]);
  const resolve = treeResolver(client);
  const enriched = [];
  for (const file of files) {
    const fromBase = file.status === 'removed';
    const entry = await resolve(fromBase ? baseTree : headTree, fromBase ? file.previous_filename ?? file.filename : file.filename);
    const value = {
      filename: file.filename,
      status: file.status,
      additions: file.additions,
      deletions: file.deletions,
      changes: file.changes,
      mode: entry.mode,
      binary: file.binary,
    };
    if (file.previous_filename !== null) value.previous_filename = file.previous_filename;
    enriched.push(Object.freeze(value));
  }
  return Object.freeze(enriched);
}

export class AutonomyPolicyGitHubClient extends GitHubClient {
  getRepository() {
    return this.request('GET', `/repos/${this.repository}`);
  }

  getGitRef(branchName) {
    return this.request('GET', `/repos/${this.repository}/git/ref/heads/${encodeURIComponent(branchName)}`);
  }

  getGitCommit(sha) {
    return this.request('GET', `/repos/${this.repository}/git/commits/${encodeURIComponent(sha)}`);
  }

  getGitTree(sha) {
    return this.request('GET', `/repos/${this.repository}/git/trees/${encodeURIComponent(sha)}`);
  }

  getPullFilePage(number, page, perPage) {
    return this.request('GET', `/repos/${this.repository}/pulls/${number}/files?per_page=${perPage}&page=${page}`);
  }

  getPullLabelPage(number, page, perPage) {
    return this.request('GET', `/repos/${this.repository}/issues/${number}/labels?per_page=${perPage}&page=${page}`);
  }

}

async function listCompletePullFiles(client, pullNumber, limits) {
  const files = [];
  const maximumPages = Math.ceil(limits.maximumFiles / limits.pageSize) + 1;
  for (let page = 1; page <= maximumPages; page += 1) {
    const batch = await client.getPullFilePage(pullNumber, page, limits.pageSize);
    if (!Array.isArray(batch)) fail('pull files response is invalid');
    if (batch.length > limits.pageSize) fail('pull files response exceeds the requested page size');
    if (files.length + batch.length > limits.maximumFiles) fail('pull files exceed the policy snapshot limit');
    files.push(...batch.map((file, index) => normalizePullFile(file, files.length + index)));
    if (batch.length < limits.pageSize) return Object.freeze(files);
  }
  fail('pull files pagination did not terminate within the policy limit');
}

async function listCompletePullLabels(client, pullNumber, limits) {
  if (typeof client?.getPullLabelPage !== 'function') fail('policy client cannot read pull request labels');
  const labels = [];
  const maximumPages = Math.ceil(limits.maximumLabels / limits.labelPageSize) + 1;
  for (let page = 1; page <= maximumPages; page += 1) {
    const batch = await client.getPullLabelPage(pullNumber, page, limits.labelPageSize);
    if (!Array.isArray(batch)) fail('pull labels response is invalid');
    if (batch.length > limits.labelPageSize) fail('pull labels response exceeds the requested page size');
    if (labels.length + batch.length > limits.maximumLabels) fail('pull labels exceed the policy snapshot limit');
    labels.push(...batch.map((label, index) => normalizePullLabel(label, labels.length + index)));
    if (batch.length < limits.labelPageSize) return canonicalLabels(labels);
  }
  fail('pull labels pagination did not terminate within the policy limit');
}

/**
 * Rebuild a PR snapshot from authoritative API reads and bind it to trusted default-branch policy code.
 */
export async function buildAutonomyPolicySnapshot({ client, trigger, trust, config, limits }) {
  const normalizedTrigger = normalizeTrigger(trigger);
  const normalizedTrust = normalizeTrust(trust);
  const normalizedLimits = normalizeLimits(limits);
  if (config?.repository !== normalizedTrust.repository) fail('policy configuration repository is not trusted');

  const [repository, initialPull] = await Promise.all([
    client.getRepository(),
    client.getPull(normalizedTrigger.pull_number),
  ]);
  validateRepository(repository, normalizedTrust);
  const initial = pullIdentity(initialPull);
  validatePull(initial, normalizedTrigger, normalizedTrust, config);

  const baseRef = await client.getGitRef(normalizedTrust.default_branch);
  const currentBaseSha = exactSha(baseRef?.object?.sha, 'default branch ref sha');
  if (currentBaseSha !== initial.base_sha || currentBaseSha !== normalizedTrust.policy_sha) {
    fail('base branch, pull base, and trusted policy SHA are not identical');
  }
  const initialLabels = await listCompletePullLabels(client, normalizedTrigger.pull_number, normalizedLimits);

  if (typeof config?.branch_prefix !== 'string' || !config.branch_prefix.endsWith('-')) {
    fail('policy configuration branch_prefix is invalid');
  }
  const managedSuffix = initial.head_ref.startsWith(config.branch_prefix)
    ? initial.head_ref.slice(config.branch_prefix.length)
    : '';
  const managed = initial.head_repository === normalizedTrust.repository && /^[1-9][0-9]*$/.test(managedSuffix);
  if (!managed) {
    const [finalPull, finalLabels] = await Promise.all([
      client.getPull(normalizedTrigger.pull_number),
      listCompletePullLabels(client, normalizedTrigger.pull_number, normalizedLimits),
    ]);
    const final = pullIdentity(finalPull);
    if (!samePull(initial, final)) fail('pull request drifted while the policy snapshot was built');
    if (!sameLabels(initialLabels, finalLabels)) fail('pull request labels drifted while the policy snapshot was built');
    return Object.freeze({
      repository: normalizedTrust.repository,
      base: Object.freeze({ ref: initial.base_ref, sha: initial.base_sha }),
      head: Object.freeze({ ref: initial.head_ref, sha: initial.head_sha }),
      source: Object.freeze({ author: initial.author, branch: initial.head_ref, repository: initial.head_repository }),
      labels: initialLabels,
      labels_truncated: false,
      files: Object.freeze([]),
      truncated: false,
    });
  }

  const rawFiles = await listCompletePullFiles(client, normalizedTrigger.pull_number, normalizedLimits);
  const files = await enrichFiles(client, rawFiles, initial.head_sha, initial.base_sha);
  const [finalPull, finalLabels] = await Promise.all([
    client.getPull(normalizedTrigger.pull_number),
    listCompletePullLabels(client, normalizedTrigger.pull_number, normalizedLimits),
  ]);
  const final = pullIdentity(finalPull);
  if (!samePull(initial, final)) fail('pull request drifted while the policy snapshot was built');
  if (!sameLabels(initialLabels, finalLabels)) fail('pull request labels drifted while the policy snapshot was built');

  return Object.freeze({
    repository: normalizedTrust.repository,
    base: Object.freeze({ ref: initial.base_ref, sha: initial.base_sha }),
    head: Object.freeze({ ref: initial.head_ref, sha: initial.head_sha }),
    source: Object.freeze({ author: initial.author, branch: initial.head_ref, repository: initial.head_repository }),
    labels: initialLabels,
    labels_truncated: false,
    files,
    truncated: false,
  });
}

/** Evaluate trusted policy; the GitHub Actions job conclusion is the only published check. */
export async function evaluateAutonomyPolicy({ client, trigger, trust, config, limits }) {
  const snapshot = await buildAutonomyPolicySnapshot({ client, trigger, trust, config, limits });
  const decision = classifyAutonomyPolicy(snapshot, config);
  return Object.freeze({ snapshot, decision });
}
