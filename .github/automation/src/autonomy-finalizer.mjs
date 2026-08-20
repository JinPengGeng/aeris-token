import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import { AutonomyPolicyGitHubClient, evaluateAutonomyPolicy } from './autonomy-policy-runtime.mjs';
import { policyConfigFromEnvironment } from './run-autonomy-policy.mjs';

const SHA = /^[0-9a-f]{40}$/;
const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const REQUIRED_CHECKS = Object.freeze(new Map([
  ['Rust CI / check', Object.freeze({ workflow_name: 'Rust CI', workflow_path: '.github/workflows/rust-ci.yml' })],
  ['Frontend CI / check', Object.freeze({ workflow_name: 'Frontend CI', workflow_path: '.github/workflows/frontend-ci.yml' })],
  ['Automation Policy / gate', Object.freeze({ workflow_name: 'Automation Policy', workflow_path: '.github/workflows/automation-policy.yml' })],
]));
const GITHUB_ACTIONS_APP_ID = 15368;
const MANAGED_MARKER = '<!-- aeris-autonomy-managed -->';
const MANUAL_LABELS = new Set(['autonomy-manual', 'do-not-merge']);
const WORKFLOW_IDENTITIES = Object.freeze(new Map([
  ['Automation Policy', '.github/workflows/automation-policy.yml'],
  ['Rust CI', '.github/workflows/rust-ci.yml'],
  ['Frontend CI', '.github/workflows/frontend-ci.yml'],
]));
const MAXIMUM_GRAPHQL_BYTES = 4 * 1024 * 1024;

export class AutonomyFinalizerError extends Error {
  constructor(message) {
    super(message);
    this.name = 'AutonomyFinalizerError';
  }
}

function reject(message) {
  throw new AutonomyFinalizerError(message);
}

function required(value, name, pattern = null) {
  if (typeof value !== 'string' || value.length === 0 || /[\u0000-\u001f\u007f]/.test(value)) reject(`${name} is invalid`);
  if (pattern && !pattern.test(value)) reject(`${name} format is invalid`);
  return value;
}

function positiveInteger(value, name) {
  const parsed = typeof value === 'string' && /^[1-9][0-9]*$/.test(value) ? Number(value) : value;
  if (!Number.isSafeInteger(parsed) || parsed <= 0) reject(`${name} must be a positive integer`);
  return parsed;
}

function boolean(value, name) {
  if (typeof value !== 'boolean') reject(`${name} must be a boolean`);
  return value;
}

function object(value, name) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) reject(`${name} must be an object`);
  return value;
}

function nullableString(value, name) {
  if (value === null) return null;
  return required(value, name);
}

function multilineString(value, name) {
  if (typeof value !== 'string' || value.length === 0 || /[\u0000\u007f]/.test(value)) reject(`${name} is invalid`);
  return value;
}

function checkCandidates(checkRuns) {
  if (!Array.isArray(checkRuns)) reject('check run projection is invalid');
  const candidates = new Map([...REQUIRED_CHECKS.keys()].map((name) => [name, []]));
  for (const check of checkRuns) {
    if (!candidates.has(check?.name) || !Number.isSafeInteger(check?.id) || check.id <= 0) continue;
    candidates.get(check.name).push(check);
  }
  for (const values of candidates.values()) values.sort((left, right) => right.id - left.id);
  return candidates;
}

