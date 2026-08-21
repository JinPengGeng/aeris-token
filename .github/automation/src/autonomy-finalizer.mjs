import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import { AutonomyPolicyGitHubClient, evaluateAutonomyPolicy } from './autonomy-policy-runtime.mjs';
import { validateWriterPermissions } from './github-app-attestation.mjs';
import { policyConfigFromEnvironment } from './run-autonomy-policy.mjs';

const GITHUB_ACTIONS_APP_ID = 15368;
const GITHUB_ACTIONS_APP_SLUG = 'github-actions';
const MANAGED_MARKER = '<!-- aeris-autonomy-managed -->';
const SHA = /^[0-9a-f]{40}$/;
const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const REQUIRED_CHECKS = Object.freeze(new Map([
  ['Rust CI / check', Object.freeze({ workflow_name: 'Rust CI', workflow_path: '.github/workflows/rust-ci.yml' })],
  ['Frontend CI / check', Object.freeze({ workflow_name: 'Frontend CI', workflow_path: '.github/workflows/frontend-ci.yml' })],
  ['Automation Policy / gate', Object.freeze({ workflow_name: 'Automation Policy', workflow_path: '.github/workflows/automation-policy.yml' })],
]));
const REQUIRED_PROTECTION_CHECKS = Object.freeze([...REQUIRED_CHECKS.keys()]);
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

