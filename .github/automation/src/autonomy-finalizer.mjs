import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import { AutonomyPolicyGitHubClient, evaluateAutonomyPolicy } from './autonomy-policy-runtime.mjs';
import {
  GITHUB_ACTIONS_APP_ID,
  GITHUB_ACTIONS_APP_SLUG,
  HOLD_CHECK_NAME,
  MANAGED_MARKER,
  holdExternalId,
} from './autonomy-hold-initializer.mjs';
import { policyConfigFromEnvironment } from './run-autonomy-policy.mjs';

const SHA = /^[0-9a-f]{40}$/;
const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const REQUIRED_CHECKS = Object.freeze(new Map([
  ['Rust CI / check', Object.freeze({ workflow_name: 'Rust CI', workflow_path: '.github/workflows/rust-ci.yml' })],
  ['Frontend CI / check', Object.freeze({ workflow_name: 'Frontend CI', workflow_path: '.github/workflows/frontend-ci.yml' })],
  ['Automation Policy / gate', Object.freeze({ workflow_name: 'Automation Policy', workflow_path: '.github/workflows/automation-policy.yml' })],
]));
const REQUIRED_PROTECTION_CHECKS = Object.freeze([
  ...REQUIRED_CHECKS.keys(),
  HOLD_CHECK_NAME,
]);
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

function holdCheckMatches(check, expected, externalId) {
  return check?.name === HOLD_CHECK_NAME && check?.head_sha === expected.head_sha &&
    check?.external_id === externalId;
}

function validateHoldCheck(check, expected, externalId, { requirePending = false } = {}) {
  if (!check || !Number.isSafeInteger(check.id) || check.id <= 0 ||
      !holdCheckMatches(check, expected, externalId) ||
      check?.app?.id !== GITHUB_ACTIONS_APP_ID || check?.app?.slug !== GITHUB_ACTIONS_APP_SLUG ||
      !Array.isArray(check.pull_requests) || check.pull_requests.length !== 1 ||
      check.pull_requests[0]?.number !== expected.pull_number) {
    reject('Autonomy Finalizer hold check identity is invalid');
  }
  if (requirePending && (check.status !== 'in_progress' || check.conclusion !== null)) {
    reject('Autonomy Finalizer hold check is not pending');
  }
  return check;
}

function holdCheckFromRuns(checkRuns, expected, externalId) {
  if (!Array.isArray(checkRuns)) reject('check run projection is invalid');
  const sameHead = checkRuns.filter((check) => check?.name === HOLD_CHECK_NAME && check?.head_sha === expected.head_sha);
  const matching = sameHead.filter((check) => check?.external_id === externalId);
  if (matching.length > 1) reject('Autonomy Finalizer has duplicate hold checks for the exact head');
  if (sameHead.length !== matching.length) reject('Autonomy Finalizer has an ambiguous hold check for the exact head');
  if (matching.length === 0) return null;
  return validateHoldCheck(matching[0], expected, externalId);
}