function workflowRunIdFromCheck(check, repository) {
  if (typeof check?.details_url !== 'string') return null;
  let url;
  try { url = new URL(check.details_url); } catch { return null; }
  if (url.protocol !== 'https:' || url.hostname !== 'github.com' || url.search || url.hash) return null;
  const escapedRepository = repository.split('/').map((part) => part.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('\\/');
  const match = new RegExp(`^/${escapedRepository}/actions/runs/([1-9][0-9]*)/job/[1-9][0-9]*$`, 'i').exec(url.pathname);
  if (!match) return null;
  const runId = Number(match[1]);
  return Number.isSafeInteger(runId) ? runId : null;
}

function checkRunMatches(run, check, identity, expected, runId) {
  return run?.id === runId && run?.check_suite_id === check?.check_suite?.id &&
    run?.name === identity.workflow_name && run?.path === identity.workflow_path && run?.event === 'pull_request' &&
    run?.head_sha === expected.head_sha && run?.repository?.id === expected.repository_id &&
    run?.repository?.full_name === expected.repository && run?.head_repository?.id === expected.repository_id &&
    run?.head_repository?.full_name === expected.repository && Array.isArray(run?.pull_requests) &&
    run.pull_requests.length === 1 && run.pull_requests[0]?.number === expected.pull_number;
}

function checkSuiteMatches(suite, suiteId, expected) {
  return suite?.id === suiteId && suite?.head_sha === expected.head_sha &&
    suite?.app?.id === GITHUB_ACTIONS_APP_ID && suite?.app?.slug === 'github-actions' &&
    Array.isArray(suite?.pull_requests) && suite.pull_requests.length === 1 &&
    suite.pull_requests[0]?.number === expected.pull_number;
}

async function requiredChecksReady(client, checkRuns, expected) {
  const candidates = checkCandidates(checkRuns);
  const unsuccessful = [];
  for (const [name, identity] of REQUIRED_CHECKS) {
    let matched = null;
    let matchedRun = null;
    for (const check of candidates.get(name)) {
      if (check?.head_sha !== expected.head_sha || !Number.isSafeInteger(check?.check_suite?.id) || check.check_suite.id <= 0 ||
          check?.app?.id !== GITHUB_ACTIONS_APP_ID || check?.app?.slug !== 'github-actions') continue;
      const suite = await client.getCheckSuite(check.check_suite.id);
      if (!checkSuiteMatches(suite, check.check_suite.id, expected)) continue;
      const runId = workflowRunIdFromCheck(check, expected.repository);
      if (runId === null) continue;
      const run = await client.getWorkflowRun(runId);
      if (!checkRunMatches(run, check, identity, expected, runId)) continue;
      matched = check;
      matchedRun = run;
      break;
    }
    if (matched?.status !== 'completed' || matched?.conclusion !== 'success' ||
        matchedRun?.status !== 'completed' || matchedRun?.conclusion !== 'success') {
      unsuccessful.push(name);
    }
  }
  return Object.freeze({ ready: unsuccessful.length === 0, unsuccessful: Object.freeze(unsuccessful) });
}

function validateWorkflowRun(run, expected) {
  if (run?.id !== expected.run_id || run?.run_attempt !== expected.run_attempt || run?.event !== 'pull_request' ||
      run?.status !== 'completed' || run?.conclusion !== 'success') reject('trigger workflow lifecycle is invalid');
  const expectedPath = WORKFLOW_IDENTITIES.get(run?.name);
  if (!expectedPath || run?.path !== expectedPath) reject('trigger workflow identity is invalid');
  if (run?.repository?.id !== expected.repository_id || run?.repository?.full_name !== expected.repository ||
      run?.head_repository?.id !== expected.repository_id || run?.head_repository?.full_name !== expected.repository) {
    reject('trigger workflow repository identity is invalid');
  }
  const headSha = required(run.head_sha, 'trigger workflow head SHA', SHA);
  if (!Array.isArray(run.pull_requests) || run.pull_requests.length !== 1) reject('trigger workflow must bind exactly one pull request');
  const pullNumber = positiveInteger(run.pull_requests[0]?.number, 'trigger pull request number');
  return Object.freeze({ pull_number: pullNumber, head_sha: headSha, workflow_name: run.name });
}

function managedIssueBinding(policy, config) {
  const branchPrefix = required(config?.branch_prefix, 'managed branch prefix');
  const branchName = required(policy?.snapshot?.source?.branch, 'managed branch name');
  if (!branchName.startsWith(branchPrefix)) reject('managed pull request branch prefix is invalid');
  const issueNumber = positiveInteger(branchName.slice(branchPrefix.length), 'managed issue number');
  return Object.freeze({
    branch_name: branchName,
    issue_number: issueNumber,
    task_id: `issue:${issueNumber}`,
  });
}

function validateGovernance(snapshot, expected, { requireReady = false } = {}) {
  if (!snapshot || typeof snapshot !== 'object') reject('pull request governance projection is invalid');
  if (snapshot.number !== expected.pull_number || snapshot.state !== 'OPEN') reject('pull request is no longer open');
  if (snapshot.headRefOid !== expected.head_sha || snapshot.headRefName !== expected.branch_name ||
      snapshot.headRepository !== expected.repository) reject('managed pull request head identity drifted');
  if (snapshot.baseRefName !== 'main' || snapshot.baseRefOid !== expected.base_sha) reject('managed pull request base drifted');
  if (snapshot.author !== expected.writer_login) reject('managed pull request author is not the Writer App');
  if (typeof snapshot.body !== 'string' || !snapshot.body.includes(MANAGED_MARKER) ||
      !snapshot.body.includes(`<!-- aeris-autonomy-task:${expected.task_id} -->`)) reject('managed pull request marker is invalid');
  if (!Array.isArray(snapshot.labels) || snapshot.labels.some((label) => MANUAL_LABELS.has(label))) {
    reject('managed pull request has a manual-only label');
  }
  if (!Array.isArray(snapshot.reviewThreads) || snapshot.reviewThreads.some((thread) => thread?.isResolved !== true)) {
    reject('managed pull request has unresolved review discussions');
  }
  if (![null, 'APPROVED'].includes(snapshot.reviewDecision)) reject('managed pull request review decision is blocking');
  if (snapshot.autoMergeRequest !== null && snapshot.autoMergeRequest?.mergeMethod !== 'SQUASH') {
    reject('managed pull request auto-merge method is invalid');
  }
  if (snapshot.mergeable !== 'MERGEABLE') reject('managed pull request is conflicting or mergeability is unknown');
  if (requireReady) {
    if (snapshot.isDraft !== false || snapshot.mergeStateStatus !== 'CLEAN') reject('managed pull request is not clean and ready');
  } else if (snapshot.isDraft !== true && snapshot.isDraft !== false) {
    reject('managed pull request draft state is invalid');
  }
  return snapshot;
}

async function boundedGraphql(client, query, variables) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 30_000);
  try {
    const response = await client.fetchImpl('https://api.github.com/graphql', {
      method: 'POST',
      signal: controller.signal,
      headers: {
        accept: 'application/vnd.github+json',
        authorization: `Bearer ${client.token}`,
        'content-type': 'application/json',
        'x-github-api-version': '2022-11-28',
      },
      body: JSON.stringify({ query, variables }),
    });
    const text = await response.text();
    if (Buffer.byteLength(text, 'utf8') > MAXIMUM_GRAPHQL_BYTES) reject('GitHub GraphQL response is too large');
    if (!response.ok) reject(`GitHub GraphQL returned HTTP ${response.status}`);
    let value;
    try { value = JSON.parse(text); } catch { reject('GitHub GraphQL returned invalid JSON'); }
    if (!value || (Array.isArray(value.errors) && value.errors.length > 0)) reject('GitHub GraphQL returned errors');
    return value.data;
  } catch (error) {
    if (controller.signal.aborted) reject('GitHub GraphQL request timed out');
    throw error;
  } finally {
    clearTimeout(timeout);
  }
}