function nonNegativeInteger(value, name) {
  if (!Number.isSafeInteger(value) || value < 0) reject(`${name} must be a non-negative integer`);
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

function validateCompleteConnection(connection, name) {
  const value = object(connection, name);
  if (!Array.isArray(value.nodes)) reject(`${name} nodes are invalid`);
  const pageInfo = object(value.pageInfo, `${name} pageInfo`);
  const totalCount = nonNegativeInteger(value.totalCount, `${name} totalCount`);
  if (boolean(pageInfo.hasNextPage, `${name} hasNextPage`) || value.nodes.length !== totalCount) {
    reject(`${name} pagination is incomplete`);
  }
  return value;
}

function validateBranchProtection(proof, defaultBranch) {
  const repository = object(proof, 'branch protection proof');
  const repositoryProfile = Object.freeze({
    mergeCommitAllowed: false,
    rebaseMergeAllowed: false,
    squashMergeAllowed: true,
    isArchived: false,
    isDisabled: false,
    isLocked: false,
  });
  for (const [field, expected] of Object.entries(repositoryProfile)) {
    if (boolean(repository[field], `repository ${field}`) !== expected) {
      reject(`repository ${field} does not match the autonomous merge profile`);
    }
  }
  const value = validateCompleteConnection(
    repository.branchProtectionRules,
    'branch protection rules',
  );
  if (value.totalCount !== 1) reject('repository branch protection profile is ambiguous');
  const rules = value.nodes.filter((rule) => rule?.pattern === defaultBranch);
  if (rules.length !== 1) reject('default branch protection rule is missing or ambiguous');
  const rule = object(rules[0], 'default branch protection rule');
  if (rule.requiresStatusChecks !== true || rule.requiresStrictStatusChecks !== true) {
    reject('default branch required checks are not strict');
  }
  if (rule.isAdminEnforced !== true) reject('default branch protection is not admin-enforced');
  if (rule.requiresConversationResolution !== true) {
    reject('default branch conversation resolution is not required');
  }
  const booleanProfile = Object.freeze({
    allowsDeletions: false,
    allowsForcePushes: false,
    blocksCreations: false,
    dismissesStaleReviews: true,
    lockAllowsFetchAndMerge: false,
    lockBranch: false,
    requireLastPushApproval: false,
    requiresApprovingReviews: true,
    requiresCodeOwnerReviews: false,
    requiresCommitSignatures: false,
    requiresDeployments: false,
    requiresLinearHistory: true,
    restrictsPushes: false,
    restrictsReviewDismissals: false,
  });
  for (const [field, expected] of Object.entries(booleanProfile)) {
    if (boolean(rule[field], `default branch ${field}`) !== expected) {
      reject(`default branch ${field} does not match the autonomous merge profile`);
    }
  }
  if (nonNegativeInteger(rule.requiredApprovingReviewCount, 'default branch required approving review count') !== 0) {
    reject('default branch requires approving reviews');
  }
  if (!Array.isArray(rule.requiredDeploymentEnvironments) || rule.requiredDeploymentEnvironments.length !== 0) {
    reject('default branch required deployment environments are not empty');
  }
  for (const field of [
    'bypassPullRequestAllowances',
    'bypassForcePushAllowances',
    'pushAllowances',
    'reviewDismissalAllowances',
  ]) {
    const allowance = validateCompleteConnection(rule[field], `default branch ${field}`);
    if (allowance.totalCount !== 0) {
      reject(`default branch ${field} are not empty`);
    }
  }
  if (!Array.isArray(rule.requiredStatusChecks)) reject('required status check descriptions are invalid');
  if (rule.requiredStatusChecks.length !== REQUIRED_PROTECTION_CHECKS.length) {
    reject('default branch required checks contain unexpected entries');
  }
  const contexts = new Set();
  for (const check of rule.requiredStatusChecks) {
    const context = required(check?.context, 'required status check context');
    if (!REQUIRED_PROTECTION_CHECKS.includes(context) || contexts.has(context)) {
      reject(`required status check is missing or ambiguous: ${context}`);
    }
    contexts.add(context);
  }
  for (const context of REQUIRED_PROTECTION_CHECKS) {
    const descriptions = rule.requiredStatusChecks.filter((check) => check?.context === context);
    if (descriptions.length !== 1) reject(`required status check is missing or ambiguous: ${context}`);
    const app = object(descriptions[0].app, `required status check App: ${context}`);
    if (app.databaseId !== GITHUB_ACTIONS_APP_ID || app.slug !== GITHUB_ACTIONS_APP_SLUG) {
      reject(`required status check source is invalid: ${context}`);
    }
  }
  const rulesets = validateCompleteConnection(
    repository.rulesets,
    'branch rulesets including parents',
  );
  for (const ruleset of rulesets.nodes) {
    const enforcement = required(ruleset?.enforcement, 'branch ruleset enforcement');
    if (!['ACTIVE', 'DISABLED', 'EVALUATE'].includes(enforcement)) reject('branch ruleset enforcement is unknown');
    if (ruleset?.target !== 'BRANCH') reject('branch ruleset target is invalid');
    if (enforcement === 'ACTIVE') reject('an active branch ruleset can alter merge governance');
  }
  return Object.freeze({
    pattern: rule.pattern,
    contexts: REQUIRED_PROTECTION_CHECKS,
    rulesets: rulesets.totalCount,
    profile: 'direct-squash-v1',
  });
}

function validateWriterIdentity(repositories, expected) {
  const accessible = object(repositories, 'Writer installation repositories');
  if (nonNegativeInteger(accessible.total_count, 'Writer installation repository count') !== 1 ||
      !Array.isArray(accessible.repositories) || accessible.repositories.length !== 1) {
    reject('Writer token repository scope is not exact');
  }
  const repository = object(accessible.repositories[0], 'Writer installation repository');
  if (positiveInteger(repository.id, 'Writer repository id') !== expected.repository_id ||
      required(repository.full_name, 'Writer repository full_name', REPOSITORY) !== expected.repository ||
      required(object(repository.owner, 'Writer repository owner').login, 'Writer repository owner login') !== expected.owner) {
    reject('Writer token repository identity is invalid');
  }
  return Object.freeze({ app_id: expected.app_id, app_slug: expected.app_slug, installation_id: expected.installation_id });
}

function validateWriterBot(user, expected) {
  const bot = object(user, 'Writer Bot identity');
  const login = required(bot.login, 'Writer Bot login');
  if (bot.type !== 'Bot' || login !== expected.writer_login || boolean(bot.site_admin, 'Writer Bot site_admin')) {
    reject('Writer Bot REST identity is invalid');
  }
  return Object.freeze({
    login,
    graphql_login: expected.graphql_login,
    database_id: positiveInteger(bot.id, 'Writer Bot database id'),
    node_id: required(bot.node_id, 'Writer Bot node id'),
  });
}

async function proveWriterIdentity(writerClient, expected) {
  const [repositories, bot] = await Promise.all([
    writerClient.getInstallationRepositories(),
    writerClient.getUser(expected.writer_login),
  ]);
  return Object.freeze({
    installation: validateWriterIdentity(repositories, expected),
    bot: validateWriterBot(bot, expected),
  });
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
  const expectedGraphQlLogin = expected.writer_graphql_login ?? expected.writer_login.replace(/\[bot\]$/, '');
  if (snapshot.authorType !== 'Bot' || snapshot.author !== expectedGraphQlLogin) {
    reject('managed pull request author is not the Writer App');
  }
  if (!Number.isSafeInteger(snapshot.authorDatabaseId) || snapshot.authorDatabaseId <= 0 ||
      typeof snapshot.authorId !== 'string' || snapshot.authorId.length === 0) {
    reject('managed pull request author identity is incomplete');
  }
  if (expected.writer_bot && (snapshot.authorId !== expected.writer_bot.node_id ||
      snapshot.authorDatabaseId !== expected.writer_bot.database_id)) {
    reject('managed pull request author does not match the live Writer Bot');
  }
  if (typeof snapshot.body !== 'string' || !snapshot.body.includes(MANAGED_MARKER) ||
      !snapshot.body.includes(`<!-- aeris-autonomy-task:${expected.task_id} -->`)) reject('managed pull request marker is invalid');
  if (!Array.isArray(snapshot.labels) || snapshot.labels.some((label) => MANUAL_LABELS.has(label))) {
    reject('managed pull request has a manual-only label');
  }
  if (!Array.isArray(snapshot.reviewThreads) || snapshot.reviewThreads.some((thread) => thread?.isResolved !== true)) {
    reject('managed pull request has unresolved review discussions');
  }
  if (![null, 'APPROVED'].includes(snapshot.reviewDecision)) reject('managed pull request review decision is blocking');
  if (snapshot.autoMergeRequest !== null) reject('managed pull request has a preexisting auto-merge request');
  if (snapshot.merged !== false || snapshot.mergedAt !== null || snapshot.mergedBy !== null || snapshot.mergeCommit !== null) {
    reject('managed pull request already has merge outcome state');
  }
  if (snapshot.mergeable !== 'MERGEABLE') reject('managed pull request is conflicting or mergeability is unknown');
  if (requireReady) {
    if (snapshot.isDraft !== false || snapshot.mergeStateStatus !== 'CLEAN') reject('managed pull request is not ready and clean');
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
  for (const field of ['merged', 'mergedAt', 'mergedBy', 'mergeCommit']) {
    if (!Object.hasOwn(pull, field)) reject(`pull request ${field} is missing`);
  }

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
    const enabledBy = object(request.enabledBy, 'pull request autoMergeRequest.enabledBy');
    autoMergeRequest = Object.freeze({
      enabledAt: required(request.enabledAt, 'pull request autoMergeRequest.enabledAt'),
      mergeMethod: required(request.mergeMethod, 'pull request autoMergeRequest.mergeMethod'),
      enabledBy: Object.freeze({
        type: required(enabledBy.__typename, 'pull request autoMergeRequest.enabledBy.__typename'),
        login: required(enabledBy.login, 'pull request autoMergeRequest.enabledBy.login'),
        id: required(enabledBy.id, 'pull request autoMergeRequest.enabledBy.id'),
        databaseId: positiveInteger(enabledBy.databaseId, 'pull request autoMergeRequest.enabledBy.databaseId'),
      }),
    });
  }

  let mergedBy = null;
  if (pull.mergedBy !== null) {
    const actor = object(pull.mergedBy, 'pull request mergedBy');
    mergedBy = Object.freeze({
      type: required(actor.__typename, 'pull request mergedBy.__typename'),
      login: required(actor.login, 'pull request mergedBy.login'),
      id: required(actor.id, 'pull request mergedBy.id'),
      databaseId: positiveInteger(actor.databaseId, 'pull request mergedBy.databaseId'),
    });
  }

  let mergeCommit = null;
  if (pull.mergeCommit !== null) {
    const commit = object(pull.mergeCommit, 'pull request mergeCommit');
    const parents = validateCompleteConnection(commit.parents, 'pull request mergeCommit parents');
    mergeCommit = Object.freeze({
      oid: required(commit.oid, 'pull request mergeCommit oid', SHA),
      parents: Object.freeze(parents.nodes.map((parent, index) => Object.freeze({
        oid: required(object(parent, `pull request mergeCommit parents[${index}]`).oid,
          `pull request mergeCommit parents[${index}].oid`, SHA),
      }))),
      parentCount: parents.totalCount,
    });
  }

  const author = object(pull.author, 'pull request author');
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
    authorType: required(author.__typename, 'pull request author.__typename'),
    author: required(author.login, 'pull request author.login'),
    authorId: required(author.id, 'pull request author.id'),
    authorDatabaseId: positiveInteger(author.databaseId, 'pull request author.databaseId'),
    mergeable: required(pull.mergeable, 'pull request mergeable'),
    mergeStateStatus: required(pull.mergeStateStatus, 'pull request mergeStateStatus'),
    merged: boolean(pull.merged, 'pull request merged'),
    mergedAt: nullableString(pull.mergedAt, 'pull request mergedAt'),
    mergedBy,
    mergeCommit,
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
            author {
              __typename login
              ... on Bot { id databaseId }
              ... on User { id databaseId }
            }
            mergeable mergeStateStatus merged mergedAt reviewDecision
            mergedBy {
              __typename login
              ... on Bot { id databaseId }
              ... on User { id databaseId }
            }
            mergeCommit {
              oid
              parents(first: 2) {
                totalCount nodes { oid } pageInfo { hasNextPage endCursor }
              }
            }
            autoMergeRequest {
              enabledAt mergeMethod
              enabledBy {
                __typename login
                ... on Bot { id databaseId }
                ... on User { id databaseId }
              }
            }
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

  async getBranchProtection() {
    const [owner, name] = this.repository.split('/');
    const data = await boundedGraphql(this, `
      query($owner: String!, $name: String!) {
        repository(owner: $owner, name: $name) {
          mergeCommitAllowed
          rebaseMergeAllowed
          squashMergeAllowed
          isArchived
          isDisabled
          isLocked
          branchProtectionRules(first: 100) {
            nodes {
              pattern
              allowsDeletions
              allowsForcePushes
              blocksCreations
              dismissesStaleReviews
              requiresStatusChecks
              requiresStrictStatusChecks
              isAdminEnforced
              lockAllowsFetchAndMerge
              lockBranch
              requireLastPushApproval
              requiredApprovingReviewCount
              requiredDeploymentEnvironments
              requiresApprovingReviews
              requiresCodeOwnerReviews
              requiresCommitSignatures
              requiresConversationResolution
              requiresDeployments
              requiresLinearHistory
              restrictsPushes
              restrictsReviewDismissals
              bypassPullRequestAllowances(first: 100) {
                totalCount nodes { id } pageInfo { hasNextPage endCursor }
              }
              bypassForcePushAllowances(first: 100) {
                totalCount nodes { id } pageInfo { hasNextPage endCursor }
              }
              pushAllowances(first: 100) {
                totalCount nodes { id } pageInfo { hasNextPage endCursor }
              }
              reviewDismissalAllowances(first: 100) {
                totalCount nodes { id } pageInfo { hasNextPage endCursor }
              }
              requiredStatusChecks { context app { databaseId slug } }
            }
            totalCount pageInfo { hasNextPage endCursor }
          }
          rulesets(first: 100, includeParents: true, targets: [BRANCH]) {
            nodes { id databaseId name enforcement target }
            totalCount pageInfo { hasNextPage endCursor }
          }
        }
      }`, { owner, name });
    return Object.freeze({
      mergeCommitAllowed: data?.repository?.mergeCommitAllowed,
      rebaseMergeAllowed: data?.repository?.rebaseMergeAllowed,
      squashMergeAllowed: data?.repository?.squashMergeAllowed,
      isArchived: data?.repository?.isArchived,
      isDisabled: data?.repository?.isDisabled,
      isLocked: data?.repository?.isLocked,
      branchProtectionRules: data?.repository?.branchProtectionRules,
      rulesets: data?.repository?.rulesets,
    });
  }

  getInstallationRepositories() {
    return this.request('GET', '/installation/repositories?per_page=100&page=1');
  }

  getUser(login) {
    return this.request('GET', `/users/${encodeURIComponent(required(login, 'GitHub user login'))}`);
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

  async mergePullRequest(pullRequestId, expectedHeadOid) {
    const data = await boundedGraphql(this, `
      mutation($id: ID!, $head: GitObjectID!) {
        mergePullRequest(input: { pullRequestId: $id, mergeMethod: SQUASH, expectedHeadOid: $head }) {
          pullRequest {
            id headRefOid merged mergeCommit { oid }
          }
        }
      }`, { id: pullRequestId, head: expectedHeadOid });
    return data?.mergePullRequest?.pullRequest;
  }
}

async function evaluateAutonomyFinalizerGates({
  client, protectionClient, trigger, trust, config, proofLevel,
  checkAttempts = 3, sleepImpl = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
}) {
  const run = await client.getWorkflowRun(trigger.run_id);
  const bound = validateWorkflowRun(run, { ...trigger, repository: trust.repository, repository_id: trust.repository_id });
  const policy = await evaluateAutonomyPolicy({ client, trigger: bound, trust, config });
  if (policy.decision.classification !== 'eligible') {
    return Object.freeze({
      eligible: false,
      reason: `policy_${policy.decision.classification}`,
      proof_level: proofLevel,
      bound,
      policy,
    });
  }
  const protection = proofLevel === 'full'
    ? validateBranchProtection(await protectionClient.getBranchProtection(), trust.default_branch)
    : null;
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
      return Object.freeze({
        eligible: true,
        reason: null,
        proof_level: proofLevel,
        bound,
        policy,
        governance,
        checks: readiness,
        protection,
      });
    }
    if (attempt < checkAttempts) await sleepImpl(2_000 * attempt);
  }
  return Object.freeze({
    eligible: false,
    reason: `required_checks_not_successful:${readiness.unsuccessful.join(',')}`,
    proof_level: proofLevel,
    bound,
    policy,
    governance,
  });
}