function validateCompletedHoldCheck(check, expected, externalId) {
  validateHoldCheck(check, expected, externalId);
  if (check.status !== 'completed' || check.conclusion !== 'success') {
    reject('Autonomy Finalizer hold check was not released successfully');
  }
  return check;
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
    autoMergeAllowed: true,
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
    profile: 'native-squash-v1',
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
  if (snapshot.autoMergeRequest !== null) {
    if (snapshot.autoMergeRequest?.mergeMethod !== 'SQUASH') {
      reject('managed pull request auto-merge method is invalid');
    }
    const enabledBy = object(snapshot.autoMergeRequest.enabledBy, 'pull request auto-merge actor');
    if (enabledBy.type !== 'Bot' || enabledBy.login !== expectedGraphQlLogin) {
      reject('managed pull request auto-merge was not enabled by the Writer App');
    }
    if (!Number.isSafeInteger(enabledBy.databaseId) || enabledBy.databaseId <= 0 ||
        typeof enabledBy.id !== 'string' || enabledBy.id.length === 0) {
      reject('managed pull request auto-merge actor identity is incomplete');
    }
    if (expected.writer_bot && (enabledBy.id !== expected.writer_bot.node_id ||
        enabledBy.databaseId !== expected.writer_bot.database_id)) {
      reject('managed pull request auto-merge actor does not match the live Writer Bot');
    }
  }
  if (snapshot.mergeable !== 'MERGEABLE') reject('managed pull request is conflicting or mergeability is unknown');
  if (requireReady) {
    // A required pending hold intentionally makes mergeStateStatus BLOCKED. Only
    // reject an unknown state here; required checks and mergeability are checked
    // independently immediately before arming.
    if (snapshot.isDraft !== false || snapshot.mergeStateStatus === 'UNKNOWN') reject('managed pull request is not ready');
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
            mergeable mergeStateStatus reviewDecision
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
          autoMergeAllowed
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
      autoMergeAllowed: data?.repository?.autoMergeAllowed,
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

  getCheckRun(checkRunId) {
    return this.request('GET', `/repos/${this.repository}/check-runs/${positiveInteger(checkRunId, 'check run id')}`);
  }

  createHoldCheck(headSha, externalId) {
    return this.request('POST', `/repos/${this.repository}/check-runs`, {
      body: {
        name: HOLD_CHECK_NAME,
        head_sha: required(headSha, 'hold check head SHA', SHA),
        external_id: required(externalId, 'hold check external id'),
        status: 'in_progress',
        output: {
          title: 'Native auto-merge is not yet armed',
          summary: 'This exact head remains blocked until the trusted Finalizer confirms native auto-merge.',
        },
      },
    });
  }

  completeHoldCheck(checkRunId, externalId) {
    return this.request('PATCH', `/repos/${this.repository}/check-runs/${positiveInteger(checkRunId, 'check run id')}`, {
      body: {
        external_id: required(externalId, 'hold check external id'),
        status: 'completed',
        conclusion: 'success',
        completed_at: new Date().toISOString(),
        output: {
          title: 'Native auto-merge confirmed',
          summary: 'The trusted Finalizer confirmed native squash auto-merge for this exact head.',
        },
      },
    });
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
          pullRequest {
            id headRefOid
            autoMergeRequest {
              enabledAt mergeMethod
              enabledBy {
                __typename login
                ... on Bot { id databaseId }
                ... on User { id databaseId }
              }
            }
          }
        }
      }`, { id: pullRequestId, head: expectedHeadOid });
    return data?.enablePullRequestAutoMerge?.pullRequest;
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

async function readExactHold(holdClient, expected, externalId) {
  const checkRuns = await holdClient.listCheckRunsForRef(expected.head_sha);
  const hold = holdCheckFromRuns(checkRuns, expected, externalId);
  if (hold === null) return null;
  const confirmed = await holdClient.getCheckRun(hold.id);
  return validateHoldCheck(confirmed, expected, externalId);
}

async function acquireHold(holdClient, expected) {
  const externalId = holdExternalId(expected);
  let check = await readExactHold(holdClient, expected, externalId);
  if (check === null) {
    const created = await holdClient.createHoldCheck(expected.head_sha, externalId);
    validateHoldCheck(created, expected, externalId, { requirePending: true });
    check = await readExactHold(holdClient, expected, externalId);
  }
  if (check === null) reject('Autonomy Finalizer hold creation could not be confirmed');
  return Object.freeze({ check, externalId });
}

async function confirmPendingHold(holdClient, hold, expected) {
  const check = await readExactHold(holdClient, expected, hold.externalId);
  if (check === null) reject('Autonomy Finalizer hold check is missing');
  validateHoldCheck(check, expected, hold.externalId, { requirePending: true });
  if (check.id !== hold.check.id) reject('Autonomy Finalizer hold check identity drifted');
  return check;
}

async function releaseHoldAfterArm({
  readClient, protectionClient, holdClient, hold, expected, baseline, trigger, trust, config, sleepImpl,
}) {
  await confirmPendingHold(holdClient, hold, expected);
  const final = await evaluateAutonomyFinalizer({
    client: readClient, protectionClient, trigger, trust, config, checkAttempts: 1, sleepImpl,
  });
  if (!final.eligible) reject(`post-arm verification failed: ${final.reason}`);
  if (final.bound.pull_number !== expected.pull_number || final.bound.head_sha !== expected.head_sha) {
    reject('final independent governance proof drifted from the exact pull request');
  }
  assertGovernanceReleaseStable(baseline, final.governance, expected);
  if (!exactAutoMergeArmed(final.governance, expected)) {
    reject('final independent governance proof did not confirm exact native auto-merge');
  }
  const pending = await confirmPendingHold(holdClient, hold, expected);
  let mutationError = null;
  try { await holdClient.completeHoldCheck(pending.id, hold.externalId); } catch (error) { mutationError = error; }
  const completed = await holdClient.getCheckRun(pending.id);
  try { return validateCompletedHoldCheck(completed, expected, hold.externalId); } catch (error) {
    throw mutationError ?? error;
  }
}

function exactAutoMergeArmed(governance, expected) {
  validateGovernance(governance, expected, { requireReady: true });
  return governance.autoMergeRequest !== null && governance.autoMergeRequest.mergeMethod === 'SQUASH';
}

const GOVERNANCE_RELEASE_FIELDS = Object.freeze([
  'id', 'number', 'state', 'isDraft', 'body', 'headRefName', 'headRefOid',
  'headRepository', 'baseRefName', 'baseRefOid', 'authorType', 'author',
  'authorId', 'authorDatabaseId',
  'mergeable', 'reviewDecision', 'labels', 'reviewThreads',
]);

function assertGovernanceReleaseStable(before, after, expected) {
  validateGovernance(after, expected, { requireReady: true });
  for (const field of GOVERNANCE_RELEASE_FIELDS) {
    if (JSON.stringify(before?.[field]) !== JSON.stringify(after?.[field])) {
      reject(`pull request governance drifted after native auto-merge request: ${field}`);
    }
  }
}

export async function finalizeAutonomyPull({
  readClient, writerClient, protectionClient = writerClient, holdClient = readClient, trigger, trust, config,
  writerTrust = config.writer_trust, sleepImpl,
}) {
  const configuredWriter = object(writerTrust, 'Writer trust configuration');
  const writerAppSlug = required(configuredWriter.app_slug, 'Writer trusted App slug');
  if (`${writerAppSlug}[bot]` !== config.writer_login) reject('Writer trusted App slug does not match policy');
  const writerInstallationId = positiveInteger(configuredWriter.installation_id, 'Writer trusted installation id');
  if (positiveInteger(configuredWriter.token_installation_id, 'Writer token installation id') !== writerInstallationId) {
    reject('Writer token installation does not match the configured installation');
  }
  const writerIdentity = await proveWriterIdentity(writerClient, Object.freeze({
    app_id: positiveInteger(configuredWriter.app_id, 'Writer trusted App id'),
    installation_id: writerInstallationId,
    app_slug: writerAppSlug,
    writer_login: config.writer_login,
    graphql_login: config.writer_login.replace(/\[bot\]$/, ''),
    owner: trust.repository.split('/')[0],
    repository: trust.repository,
    repository_id: trust.repository_id,
  }));
  let current = await evaluateAutonomyFinalizer({
    client: readClient, protectionClient, trigger, trust, config, sleepImpl,
  });
  if (!current.eligible) return Object.freeze({ action: 'skipped', ...current });
  const expected = Object.freeze({
    ...current.bound,
    ...managedIssueBinding(current.policy, config),
    repository: trust.repository,
    repository_id: trust.repository_id,
    base_sha: trust.policy_sha,
    writer_login: config.writer_login,
    writer_graphql_login: config.writer_login.replace(/\[bot\]$/, ''),
    writer_bot: writerIdentity.bot,
  });
  const pullRequestId = current.governance.id;
  const pullNumber = current.bound.pull_number;
  validateGovernance(current.governance, expected);
  const hold = await acquireHold(holdClient, expected);

  if (hold.check.status === 'completed') {
    if (hold.check.conclusion !== 'success' || !exactAutoMergeArmed(current.governance, expected)) {
      reject('Autonomy Finalizer hold was released without exact native auto-merge');
    }
    const verification = await evaluateAutonomyFinalizer({
      client: readClient, protectionClient, trigger, trust, config, sleepImpl,
    });
    if (!verification.eligible || !exactAutoMergeArmed(verification.governance, expected)) {
      reject('completed Autonomy Finalizer hold no longer matches exact native auto-merge');
    }
    assertGovernanceReleaseStable(current.governance, verification.governance, expected);
    return Object.freeze({ action: 'already_armed', pull_number: pullNumber, head_sha: current.bound.head_sha });
  }
  await confirmPendingHold(holdClient, hold, expected);
  if (current.governance.autoMergeRequest !== null) {
    if (!exactAutoMergeArmed(current.governance, expected)) reject('managed pull request auto-merge state is invalid');
    await releaseHoldAfterArm({
      readClient, protectionClient, holdClient, hold, expected, baseline: current.governance,
      trigger, trust, config, sleepImpl,
    });
    return Object.freeze({ action: 'already_armed', pull_number: pullNumber, head_sha: current.bound.head_sha });
  }

  let attemptedReadyTransition = false;
  if (current.governance.isDraft) {
    await confirmPendingHold(holdClient, hold, expected);
    const beforeReady = await readClient.getPullGovernance(pullNumber);
    validateGovernance(beforeReady, expected);
    if (beforeReady.id !== pullRequestId || beforeReady.isDraft !== true || beforeReady.autoMergeRequest !== null ||
        JSON.stringify(beforeReady) !== JSON.stringify(current.governance)) {
      reject('pull request drifted immediately before the ready-for-review transition');
    }
    attemptedReadyTransition = true;
    try {
      const ready = await writerClient.markPullReady(pullRequestId);
      if (ready?.id !== pullRequestId || ready?.isDraft !== false || ready?.headRefOid !== current.bound.head_sha) {
        reject('GitHub did not persist the exact ready-for-review transition');
      }
    } catch (error) {
      let observed;
      try { observed = await readClient.getPullGovernance(pullNumber); } catch { throw error; }
      validateGovernance(observed, expected);
      if (observed.autoMergeRequest !== null) throw error;
      await rollbackAndRethrow(error, { readClient, writerClient, pullRequestId, pullNumber });
    }
  }

  try {
    current = await evaluateAutonomyFinalizer({
      client: readClient, protectionClient, trigger, trust, config, sleepImpl,
    });
    if (!current.eligible) reject('pull request drifted after the ready-for-review transition');
    validateGovernance(current.governance, expected, { requireReady: true });
    if (current.governance.autoMergeRequest !== null) {
      await releaseHoldAfterArm({
        readClient, protectionClient, holdClient, hold, expected, baseline: current.governance,
        trigger, trust, config, sleepImpl,
      });
      return Object.freeze({ action: 'already_armed', pull_number: pullNumber, head_sha: current.bound.head_sha });
    }
    await confirmPendingHold(holdClient, hold, expected);
    const beforeArm = await readClient.getPullGovernance(pullNumber);
    validateGovernance(beforeArm, expected, { requireReady: true });
    if (beforeArm.id !== pullRequestId || beforeArm.autoMergeRequest !== null ||
        JSON.stringify(beforeArm) !== JSON.stringify(current.governance)) {
      reject('pull request drifted immediately before the native auto-merge request');
    }
    // GitHub exposes only an aggregate merge state. BLOCKED/UNSTABLE are
    // compatible with the pending required hold but do not prove that it is
    // the sole blocker; the exact governance profile above is the authority.
    if (!['BLOCKED', 'UNSTABLE'].includes(beforeArm.mergeStateStatus)) {
      reject('managed pull request aggregate merge state is incompatible with the pending hold');
    }
    let armError = null;
    try { await writerClient.enableAutoMerge(pullRequestId, current.bound.head_sha); } catch (error) { armError = error; }
    let confirmedArm;
    try { confirmedArm = await readClient.getPullGovernance(pullNumber); } catch (error) { throw armError ?? error; }
    if (!exactAutoMergeArmed(confirmedArm, expected)) {
      if (armError) throw armError;
      reject('GitHub did not persist the exact native auto-merge request');
    }
    await releaseHoldAfterArm({
      readClient, protectionClient, holdClient, hold, expected, baseline: beforeArm,
      trigger, trust, config, sleepImpl,
    });
  } catch (error) {
    if (attemptedReadyTransition) {
      let observed;
      try { observed = await readClient.getPullGovernance(pullNumber); } catch { throw error; }
      validateGovernance(observed, expected);
      if (observed.autoMergeRequest !== null) throw error;
      await rollbackAndRethrow(error, { readClient, writerClient, pullRequestId, pullNumber });
    }
    throw error;
  }
  return Object.freeze({ action: 'armed', pull_number: pullNumber, head_sha: current.bound.head_sha });
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
          installation_id: positiveInteger(environment.AERIS_WRITER_INSTALLATION_ID, 'AERIS_WRITER_INSTALLATION_ID'),
          token_installation_id: positiveInteger(environment.AERIS_WRITER_TOKEN_INSTALLATION_ID, 'AERIS_WRITER_TOKEN_INSTALLATION_ID'),
          app_slug: required(environment.AERIS_WRITER_TOKEN_APP_SLUG, 'AERIS_WRITER_TOKEN_APP_SLUG'),
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