function normalizeGovernancePage(pull) {
  object(pull, 'pull request projection');
  if (!Object.hasOwn(pull, 'reviewDecision')) reject('pull request reviewDecision is missing');
  if (!Object.hasOwn(pull, 'autoMergeRequest')) reject('pull request autoMergeRequest is missing');

  const labelsConnection = object(pull.labels, 'pull request labels');
  if (!Array.isArray(labelsConnection.nodes)) reject('pull request label nodes are invalid');
  const labelPageInfo = object(labelsConnection.pageInfo, 'pull request label pageInfo');
  const labelsHaveNext = boolean(labelPageInfo.hasNextPage, 'pull request labels hasNextPage');
  if (labelsHaveNext) reject('pull request labels exceed the governance limit');
  const labels = labelsConnection.nodes.map((label, index) => required(object(label, `pull request labels[${index}]`).name, `pull request labels[${index}].name`));
  if (new Set(labels).size !== labels.length) reject('pull request labels contain duplicates');
  labels.sort();

  const threadsConnection = object(pull.reviewThreads, 'review thread connection');
  if (!Array.isArray(threadsConnection.nodes)) reject('review thread nodes are invalid');
  const threadPageInfo = object(threadsConnection.pageInfo, 'review thread pageInfo');
  const threadsHaveNext = boolean(threadPageInfo.hasNextPage, 'review thread hasNextPage');
  const endCursor = nullableString(threadPageInfo.endCursor, 'review thread endCursor');
  if (threadsHaveNext && endCursor === null) reject('review thread cursor is missing');
  const reviewThreads = threadsConnection.nodes.map((thread, index) => {
    const value = object(thread, `reviewThreads[${index}]`);
    return Object.freeze({
      id: required(value.id, `reviewThreads[${index}].id`),
      isResolved: boolean(value.isResolved, `reviewThreads[${index}].isResolved`),
    });
  });

  let autoMergeRequest = null;
  if (pull.autoMergeRequest !== null) {
    const request = object(pull.autoMergeRequest, 'pull request autoMergeRequest');
    autoMergeRequest = Object.freeze({
      enabledAt: required(request.enabledAt, 'pull request autoMergeRequest.enabledAt'),
      mergeMethod: required(request.mergeMethod, 'pull request autoMergeRequest.mergeMethod'),
    });
  }

  const core = Object.freeze({
    id: required(pull.id, 'pull request node id'),
    number: positiveInteger(pull.number, 'pull request number'),
    state: required(pull.state, 'pull request state'),
    isDraft: boolean(pull.isDraft, 'pull request isDraft'),
    body: multilineString(pull.body, 'pull request body'),
    headRefName: required(pull.headRefName, 'pull request headRefName'),
    headRefOid: required(pull.headRefOid, 'pull request headRefOid', SHA),
    headRepository: required(object(pull.headRepository, 'pull request headRepository').nameWithOwner, 'pull request headRepository.nameWithOwner'),
    baseRefName: required(pull.baseRefName, 'pull request baseRefName'),
    baseRefOid: required(pull.baseRefOid, 'pull request baseRefOid', SHA),
    author: required(object(pull.author, 'pull request author').login, 'pull request author.login'),
    mergeable: required(pull.mergeable, 'pull request mergeable'),
    mergeStateStatus: required(pull.mergeStateStatus, 'pull request mergeStateStatus'),
    reviewDecision: nullableString(pull.reviewDecision, 'pull request reviewDecision'),
    autoMergeRequest,
    labels: Object.freeze(labels),
  });
  return Object.freeze({ core, reviewThreads: Object.freeze(reviewThreads), hasNextPage: threadsHaveNext, endCursor });
}