export async function evaluateAutonomyFinalizerPreliminary({
  client, trigger, trust, config,
  checkAttempts = 3, sleepImpl = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
}) {
  return evaluateAutonomyFinalizerGates({
    client,
    protectionClient: null,
    trigger,
    trust,
    config,
    proofLevel: 'preliminary',
    checkAttempts,
    sleepImpl,
  });
}

export async function evaluateAutonomyFinalizer({
  client, protectionClient = client, trigger, trust, config,
  checkAttempts = 3, sleepImpl = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
}) {
  if (!protectionClient || typeof protectionClient.getBranchProtection !== 'function') {
    reject('full Finalizer proof requires a branch-protection client');
  }
  return evaluateAutonomyFinalizerGates({
    client,
    protectionClient,
    trigger,
    trust,
    config,
    proofLevel: 'full',
    checkAttempts,
    sleepImpl,
  });
}

function exactOpenPull(snapshot, expected, pullRequestId) {
  return snapshot?.id === pullRequestId && snapshot?.number === expected.pull_number && snapshot?.state === 'OPEN' &&
    snapshot?.merged === false && snapshot?.mergedAt === null && snapshot?.mergedBy === null && snapshot?.mergeCommit === null &&
    snapshot?.headRefName === expected.branch_name && snapshot?.headRefOid === expected.head_sha &&
    snapshot?.headRepository === expected.repository && snapshot?.baseRefName === 'main' &&
    snapshot?.baseRefOid === expected.base_sha &&
    snapshot?.autoMergeRequest === null;
}