async function readGovernanceSnapshot(client, owner, name, number) {
  let cursor = null;
  let core = null;
  let stableCore = null;
  const threads = [];
  const threadIds = new Set();
  for (let page = 0; page < 10; page += 1) {
    const data = await boundedGraphql(client, `
      query($owner: String!, $name: String!, $number: Int!, $cursor: String) {
        repository(owner: $owner, name: $name) {
          pullRequest(number: $number) {
            id number state isDraft body headRefName headRefOid baseRefName baseRefOid
            headRepository { nameWithOwner }
            author { login }
            mergeable mergeStateStatus reviewDecision
            autoMergeRequest { enabledAt mergeMethod }
            labels(first: 100) { nodes { name } pageInfo { hasNextPage } }
            reviewThreads(first: 100, after: $cursor) {
              nodes { id isResolved }
              pageInfo { hasNextPage endCursor }
            }
          }
        }
      }`, { owner, name, number, cursor });
    const pageValue = normalizeGovernancePage(data?.repository?.pullRequest);
    const encodedCore = JSON.stringify(pageValue.core);
    if (stableCore === null) {
      stableCore = encodedCore;
      core = pageValue.core;
    } else if (encodedCore !== stableCore) {
      reject('pull request governance drifted during pagination');
    }
    for (const thread of pageValue.reviewThreads) {
      if (threadIds.has(thread.id)) reject('review thread pagination contains duplicates');
      threadIds.add(thread.id);
      threads.push(thread);
    }
    if (!pageValue.hasNextPage) {
      return Object.freeze({ ...core, reviewThreads: Object.freeze(threads) });
    }
    cursor = pageValue.endCursor;
  }
  reject('review thread pagination exceeds the governance limit');
}

export class AutonomyFinalizerGitHubClient extends AutonomyPolicyGitHubClient {
  getWorkflowRun(runId) {
    return this.request('GET', `/repos/${this.repository}/actions/runs/${runId}`);
  }

  getCheckSuite(suiteId) {
    return this.request('GET', `/repos/${this.repository}/check-suites/${positiveInteger(suiteId, 'check suite id')}`);
  }

  async getPullGovernance(number) {
    const [owner, name] = this.repository.split('/');
    const initial = await readGovernanceSnapshot(this, owner, name, number);
    const confirmed = await readGovernanceSnapshot(this, owner, name, number);
    if (JSON.stringify(initial) !== JSON.stringify(confirmed)) reject('pull request governance drifted between complete reads');
    return confirmed;
  }

  async markPullReady(pullRequestId) {
    const data = await boundedGraphql(this, `
      mutation($id: ID!) {
        markPullRequestReadyForReview(input: { pullRequestId: $id }) {
          pullRequest { id isDraft headRefOid }
        }
      }`, { id: pullRequestId });
    return data?.markPullRequestReadyForReview?.pullRequest;
  }

  async convertPullToDraft(pullRequestId) {
    const data = await boundedGraphql(this, `
      mutation($id: ID!) {
        convertPullRequestToDraft(input: { pullRequestId: $id }) {
          pullRequest { id isDraft headRefOid }
        }
      }`, { id: pullRequestId });
    return data?.convertPullRequestToDraft?.pullRequest;
  }

  async enableAutoMerge(pullRequestId, expectedHeadOid) {
    const data = await boundedGraphql(this, `
      mutation($id: ID!, $head: GitObjectID!) {
        enablePullRequestAutoMerge(input: { pullRequestId: $id, mergeMethod: SQUASH, expectedHeadOid: $head }) {
          pullRequest { id headRefOid autoMergeRequest { enabledAt mergeMethod } }
        }
      }`, { id: pullRequestId, head: expectedHeadOid });
    return data?.enablePullRequestAutoMerge?.pullRequest;
  }
}

export async function evaluateAutonomyFinalizer({ client, trigger, trust, config, checkAttempts = 3, sleepImpl = (ms) => new Promise((resolve) => setTimeout(resolve, ms)) }) {
  const run = await client.getWorkflowRun(trigger.run_id);
  const bound = validateWorkflowRun(run, { ...trigger, repository: trust.repository, repository_id: trust.repository_id });
  const policy = await evaluateAutonomyPolicy({ client, trigger: bound, trust, config });
  if (policy.decision.classification !== 'eligible') {
    return Object.freeze({ eligible: false, reason: `policy_${policy.decision.classification}`, bound, policy });
  }
  const issue = managedIssueBinding(policy, config);
  const expected = Object.freeze({
    ...bound,
    ...issue,
    repository: trust.repository,
    repository_id: trust.repository_id,
    base_sha: trust.policy_sha,
    writer_login: config.writer_login,
  });
  let governance;
  let checks;
  let readiness;
  for (let attempt = 1; attempt <= checkAttempts; attempt += 1) {
    [governance, checks] = await Promise.all([
      client.getPullGovernance(bound.pull_number),
      client.listCheckRunsForRef(bound.head_sha),
    ]);
    validateGovernance(governance, expected);
    readiness = await requiredChecksReady(client, checks, expected);
    if (readiness.ready) {
      return Object.freeze({ eligible: true, reason: null, bound, policy, governance, checks: readiness });
    }
    if (attempt < checkAttempts) await sleepImpl(2_000 * attempt);
  }
  return Object.freeze({
    eligible: false,
    reason: `required_checks_not_successful:${readiness.unsuccessful.join(',')}`,
    bound,
    policy,
    governance,
  });
}