async function restoreDraft({ readClient, writerClient, pullRequestId, pullNumber, expected, observed }) {
  if (!exactOpenPull(observed, expected, pullRequestId)) reject('pull request state is ambiguous; Draft rollback was not attempted');
  let latest;
  try {
    latest = await readClient.getPullGovernance(pullNumber);
  } catch (error) {
    reject(`Draft rollback precondition read failed: ${error instanceof Error ? error.message : 'read failed'}`);
  }
  if (!exactOpenPull(latest, expected, pullRequestId)) {
    reject('pull request state drifted before Draft rollback; mutation was not attempted');
  }
  if (latest.isDraft === true) return;
  if (latest.isDraft !== false) reject('pull request draft state is ambiguous; Draft rollback was not attempted');
  let mutationError = null;
  try {
    const restored = await writerClient.convertPullToDraft(pullRequestId);
    if (restored?.id !== pullRequestId || restored?.isDraft !== true || restored?.headRefOid !== expected.head_sha) {
      mutationError = new AutonomyFinalizerError('GitHub returned an invalid Draft restoration result');
    }
  } catch (error) {
    mutationError = error;
  }
  let confirmed;
  try { confirmed = await readClient.getPullGovernance(pullNumber); } catch (error) {
    throw mutationError ?? error;
  }
  if (exactOpenPull(confirmed, expected, pullRequestId) && confirmed.isDraft === true) return;
  throw mutationError ?? new AutonomyFinalizerError('Draft restoration could not be independently confirmed');
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

function exactMergedOutcome(snapshot, expected, pullRequestId) {
  const expectedGraphQlLogin = expected.writer_graphql_login ?? expected.writer_login.replace(/\[bot\]$/, '');
  const merger = snapshot?.mergedBy;
  const commit = snapshot?.mergeCommit;
  return snapshot?.id === pullRequestId && snapshot?.number === expected.pull_number && snapshot?.state === 'MERGED' &&
    snapshot?.merged === true && snapshot?.isDraft === false && typeof snapshot?.mergedAt === 'string' &&
    snapshot?.headRefName === expected.branch_name && snapshot?.headRefOid === expected.head_sha &&
    snapshot?.headRepository === expected.repository && snapshot?.baseRefName === 'main' && snapshot?.autoMergeRequest === null &&
    snapshot?.authorType === 'Bot' && snapshot?.author === expectedGraphQlLogin &&
    snapshot?.authorId === expected.writer_bot.node_id && snapshot?.authorDatabaseId === expected.writer_bot.database_id &&
    merger?.type === 'Bot' && merger?.login === expectedGraphQlLogin && merger?.id === expected.writer_bot.node_id &&
    merger?.databaseId === expected.writer_bot.database_id && commit?.parentCount === 1 && commit?.parents?.length === 1 &&
    commit.parents[0]?.oid === expected.base_sha && commit?.oid !== expected.base_sha && commit?.oid !== expected.head_sha;
}

async function readOutcomeOrReject({ readClient, pullNumber, mutationError }) {
  try {
    return await readClient.getPullGovernance(pullNumber);
  } catch (readError) {
    const mutation = mutationError instanceof Error ? mutationError.message : 'merge mutation response was inconclusive';
    const read = readError instanceof Error ? readError.message : 'independent merge outcome read failed';
    reject(`${mutation}; independent merge outcome is ambiguous: ${read}`);
  }
}

export async function finalizeAutonomyPull({
  readClient, writerClient, protectionClient = writerClient, trigger, trust, config,
  writerTrust = config.writer_trust, sleepImpl,
}) {
  const configuredWriter = object(writerTrust, 'Writer trust configuration');
  const writerAppSlug = required(configuredWriter.app_slug, 'Writer trusted App slug');
  if (`${writerAppSlug}[bot]` !== config.writer_login) reject('Writer trusted App slug does not match policy');
  const writerAppId = positiveInteger(configuredWriter.app_id, 'Writer trusted App id');
  if (positiveInteger(configuredWriter.proof_app_id, 'Writer proof App id') !== writerAppId ||
      required(configuredWriter.proof_app_slug, 'Writer proof App slug') !== writerAppSlug) {
    reject('Writer App JWT proof does not match the trusted App identity');
  }
  const repositoryOwner = trust.repository.split('/')[0];
  const proofOwnerLogin = required(configuredWriter.proof_app_owner_login, 'Writer proof App owner login');
  const proofOwnerType = required(configuredWriter.proof_app_owner_type, 'Writer proof App owner type');
  if (proofOwnerLogin !== repositoryOwner || !['User', 'Organization'].includes(proofOwnerType)) {
    reject('Writer App JWT proof does not match the repository owner');
  }
  const writerInstallationId = positiveInteger(configuredWriter.installation_id, 'Writer trusted installation id');
  if (positiveInteger(configuredWriter.proof_installation_id, 'Writer proof installation id') !== writerInstallationId) {
    reject('Writer App JWT proof does not match the trusted installation');
  }
  if (required(configuredWriter.proof_installation_account_login, 'Writer proof installation account login') !== proofOwnerLogin ||
      required(configuredWriter.proof_installation_account_type, 'Writer proof installation account type') !== proofOwnerType ||
      configuredWriter.proof_repository_selection !== 'selected') {
    reject('Writer App JWT proof does not match the repository installation');
  }
  if (positiveInteger(configuredWriter.token_installation_id, 'Writer token installation id') !== writerInstallationId) {
    reject('Writer token installation does not match the configured installation');
  }
  if (required(configuredWriter.token_app_slug, 'Writer token App slug') !== writerAppSlug) {
    reject('Writer token App does not match the configured App');
  }
  const appPermissions = validateWriterPermissions(
    configuredWriter.proof_app_permissions,
    'Writer proof App permissions',
  );
  const installationPermissions = validateWriterPermissions(
    configuredWriter.proof_installation_permissions,
    'Writer proof installation permissions',
  );
  if (JSON.stringify(appPermissions) !== JSON.stringify(installationPermissions)) {
    reject('Writer App and installation permissions do not match');
  }
  const writerIdentity = await proveWriterIdentity(writerClient, Object.freeze({
    app_id: writerAppId,
    installation_id: writerInstallationId,
    app_slug: writerAppSlug,
    writer_login: config.writer_login,
    graphql_login: config.writer_login.replace(/\[bot\]$/, ''),
    owner: repositoryOwner,
    repository: trust.repository,
    repository_id: trust.repository_id,
  }));
  const initial = await evaluateAutonomyFinalizer({
    client: readClient, protectionClient, trigger, trust, config, sleepImpl,
  });
  if (!initial.eligible) return Object.freeze({ action: 'skipped', ...initial });
  const expected = Object.freeze({
    ...initial.bound,
    ...managedIssueBinding(initial.policy, config),
    repository: trust.repository,
    repository_id: trust.repository_id,
    base_sha: trust.policy_sha,
    writer_login: config.writer_login,
    writer_graphql_login: config.writer_login.replace(/\[bot\]$/, ''),
    writer_bot: writerIdentity.bot,
  });
  const pullRequestId = initial.governance.id;
  const pullNumber = initial.bound.pull_number;
  validateGovernance(initial.governance, expected);
  let transitionedToReady = false;

  if (initial.governance.isDraft) {
    const beforeReady = await evaluateAutonomyFinalizer({
      client: readClient, protectionClient, trigger, trust, config, checkAttempts: 1, sleepImpl,
    });
    if (!beforeReady.eligible) reject(`pre-ready verification failed: ${beforeReady.reason}`);
    if (beforeReady.proof_level !== 'full' || beforeReady.bound.pull_number !== expected.pull_number ||
        beforeReady.bound.head_sha !== expected.head_sha) {
      reject('pre-ready full proof drifted from the exact pull request');
    }
    validateGovernance(beforeReady.governance, expected);
    if (beforeReady.governance.id !== pullRequestId || beforeReady.governance.isDraft !== true ||
        JSON.stringify(beforeReady.governance) !== JSON.stringify(initial.governance)) {
      reject('pull request drifted before the ready-for-review transition');
    }
    let readyError = null;
    let readyMutationConfirmed = false;
    try {
      const ready = await writerClient.markPullReady(pullRequestId);
      if (ready?.id === pullRequestId && ready?.isDraft === false && ready?.headRefOid === expected.head_sha) {
        readyMutationConfirmed = true;
      } else {
        readyError = new AutonomyFinalizerError('GitHub returned an invalid ready-for-review transition result');
      }
    } catch (error) {
      readyError = error;
    }
    let observedReady;
    try { observedReady = await readClient.getPullGovernance(pullNumber); } catch (error) {
      const mutation = readyError instanceof Error ? readyError.message : 'ready-for-review mutation response was inconclusive';
      reject(`${mutation}; ready-for-review state is ambiguous: ${error instanceof Error ? error.message : 'read failed'}`);
    }
    if (!exactOpenPull(observedReady, expected, pullRequestId) || observedReady.isDraft !== false) {
      if (exactOpenPull(observedReady, expected, pullRequestId) && observedReady.isDraft === true) {
        throw readyError ?? new AutonomyFinalizerError('GitHub did not persist the ready-for-review transition');
      }
      reject('ready-for-review state is ambiguous; Draft rollback was not attempted');
    }
    transitionedToReady = readyMutationConfirmed;
  }

  let beforeMerge;
  try {
    beforeMerge = await evaluateAutonomyFinalizer({
      client: readClient, protectionClient, trigger, trust, config, checkAttempts: 1, sleepImpl,
    });
  } catch (error) {
    if (transitionedToReady) {
      let observed;
      try { observed = await readClient.getPullGovernance(pullNumber); } catch { throw error; }
      if (exactOpenPull(observed, expected, pullRequestId)) {
        await rollbackAndRethrow(error, { readClient, writerClient, pullRequestId, pullNumber, expected, observed });
      }
    }
    throw error;
  }
  if (!beforeMerge.eligible) {
    const error = new AutonomyFinalizerError(`pre-merge verification failed: ${beforeMerge.reason}`);
    let observed = beforeMerge.governance;
    if (transitionedToReady && !observed) {
      try { observed = await readClient.getPullGovernance(pullNumber); } catch { throw error; }
    }
    if (transitionedToReady && exactOpenPull(observed, expected, pullRequestId) && observed.isDraft === false) {
      await rollbackAndRethrow(error, {
        readClient, writerClient, pullRequestId, pullNumber, expected, observed,
      });
    }
    throw error;
  }
  if (beforeMerge.proof_level !== 'full' || beforeMerge.bound.pull_number !== expected.pull_number ||
      beforeMerge.bound.head_sha !== expected.head_sha || beforeMerge.governance.id !== pullRequestId) {
    reject('pre-merge full proof drifted from the exact pull request');
  }
  try {
    validateGovernance(beforeMerge.governance, expected, { requireReady: true });
  } catch (error) {
    if (transitionedToReady && exactOpenPull(beforeMerge.governance, expected, pullRequestId)) {
      await rollbackAndRethrow(error, {
        readClient, writerClient, pullRequestId, pullNumber, expected, observed: beforeMerge.governance,
      });
    }
    throw error;
  }

  try {
    const immediatelyBeforeMerge = await readClient.getPullGovernance(pullNumber);
    validateGovernance(immediatelyBeforeMerge, expected, { requireReady: true });
    if (JSON.stringify(immediatelyBeforeMerge) !== JSON.stringify(beforeMerge.governance)) {
      reject('pull request governance drifted immediately before merge mutation');
    }
  } catch (error) {
    if (transitionedToReady) {
      let observed;
      try { observed = await readClient.getPullGovernance(pullNumber); } catch { throw error; }
      if (exactOpenPull(observed, expected, pullRequestId)) {
        await rollbackAndRethrow(error, {
          readClient, writerClient, pullRequestId, pullNumber, expected, observed,
        });
      }
    }
    throw error;
  }

  let mutationError = null;
  try { await writerClient.mergePullRequest(pullRequestId, expected.head_sha); } catch (error) { mutationError = error; }
  const outcome = await readOutcomeOrReject({ readClient, pullNumber, mutationError });
  if (exactMergedOutcome(outcome, expected, pullRequestId)) {
    return Object.freeze({
      action: 'merged', pull_number: pullNumber, head_sha: expected.head_sha,
      merge_commit_sha: outcome.mergeCommit.oid,
    });
  }
  if (exactOpenPull(outcome, expected, pullRequestId)) {
    const error = mutationError ?? new AutonomyFinalizerError('GitHub did not persist the exact squash merge');
    if (!transitionedToReady) throw error;
    await rollbackAndRethrow(error, {
      readClient, writerClient, pullRequestId, pullNumber, expected, observed: outcome,
    });
  }
  reject('independent merge outcome is ambiguous; Draft rollback was not attempted');
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
  const proofLevel = required(environment.AERIS_FINALIZER_PROOF_LEVEL, 'AERIS_FINALIZER_PROOF_LEVEL');
  if (!['preliminary', 'full'].includes(proofLevel)) reject('AERIS_FINALIZER_PROOF_LEVEL is invalid');
  const mutate = environment.AERIS_FINALIZER_MUTATE === 'true';
  if (mutate !== (proofLevel === 'full')) {
    reject('Finalizer mutation mode requires full proof and preliminary proof must be read-only');
  }
  const writerClient = mutate
    ? dependencies.writerClient ?? new AutonomyFinalizerGitHubClient({
        token: required(environment.AERIS_WRITER_TOKEN, 'AERIS_WRITER_TOKEN'),
        repository,
        apiUrl: environment.GITHUB_API_URL,
      })
    : null;
  const result = mutate
    ? await finalizeAutonomyPull({
        readClient,
        writerClient,
        protectionClient: dependencies.protectionClient ?? writerClient,
        writerTrust: Object.freeze({
          app_id: positiveInteger(environment.AERIS_WRITER_APP_ID, 'AERIS_WRITER_APP_ID'),
          proof_app_id: positiveInteger(environment.AERIS_WRITER_PROOF_APP_ID, 'AERIS_WRITER_PROOF_APP_ID'),
          proof_app_slug: required(environment.AERIS_WRITER_PROOF_APP_SLUG, 'AERIS_WRITER_PROOF_APP_SLUG'),
          proof_app_owner_login: required(environment.AERIS_WRITER_PROOF_APP_OWNER_LOGIN, 'AERIS_WRITER_PROOF_APP_OWNER_LOGIN'),
          proof_app_owner_type: required(environment.AERIS_WRITER_PROOF_APP_OWNER_TYPE, 'AERIS_WRITER_PROOF_APP_OWNER_TYPE'),
          proof_app_permissions: environment.AERIS_WRITER_PROOF_APP_PERMISSIONS,
          installation_id: positiveInteger(environment.AERIS_WRITER_INSTALLATION_ID, 'AERIS_WRITER_INSTALLATION_ID'),
          proof_installation_id: positiveInteger(environment.AERIS_WRITER_PROOF_INSTALLATION_ID, 'AERIS_WRITER_PROOF_INSTALLATION_ID'),
          proof_installation_account_login: required(environment.AERIS_WRITER_PROOF_INSTALLATION_ACCOUNT_LOGIN, 'AERIS_WRITER_PROOF_INSTALLATION_ACCOUNT_LOGIN'),
          proof_installation_account_type: required(environment.AERIS_WRITER_PROOF_INSTALLATION_ACCOUNT_TYPE, 'AERIS_WRITER_PROOF_INSTALLATION_ACCOUNT_TYPE'),
          proof_installation_permissions: environment.AERIS_WRITER_PROOF_INSTALLATION_PERMISSIONS,
          proof_repository_selection: required(environment.AERIS_WRITER_PROOF_REPOSITORY_SELECTION, 'AERIS_WRITER_PROOF_REPOSITORY_SELECTION'),
          token_installation_id: positiveInteger(environment.AERIS_WRITER_TOKEN_INSTALLATION_ID, 'AERIS_WRITER_TOKEN_INSTALLATION_ID'),
          token_app_slug: required(environment.AERIS_WRITER_TOKEN_APP_SLUG, 'AERIS_WRITER_TOKEN_APP_SLUG'),
          app_slug: required(environment.AERIS_WRITER_APP_SLUG, 'AERIS_WRITER_APP_SLUG'),
        }),
        trigger, trust, config, sleepImpl: dependencies.sleepImpl,
      })
    : await evaluateAutonomyFinalizerPreliminary({
        client: readClient, trigger, trust, config, sleepImpl: dependencies.sleepImpl,
      });
  if (environment.GITHUB_OUTPUT) {
    const eligible = mutate ? result.action !== 'skipped' : result.eligible;
    fs.appendFileSync(environment.GITHUB_OUTPUT, [
      `eligible=${eligible ? 'true' : 'false'}`,
      `reason=${result.reason ?? ''}`,
      `pull_number=${result.pull_number ?? result.bound?.pull_number ?? ''}`,
      `head_sha=${result.head_sha ?? result.bound?.head_sha ?? ''}`,
      `action=${result.action ?? (result.proof_level === 'preliminary' ? 'candidate' : 'verified')}`,
      `proof_level=${result.proof_level ?? proofLevel}`,
      '',
    ].join('\n'));
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  await runAutonomyFinalizer();
}