async function restoreDraft({ readClient, writerClient, pullRequestId, pullNumber }) {
  try {
    const observed = await readClient.getPullGovernance(pullNumber);
    if (observed?.id === pullRequestId && observed?.state === 'OPEN' && observed?.isDraft === true &&
        observed?.autoMergeRequest === null) return;
  } catch {
    // The mutation below is the best remaining recovery attempt after an inconclusive read.
  }
  const restored = await writerClient.convertPullToDraft(pullRequestId);
  if (restored?.id !== pullRequestId || restored?.isDraft !== true) reject('GitHub did not restore the pull request to Draft');
  const confirmed = await readClient.getPullGovernance(pullNumber);
  if (confirmed?.id !== pullRequestId || confirmed?.state !== 'OPEN' || confirmed?.isDraft !== true ||
      confirmed?.autoMergeRequest !== null) {
    reject('Draft restoration could not be confirmed');
  }
}

async function rollbackAndRethrow(error, context) {
  try {
    await restoreDraft(context);
  } catch (rollbackError) {
    const cause = error instanceof Error ? error.message : 'unknown finalization failure';
    const rollback = rollbackError instanceof Error ? rollbackError.message : 'unknown rollback failure';
    reject(`${cause}; Draft rollback failed: ${rollback}`);
  }
  throw error;
}

export async function finalizeAutonomyPull({ readClient, writerClient, trigger, trust, config, sleepImpl }) {
  let current = await evaluateAutonomyFinalizer({ client: readClient, trigger, trust, config, sleepImpl });
  if (!current.eligible) return Object.freeze({ action: 'skipped', ...current });
  const expected = {
    ...current.bound,
    ...managedIssueBinding(current.policy, config),
    repository: trust.repository,
    base_sha: trust.policy_sha,
    writer_login: config.writer_login,
  };
  if (current.governance.autoMergeRequest !== null) {
    validateGovernance(current.governance, expected, { requireReady: true });
    return Object.freeze({ action: 'already_armed', pull_number: current.bound.pull_number, head_sha: current.bound.head_sha });
  }
  const pullRequestId = current.governance.id;
  const pullNumber = current.bound.pull_number;
  let attemptedReadyTransition = false;
  if (current.governance.isDraft) {
    const beforeReady = await readClient.getPullGovernance(current.bound.pull_number);
    validateGovernance(beforeReady, expected);
    if (beforeReady.id !== current.governance.id || beforeReady.isDraft !== true || beforeReady.autoMergeRequest !== null ||
        JSON.stringify(beforeReady) !== JSON.stringify(current.governance)) {
      reject('pull request drifted immediately before the ready-for-review transition');
    }
    attemptedReadyTransition = true;
    try {
      const ready = await writerClient.markPullReady(current.governance.id);
      if (ready?.id !== current.governance.id || ready?.isDraft !== false || ready?.headRefOid !== current.bound.head_sha) {
        reject('GitHub did not persist the exact ready-for-review transition');
      }
    } catch (error) {
      await rollbackAndRethrow(error, {
        readClient, writerClient, pullRequestId, pullNumber,
      });
    }
  }
  try {
    current = await evaluateAutonomyFinalizer({ client: readClient, trigger, trust, config, sleepImpl });
    if (!current.eligible) reject('pull request drifted after the ready-for-review transition');
    validateGovernance(current.governance, expected, { requireReady: true });
    if (current.governance.autoMergeRequest !== null) {
      return Object.freeze({ action: 'already_armed', pull_number: current.bound.pull_number, head_sha: current.bound.head_sha });
    }
    const beforeArm = await readClient.getPullGovernance(current.bound.pull_number);
    validateGovernance(beforeArm, expected, { requireReady: true });
    if (beforeArm.id !== current.governance.id || beforeArm.autoMergeRequest !== null ||
        JSON.stringify(beforeArm) !== JSON.stringify(current.governance)) {
      reject('pull request drifted immediately before the native auto-merge request');
    }
    const armed = await writerClient.enableAutoMerge(current.governance.id, current.bound.head_sha);
    if (armed?.id !== current.governance.id || armed?.headRefOid !== current.bound.head_sha ||
        !armed?.autoMergeRequest?.enabledAt || armed?.autoMergeRequest?.mergeMethod !== 'SQUASH') {
      reject('GitHub did not persist the exact native auto-merge request');
    }
  } catch (error) {
    if (attemptedReadyTransition) {
      await rollbackAndRethrow(error, {
        readClient, writerClient, pullRequestId, pullNumber,
      });
    }
    throw error;
  }
  return Object.freeze({ action: 'armed', pull_number: current.bound.pull_number, head_sha: current.bound.head_sha });
}

export async function runAutonomyFinalizer(environment = process.env, dependencies = {}) {
  const config = policyConfigFromEnvironment(environment);
  const repository = required(environment.GITHUB_REPOSITORY, 'GITHUB_REPOSITORY', REPOSITORY);
  if (config.repository !== repository) reject('finalizer repository configuration drifted');
  const trigger = Object.freeze({
    run_id: positiveInteger(environment.AERIS_TRIGGER_RUN_ID, 'AERIS_TRIGGER_RUN_ID'),
    run_attempt: positiveInteger(environment.AERIS_TRIGGER_RUN_ATTEMPT, 'AERIS_TRIGGER_RUN_ATTEMPT'),
  });
  const trust = Object.freeze({
    repository,
    repository_id: positiveInteger(environment.GITHUB_REPOSITORY_ID, 'GITHUB_REPOSITORY_ID'),
    default_branch: required(environment.AERIS_DEFAULT_BRANCH, 'AERIS_DEFAULT_BRANCH'),
    policy_ref: required(environment.AERIS_POLICY_REF, 'AERIS_POLICY_REF'),
    policy_sha: required(environment.AERIS_POLICY_SHA, 'AERIS_POLICY_SHA', SHA),
  });
  const readClient = dependencies.readClient ?? new AutonomyFinalizerGitHubClient({
    token: required(environment.GITHUB_TOKEN, 'GITHUB_TOKEN'), repository, apiUrl: environment.GITHUB_API_URL,
  });
  const mutate = environment.AERIS_FINALIZER_MUTATE === 'true';
  const result = mutate
    ? await finalizeAutonomyPull({
        readClient,
        writerClient: dependencies.writerClient ?? new AutonomyFinalizerGitHubClient({
          token: required(environment.AERIS_WRITER_TOKEN, 'AERIS_WRITER_TOKEN'), repository, apiUrl: environment.GITHUB_API_URL,
        }),
        trigger, trust, config, sleepImpl: dependencies.sleepImpl,
      })
    : await evaluateAutonomyFinalizer({ client: readClient, trigger, trust, config, sleepImpl: dependencies.sleepImpl });
  if (environment.GITHUB_OUTPUT) {
    const eligible = mutate ? result.action !== 'skipped' : result.eligible;
    fs.appendFileSync(environment.GITHUB_OUTPUT, [
      `eligible=${eligible ? 'true' : 'false'}`,
      `reason=${result.reason ?? ''}`,
      `pull_number=${result.pull_number ?? result.bound?.pull_number ?? ''}`,
      `head_sha=${result.head_sha ?? result.bound?.head_sha ?? ''}`,
      `action=${result.action ?? 'verified'}`,
      '',
    ].join('\n'));
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  await runAutonomyFinalizer();
}
