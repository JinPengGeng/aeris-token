import { execFileSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';

import {
  loadContracts,
  resolveModelCandidates,
  shouldUseStructuredOutput,
} from './config.mjs';
import { GitHubClient } from './github-client.mjs';
import { buildIssueInput, buildPullInput, inputFingerprint, sourceKey } from './input.mjs';
import {
  decodeMetadata,
  findManagedComment,
  metadataHasProcessedIdentity,
  metadataMatches,
  renderAnalysisComment,
  renderStatusComment,
} from './managed-comment.mjs';
import { trustedExecutorForRoute, createAiExecutor } from './ai-executor-factory.mjs';
import { byteLength } from './openai-client.mjs';
import { buildMessages, responseFormatForAgent } from './prompts.mjs';
import { evaluateRequiredChecks } from './required-checks.mjs';
import { routeIssueInvocation, routePullInvocation } from './router.mjs';
import { parseModelJson, validateAgentOutput } from './schemas.mjs';

const SCHEMA_VERSION = 1;
const LEASE_MS = 15 * 60 * 1000;
const ANALYSIS_LEASE_HEADROOM_MS = 3 * 60 * 1000;
const MAXIMUM_ANALYSIS_TIMEOUT_MS = 10 * 60 * 1000;
const LEDGER_LIMIT = 32;

function audit(event) {
  console.log(JSON.stringify(event));
}

function safeModelDiagnostic(error) {
  if (error?.code !== 'invalid_model_output') return null;
  return typeof error.diagnostic === 'string' && /^[a-z0-9_]{1,80}$/.test(error.diagnostic)
    ? error.diagnostic
    : 'unspecified';
}

function policyShaAt(repoRoot) {
  const sha = execFileSync('git', ['rev-parse', 'HEAD'], {
    cwd: repoRoot,
    encoding: 'utf8',
    windowsHide: true,
  }).trim();
  if (!/^[0-9a-f]{40}$/.test(sha)) throw new Error('trusted policy checkout is not a commit SHA');
  return sha;
}

function enabledByKillSwitch(policy, environment) {
  const value = environment[policy.kill_switch.repository_variable]?.trim().toLowerCase();
  return policy.kill_switch.enabled_values.includes(value);
}

function objectNumber(kind, event, environment) {
  if (kind === 'issue') return Number(event.issue?.number ?? environment.AERIS_ISSUE_NUMBER);
  return Number(
    event.workflow_run?.pull_requests?.[0]?.number ??
      event.issue?.number ??
      environment.AERIS_PULL_REQUEST_NUMBER,
  );
}

async function dispatchAuthorized(eventName, environment, github) {
  if (eventName !== 'workflow_dispatch') return false;
  const permission = await github.getCollaboratorPermission(environment.GITHUB_ACTOR);
  return ['admin', 'maintain', 'write'].includes(permission);
}

function generation(kind, object) {
  return kind === 'issue' ? object.updated_at : object.head.sha;
}

function usageSummary(usage) {
  if (!usage || typeof usage !== 'object') return null;
  const summary = {};
  for (const key of ['prompt_tokens', 'completion_tokens', 'total_tokens']) {
    if (Number.isFinite(usage[key])) summary[key] = usage[key];
  }
  return summary;
}

function createClient(environment, github) {
  return (
    github ??
    new GitHubClient({
      token: environment.GITHUB_TOKEN,
      repository: environment.GITHUB_REPOSITORY,
      apiUrl: environment.GITHUB_API_URL,
    })
  );
}

function artifactResult(state, reason = null, commentId = null) {
  return { state, reason, comment_id: commentId };
}

function terminalPreflight(decision, context) {
  return {
    schema_version: SCHEMA_VERSION,
    artifact_type: 'preflight',
    state: 'terminal',
    decision,
    context,
    input: null,
  };
}

function identityFromContext(context) {
  return {
    sourceKey: context.source_key,
    agent: context.agent,
    inputSha: context.input_sha,
    policySha: context.policy_sha,
  };
}

function normalizeLedger(metadata) {
  return Array.isArray(metadata?.processed_identities)
    ? metadata.processed_identities.filter(
        (entry) =>
          entry &&
          typeof entry.source_key === 'string' &&
          typeof entry.agent === 'string' &&
          typeof entry.input_sha === 'string' &&
          typeof entry.policy_sha === 'string',
      ).slice(-LEDGER_LIMIT)
    : [];
}

function appendLedger(metadata, identity, result, at) {
  const entries = normalizeLedger(metadata).filter(
    (entry) =>
      !(
        entry.source_key === identity.sourceKey &&
        entry.agent === identity.agent &&
        entry.input_sha === identity.inputSha &&
        entry.policy_sha === identity.policySha
      ),
  );
  entries.push({
    source_key: identity.sourceKey,
    agent: identity.agent,
    input_sha: identity.inputSha,
    policy_sha: identity.policySha,
    result,
    at,
  });
  return entries.slice(-LEDGER_LIMIT);
}

function recentRuns(metadata, now) {
  const cutoff = now.getTime() - 60 * 60 * 1000;
  return Array.isArray(metadata?.recent_model_runs)
    ? metadata.recent_model_runs.filter(
        (entry) =>
          entry &&
          typeof entry.at === 'string' &&
          Number.isFinite(Date.parse(entry.at)) &&
          Date.parse(entry.at) >= cutoff &&
          typeof entry.source_key === 'string' &&
          typeof entry.agent === 'string',
      )
    : [];
}

function releaseReservationRun(metadata, context, leaseToken) {
  return Array.isArray(metadata?.recent_model_runs)
    ? metadata.recent_model_runs.filter(
        (entry) =>
          !(
            entry?.source_key === context.source_key &&
            entry?.agent === context.agent &&
            entry?.reservation_token === leaseToken
          ),
      )
    : [];
}

function commonMetadata(context, decision, prior, recentModelRuns) {
  return {
    schema_version: SCHEMA_VERSION,
    source_key: context.source_key,
    object_id: context.object_id,
    object_generation: context.object_generation,
    input_sha: context.input_sha,
    policy_sha: context.policy_sha,
    agent: context.agent,
    reason_codes: [decision.reason],
    run_id: context.run_id,
    recent_model_runs: recentModelRuns,
    processed_identities: normalizeLedger(prior),
    cancel_epoch: Number.isSafeInteger(prior?.cancel_epoch) ? prior.cancel_epoch : 0,
  };
}

function leaseFailureCode(metadata, now) {
  if (metadata?.result === 'cancelled') return 'cancelled_before_analysis';
  if (
    metadata?.result === 'running' &&
    (!Number.isFinite(Date.parse(metadata?.lease_expires_at)) ||
      Date.parse(metadata.lease_expires_at) <= now.getTime())
  ) {
    return 'lease_expired';
  }
  return 'lease_fence_changed';
}

function ownsLease(metadata, reservation, context, now) {
  return (
    metadata?.result === 'running' &&
    Number.isFinite(Date.parse(metadata?.lease_expires_at)) &&
    Date.parse(metadata.lease_expires_at) > now.getTime() &&
    metadata?.lease_token === reservation.lease_token &&
    metadata?.cancel_epoch === reservation.cancel_epoch &&
    metadataMatches(metadata, identityFromContext(context))
  );
}

function analysisDeadlineAtMs(leaseExpiresAt, now) {
  const nowMs = now.getTime();
  const leaseDeadlineAtMs = Date.parse(leaseExpiresAt) - ANALYSIS_LEASE_HEADROOM_MS;
  if (!Number.isFinite(leaseDeadlineAtMs) || leaseDeadlineAtMs <= nowMs) {
    throw Object.assign(new Error('reservation lease has insufficient analysis time remaining'), {
      code: 'lease_expiring',
    });
  }
  return Math.min(leaseDeadlineAtMs, nowMs + MAXIMUM_ANALYSIS_TIMEOUT_MS);
}

async function fetchInput(kind, number, client, agents) {
  const object = kind === 'issue' ? await client.getIssue(number) : await client.getPull(number);
  const repositoryLabels = kind === 'issue' ? await client.listRepositoryLabels() : [];
  const maximumCharacters = kind === 'pull'
    ? agents.runtime.reviewer_limits.maximum_input_characters
    : agents.runtime.limits.maximum_input_characters;
  const input =
    kind === 'issue'
      ? buildIssueInput(object, { maximumCharacters, repositoryLabels })
      : buildPullInput(object, await client.listPullFiles(number), {
        maximumCharacters,
        maximumPatchCharactersPerFile: agents.runtime.reviewer_limits.maximum_patch_characters_per_file,
      });
  return { object, repositoryLabels, input, inputSha: inputFingerprint(input) };
}

function validateAiConfiguration(selectedAgent, agent, agents, environment, { requireSecretValue }) {
  if (!agent?.enabled) throw new Error(`agent is not enabled: ${selectedAgent}`);
  const candidates = resolveModelCandidates(selectedAgent, agent, environment);
  shouldUseStructuredOutput(selectedAgent, candidates, agents);
  const baseUrl = new URL(environment.AERIS_AI_BASE_URL);
  if (baseUrl.protocol !== 'https:' || baseUrl.username || baseUrl.password || baseUrl.search || baseUrl.hash) {
    throw new Error('AI base URL is invalid');
  }
  if (requireSecretValue) {
    if (!environment.AERIS_AI_API_KEY) throw new Error('AI API key is not configured');
  } else if (environment.AERIS_AI_API_KEY_PRESENT !== 'true' && !environment.AERIS_AI_API_KEY) {
    throw new Error('AI API key is not configured');
  }
  if (!agents.runtime.api.connect_timeout_seconds) throw new Error('AI connect timeout is invalid');
  return candidates;
}

function containsSensitiveModelOutput(value, apiKey) {
  const key = typeof apiKey === 'string' ? apiKey.trim() : '';
  const headerPattern = /\b(?:authorization|proxy-authorization|cookie|set-cookie)\s*[:=]/i;
  const bearerPattern = /\bbearer\s+[A-Za-z0-9._~+/=-]{8,}/i;
  const visit = (entry) => {
    if (typeof entry === 'string') {
      return (key.length > 0 && entry.includes(key)) || headerPattern.test(entry) || bearerPattern.test(entry);
    }
    if (Array.isArray(entry)) return entry.some(visit);
    return entry && typeof entry === 'object' && Object.entries(entry).some(
      ([name, nested]) => headerPattern.test(`${name}:`) || visit(nested),
    );
  };
  return visit(value);
}

async function updateManaged({ client, number, expected, body }) {
  const latest = findManagedComment(await client.listIssueComments(number));
  const changed =
    (expected?.id ?? null) !== (latest?.id ?? null) ||
    (expected?.updated_at ?? null) !== (latest?.updated_at ?? null);
  if (changed) return { state: 'stale', reason: 'managed_comment_changed' };
  const comment = latest
    ? await client.updateIssueComment(latest.id, body)
    : await client.createIssueComment(number, body);
  return { state: 'published', commentId: comment.id };
}

async function rebuildAndCompare(context, client, agents) {
  const current = await fetchInput(context.kind, context.number, client, agents);
  if (current.inputSha !== context.input_sha) {
    return { current, stale: true, reason: 'input_fingerprint_changed' };
  }
  return { current, stale: false, reason: null };
}

async function requiredChecksReady(context, client, policy) {
  if (context.kind !== 'pull' || context.event_name !== 'workflow_run') return true;
  const pull = await client.getPull(context.number);
  if (pull.head?.sha !== context.object_generation) return false;
  const state = evaluateRequiredChecks(
    policy.policy_gate.required_checks,
    await client.listCheckRunsForRef(context.object_generation),
    await client.listCommitStatuses(context.object_generation),
  );
  return state.ready;
}

export async function runPreflightPhase({
  kind,
  eventName,
  event,
  environment = process.env,
  repoRoot = environment.GITHUB_WORKSPACE ?? process.cwd(),
  contracts = null,
  policySha = null,
  github = null,
}) {
  const loaded = contracts ?? loadContracts(repoRoot);
  const { agents, policy } = loaded;
  const trustedPolicySha = policySha ?? policyShaAt(repoRoot);
  const client = createClient(environment, github);
  const number = objectNumber(kind, event, environment);
  if (!Number.isSafeInteger(number) || number <= 0) throw new Error(`invalid ${kind} number`);
  const object = kind === 'issue' ? await client.getIssue(number) : await client.getPull(number);
  const actorCanWrite = await dispatchAuthorized(eventName, environment, client);
  const routedEvent = kind === 'issue' ? { ...event, issue: object } : event;
  let decision =
    kind === 'issue'
      ? routeIssueInvocation({ eventName, event: routedEvent, manualAgent: environment.AERIS_AGENT, actorCanWrite, policy })
      : routePullInvocation({ eventName, event, pull: object, manualAgent: environment.AERIS_AGENT, actorCanWrite, policy });
  if (
    eventName === 'workflow_run' &&
    decision.action === 'analyze' &&
    event.workflow_run?.pull_requests?.length !== 1
  ) {
    decision = { action: 'skip', reason: 'workflow_run_pull_request_ambiguous' };
  }
  if (eventName === 'workflow_run' && decision.action === 'analyze') {
    const headSha = object.head.sha;
    const requiredCheckState = evaluateRequiredChecks(
      policy.policy_gate.required_checks,
      await client.listCheckRunsForRef(headSha),
      await client.listCommitStatuses(headSha),
    );
    if (!requiredCheckState.ready) {
      decision = { action: 'skip', reason: 'required_checks_not_successful' };
    }
  }
  const selectedAgent = decision.agent ?? null;
  const baseContext = {
    kind,
    event_name: eventName,
    number,
    agent: selectedAgent,
    source_key: sourceKey(eventName, event, object, environment),
    object_id: `${kind}:${number}`,
    object_generation: generation(kind, object),
    input_sha: null,
    policy_sha: trustedPolicySha,
    run_id: environment.GITHUB_RUN_ID ?? null,
  };
  if (!enabledByKillSwitch(policy, environment) && !['status', 'cancel'].includes(decision.action)) {
    return terminalPreflight({ action: 'disabled', reason: 'kill_switch_off' }, baseContext);
  }
  if (decision.action !== 'analyze') {
    if (['status', 'cancel'].includes(decision.action)) {
      const selectedFromManaged = decodeMetadata(
        findManagedComment(await client.listIssueComments(number))?.body,
      )?.agent;
      baseContext.agent =
        typeof selectedFromManaged === 'string'
          ? selectedFromManaged
          : kind === 'pull'
            ? 'reviewer'
            : 'triage';
    }
    return terminalPreflight(decision, baseContext);
  }
  const agent = agents.agents[selectedAgent];
  if (!agent?.enabled) {
    return terminalPreflight({ action: 'disabled', reason: 'agent_disabled' }, baseContext);
  }
  validateAiConfiguration(selectedAgent, agent, agents, environment, { requireSecretValue: false });
  const fetched = await fetchInput(kind, number, client, agents);
  if (eventName === 'workflow_run' && fetched.object.head?.sha !== object.head?.sha) {
    return terminalPreflight(
      { action: 'skip', reason: 'pull_request_head_changed_during_preflight' },
      baseContext,
    );
  }
  return {
    schema_version: SCHEMA_VERSION,
    artifact_type: 'preflight',
    state: 'ready',
    decision,
    context: { ...baseContext, object_generation: generation(kind, fetched.object), input_sha: fetched.inputSha },
    input: null,
  };
}

export async function runReservationPhase({
  artifact,
  environment = process.env,
  repoRoot = environment.GITHUB_WORKSPACE ?? process.cwd(),
  contracts = null,
  policySha = null,
  github = null,
  clock = () => new Date(),
  randomToken = () => randomBytes(32).toString('base64url'),
}) {
  if (artifact.state === 'terminal') {
    const context = artifact.context;
    if (!['status', 'cancel'].includes(artifact.decision.action)) {
      return {
        schema_version: SCHEMA_VERSION,
        artifact_type: 'reservation',
        state: 'terminal',
        preflight: artifact,
        reservation: null,
        result: artifactResult(artifact.decision.action === 'disabled' ? 'disabled' : 'skipped', artifact.decision.reason),
      };
    }
    const client = createClient(environment, github);
    const managed = findManagedComment(await client.listIssueComments(context.number));
    const prior = decodeMetadata(managed?.body);
    const cancel = artifact.decision.action === 'cancel';
    const metadata = cancel
      ? {
          ...commonMetadata(context, artifact.decision, prior, recentRuns(prior, clock())),
          result: 'cancelled',
          next_agent: null,
          lease_token: null,
          lease_expires_at: null,
          cancel_epoch: (Number.isSafeInteger(prior?.cancel_epoch) ? prior.cancel_epoch : 0) + 1,
          cancelled_at: clock().toISOString(),
        }
      : prior
        ? { ...prior, reason_codes: [...(prior.reason_codes ?? []), artifact.decision.reason] }
        : {
            ...commonMetadata(context, artifact.decision, prior, []),
            result: 'idle',
            next_agent: null,
            lease_token: null,
            lease_expires_at: null,
            cancelled_at: null,
          };
    if (cancel && prior?.source_key && prior?.input_sha && prior?.policy_sha && prior?.agent) {
      metadata.processed_identities = appendLedger(
        prior,
        { sourceKey: prior.source_key, agent: prior.agent, inputSha: prior.input_sha, policySha: prior.policy_sha },
        'cancelled',
        clock().toISOString(),
      );
    }
    const policy = (contracts ?? loadContracts(repoRoot)).policy;
    const body = renderStatusComment(metadata.result, metadata, policy.limits.maximum_comment_characters, managed?.body);
    const published = await updateManaged({ client, number: context.number, expected: managed, body });
    return {
      schema_version: SCHEMA_VERSION,
      artifact_type: 'reservation',
      state: 'terminal',
      preflight: artifact,
      reservation: null,
      result: artifactResult(published.state, published.reason ?? artifact.decision.reason, published.commentId ?? null),
    };
  }

  const loaded = contracts ?? loadContracts(repoRoot);
  const { agents, policy } = loaded;
  if ((policySha ?? policyShaAt(repoRoot)) !== artifact.context.policy_sha) {
    return {
      schema_version: SCHEMA_VERSION,
      artifact_type: 'reservation',
      state: 'terminal',
      preflight: artifact,
      reservation: null,
      result: artifactResult('stale', 'policy_sha_changed'),
    };
  }
  const client = createClient(environment, github);
  const context = artifact.context;
  const identity = identityFromContext(context);
  const managed = findManagedComment(await client.listIssueComments(context.number));
  const prior = decodeMetadata(managed?.body);
  const now = clock();
  const active = prior?.result === 'running' && Number.isFinite(Date.parse(prior?.lease_expires_at)) && Date.parse(prior.lease_expires_at) > now.getTime();
  let terminal = null;
  if (metadataHasProcessedIdentity(prior, identity)) terminal = artifactResult('noop', 'event_replayed');
  else if (metadataMatches(prior, identity) && !['running', 'deferred'].includes(prior?.result)) terminal = artifactResult('noop', 'event_replayed');
  else if (active) terminal = artifactResult('in_progress', 'running_lease_active');
  const runs = recentRuns(prior, now);
  if (!terminal && runs.length >= policy.limits.maximum_runs_per_object_per_hour) terminal = artifactResult('rate_limited', 'object_hourly_limit');
  if (terminal) {
    return { schema_version: SCHEMA_VERSION, artifact_type: 'reservation', state: 'terminal', preflight: artifact, reservation: null, result: terminal };
  }
  const freshness = await rebuildAndCompare(context, client, agents);
  if (freshness.stale) {
    return { schema_version: SCHEMA_VERSION, artifact_type: 'reservation', state: 'terminal', preflight: artifact, reservation: null, result: artifactResult('stale', freshness.reason) };
  }
  const leaseToken = randomToken();
  const reservedRuns = [...runs, {
    at: now.toISOString(),
    source_key: context.source_key,
    agent: context.agent,
    reservation_token: leaseToken,
  }];
  const metadata = {
    ...commonMetadata(context, artifact.decision, prior, reservedRuns),
    result: 'running',
    next_agent: null,
    lease_token: leaseToken,
    lease_expires_at: new Date(now.getTime() + LEASE_MS).toISOString(),
    cancelled_at: null,
  };
  const body = renderStatusComment('running', metadata, policy.limits.maximum_comment_characters, managed?.body);
  const published = await updateManaged({ client, number: context.number, expected: managed, body });
  if (published.state !== 'published') {
    return { schema_version: SCHEMA_VERSION, artifact_type: 'reservation', state: 'terminal', preflight: artifact, reservation: null, result: artifactResult(published.state, published.reason) };
  }
  const winner = findManagedComment(await client.listIssueComments(context.number));
  const winnerMetadata = decodeMetadata(winner?.body);
  if (
    winner?.id !== (published.commentId ?? managed?.id) ||
    winnerMetadata?.result !== 'running' ||
    winnerMetadata?.lease_token !== leaseToken ||
    winnerMetadata?.cancel_epoch !== metadata.cancel_epoch ||
    !metadataMatches(winnerMetadata, identity)
  ) {
    return { schema_version: SCHEMA_VERSION, artifact_type: 'reservation', state: 'terminal', preflight: artifact, reservation: null, result: artifactResult('in_progress', 'reservation_lost') };
  }
  return {
    schema_version: SCHEMA_VERSION,
    artifact_type: 'reservation',
    state: 'reserved',
    preflight: artifact,
    reservation: {
      comment_id: winner?.id ?? published.commentId ?? null,
      comment_updated_at: winner?.updated_at ? new Date(winner.updated_at).toISOString() : null,
      lease_expires_at: metadata.lease_expires_at,
      lease_token: leaseToken,
      cancel_epoch: metadata.cancel_epoch,
    },
    result: null,
  };
}

export async function runAnalysisPhase({
  artifact,
  environment = process.env,
  repoRoot = environment.GITHUB_WORKSPACE ?? process.cwd(),
  contracts = null,
  policySha = null,
  github = null,
  aiClientFactory = (options) => createAiExecutor({ ...options, repoRoot, route: 'agent_analysis' }),
  clock = () => new Date(),
  auditEvent = audit,
}) {
  if (artifact.state === 'terminal') {
    return { schema_version: SCHEMA_VERSION, artifact_type: 'analysis', state: 'terminal', reservation: artifact, output: null, model: null, failure: null };
  }
  const loaded = contracts ?? loadContracts(repoRoot);
  const { agents, policy } = loaded;
  const context = artifact.preflight.context;
  const agent = agents.agents[context.agent];
  if ((policySha ?? policyShaAt(repoRoot)) !== context.policy_sha) {
    return {
      schema_version: SCHEMA_VERSION,
      artifact_type: 'analysis',
      state: 'failed',
      reservation: artifact,
      output: null,
      model: null,
      failure: { code: 'policy_sha_changed' },
    };
  }
  const client = createClient(environment, github);
  const currentManaged = findManagedComment(await client.listIssueComments(context.number));
  const currentMetadata = decodeMetadata(currentManaged?.body);
  const reservation = artifact.reservation;
  const initialFenceTime = clock();
  const stillOwnsLease = ownsLease(currentMetadata, reservation, context, initialFenceTime);
  if (!stillOwnsLease) {
    return {
      schema_version: SCHEMA_VERSION,
      artifact_type: 'analysis',
      state: 'failed',
      reservation: artifact,
      output: null,
      model: null,
      failure: { code: leaseFailureCode(currentMetadata, initialFenceTime) },
    };
  }
  let completion = null;
  try {
    if (!enabledByKillSwitch(loaded.policy, environment)) throw new Error('agent kill switch is off');
    let modelInput = artifact.preflight.input;
    if (!modelInput) {
      const fetched = await fetchInput(context.kind, context.number, client, agents);
      if (fetched.inputSha !== context.input_sha) throw new Error('input fingerprint changed before analysis');
      modelInput = fetched.input;
    }
    const candidates = validateAiConfiguration(context.agent, agent, agents, environment, {
      requireSecretValue: true,
    });
    const useStructuredOutput = shouldUseStructuredOutput(context.agent, candidates, agents);
    const finalManaged = findManagedComment(await client.listIssueComments(context.number));
    const finalMetadata = decodeMetadata(finalManaged?.body);
    const finalFenceTime = clock();
    const finalFenceMatches = ownsLease(finalMetadata, reservation, context, finalFenceTime);
    if (!finalFenceMatches) throw Object.assign(new Error('lease fence changed before model call'), {
      code: leaseFailureCode(finalMetadata, finalFenceTime),
    });
    if (!(await requiredChecksReady(context, client, policy))) {
      throw Object.assign(new Error('required checks changed before analysis'), {
        code: 'required_checks_not_successful',
      });
    }
    const modelCallManaged = findManagedComment(await client.listIssueComments(context.number));
    const modelCallMetadata = decodeMetadata(modelCallManaged?.body);
    const modelCallTime = clock();
    if (!ownsLease(modelCallMetadata, reservation, context, modelCallTime)) {
      throw Object.assign(new Error('lease fence changed during model preconditions'), {
        code: leaseFailureCode(modelCallMetadata, modelCallTime),
      });
    }
    const deadlineAtMs = analysisDeadlineAtMs(modelCallMetadata.lease_expires_at, modelCallTime);
    const expectedExecutor = trustedExecutorForRoute({ repoRoot, route: 'agent_analysis' });
    const ai = aiClientFactory({
      baseUrl: environment.AERIS_AI_BASE_URL,
      apiKey: environment.AERIS_AI_API_KEY,
      endpoint: agents.runtime.api.endpoint,
      retryableStatuses: agents.model_policy.retryable_http_statuses,
      connectTimeoutMs: agents.runtime.api.connect_timeout_seconds * 1000,
      timeoutMs: (context.agent === 'reviewer'
        ? agents.runtime.reviewer_limits.request_timeout_seconds
        : agents.runtime.api.request_timeout_seconds) * 1000,
      deadlineAtMs,
      maximumResponseBytes: agents.runtime.api.maximum_response_bytes,
    });
    completion = await ai.complete({
      candidates,
      messages: buildMessages(context.agent, modelInput),
      maxTokens: agents.runtime.limits.maximum_output_tokens,
      responseFormat: useStructuredOutput ? responseFormatForAgent(context.agent) : undefined,
    });
    if (completion?.executor && (
      completion.executor.id !== expectedExecutor.id || completion.executor.protocol !== expectedExecutor.protocol
    )) {
      throw Object.assign(new Error('AI executor identity does not match the trusted route'), { code: 'executor_identity_mismatch' });
    }
    const repositoryLabels = context.kind === 'issue' ? modelInput.available_labels : [];
    const output = validateAgentOutput(context.agent, parseModelJson(completion.content), repositoryLabels);
    if (containsSensitiveModelOutput(output, environment.AERIS_AI_API_KEY)) {
      throw Object.assign(new Error('model output contains sensitive material'), {
        code: 'sensitive_model_output',
      });
    }
    auditEvent({
      event: 'aeris_agent_model_call',
      state: 'completed',
      agent: context.agent,
      model_alias: completion.model.alias,
      model_id: completion.model.id,
      executor_id: expectedExecutor.id,
      executor_protocol: expectedExecutor.protocol,
      duration_ms: completion.durationMs,
      usage: usageSummary(completion.usage),
    });
    return {
      schema_version: SCHEMA_VERSION,
      artifact_type: 'analysis',
      state: 'completed',
      reservation: artifact,
      output,
      model: { alias: completion.model.alias, id: completion.model.id, executor: expectedExecutor, duration_ms: completion.durationMs ?? null, usage: usageSummary(completion.usage) },
      failure: null,
    };
  } catch (error) {
    auditEvent({
      event: 'aeris_agent_model_call',
      state: 'failed',
      agent: context.agent,
      code: error.code ?? 'model_call_failed',
      diagnostic: safeModelDiagnostic(error),
      status: error.status ?? null,
      completion_received: completion !== null,
      content_bytes:
        typeof completion?.content === 'string' ? byteLength(completion.content) : null,
      model_alias: completion?.model?.alias ?? null,
      model_id: completion?.model?.id ?? null,
      executor_id: completion?.executor?.id ?? null,
      executor_protocol: completion?.executor?.protocol ?? null,
      duration_ms: completion?.durationMs ?? null,
      usage: usageSummary(completion?.usage),
    });
    return {
      schema_version: SCHEMA_VERSION,
      artifact_type: 'analysis',
      state: 'failed',
      reservation: artifact,
      output: null,
      model: null,
      failure: { code: error.code ?? 'model_call_failed' },
    };
  }
}

export async function runPublishPhase({
  artifact,
  environment = process.env,
  repoRoot = environment.GITHUB_WORKSPACE ?? process.cwd(),
  contracts = null,
  policySha = null,
  github = null,
  clock = () => new Date(),
}) {
  if (artifact.state === 'terminal') {
    return { schema_version: SCHEMA_VERSION, artifact_type: 'publication', state: artifact.reservation.result.state, analysis: artifact, result: artifact.reservation.result };
  }
  const loaded = contracts ?? loadContracts(repoRoot);
  const { agents, policy } = loaded;
  const currentPolicySha = policySha ?? policyShaAt(repoRoot);
  const client = createClient(environment, github);
  const reservation = artifact.reservation;
  const context = reservation.preflight.context;
  const identity = identityFromContext(context);
  const managed = findManagedComment(await client.listIssueComments(context.number));
  const metadata = decodeMetadata(managed?.body);
  const ownsLease =
    metadata?.result === 'running' &&
    Number.isFinite(Date.parse(metadata?.lease_expires_at)) &&
    Date.parse(metadata.lease_expires_at) > clock().getTime() &&
    metadata?.lease_token === reservation.reservation.lease_token &&
    metadata?.cancel_epoch === reservation.reservation.cancel_epoch &&
    metadataMatches(metadata, identity);
  if (currentPolicySha !== context.policy_sha) {
    if (ownsLease) {
      const staleMetadata = {
        ...metadata,
        result: 'stale',
        reason_codes: [...(metadata.reason_codes ?? []), 'policy_sha_changed'],
        lease_token: null,
        lease_expires_at: null,
        processed_identities: appendLedger(metadata, identity, 'stale', clock().toISOString()),
      };
      const body = renderStatusComment(
        'stale',
        staleMetadata,
        policy.limits.maximum_comment_characters,
        managed?.body,
      );
      await updateManaged({ client, number: context.number, expected: managed, body });
    }
    return {
      schema_version: SCHEMA_VERSION,
      artifact_type: 'publication',
      state: 'stale',
      analysis: artifact,
      result: artifactResult('stale', 'policy_sha_changed'),
    };
  }
  if (!ownsLease) {
    const state = metadata?.result === 'cancelled' || metadata?.cancel_epoch !== reservation.reservation.cancel_epoch ? 'cancelled' : 'stale';
    return { schema_version: SCHEMA_VERSION, artifact_type: 'publication', state, analysis: artifact, result: artifactResult(state, state === 'cancelled' ? 'cancelled_after_reservation' : 'lease_fence_changed') };
  }
  const deferForChecks =
    artifact.failure?.code === 'required_checks_not_successful' ||
    !(await requiredChecksReady(context, client, policy));
  if (deferForChecks) {
    const deferredMetadata = {
      ...metadata,
      result: 'deferred',
      reason_codes: [...(metadata.reason_codes ?? []), 'required_checks_not_successful'],
      lease_token: null,
      lease_expires_at: null,
      // The model has not been called when analysis itself detects the check regression.
      recent_model_runs: artifact.failure?.code === 'required_checks_not_successful'
        ? releaseReservationRun(metadata, context, reservation.reservation.lease_token)
        : metadata.recent_model_runs,
    };
    const body = renderStatusComment(
      'deferred',
      deferredMetadata,
      policy.limits.maximum_comment_characters,
      managed?.body,
    );
    const published = await updateManaged({ client, number: context.number, expected: managed, body });
    const state = published.state === 'published' ? 'skipped' : published.state;
    const reason = published.reason ?? 'required_checks_not_successful';
    audit({ event: 'aeris_agent_run', state, reason, agent: context.agent, kind: context.kind, number: context.number });
    return {
      schema_version: SCHEMA_VERSION,
      artifact_type: 'publication',
      state,
      analysis: artifact,
      result: artifactResult(state, reason, published.commentId ?? null),
    };
  }
  const freshness = await rebuildAndCompare(context, client, agents);
  const resultState = artifact.state === 'completed' ? 'completed' : 'failed';
  const staleReason = freshness.reason;
  const isStale = freshness.stale;
  const finalState = isStale ? 'stale' : resultState;
  const finalMetadata = {
    ...metadata,
    result: finalState,
    reason_codes: isStale ? [...metadata.reason_codes, staleReason] : artifact.state === 'failed' ? [...metadata.reason_codes, artifact.failure.code] : metadata.reason_codes,
    next_agent: artifact.output?.next_agent ?? null,
    model_alias: artifact.model?.alias,
    model_id: artifact.model?.id,
    lease_token: null,
    lease_expires_at: null,
    processed_identities: appendLedger(metadata, identity, finalState, clock().toISOString()),
  };
  const body = artifact.state === 'completed' && !isStale
    ? renderAnalysisComment(context.agent, artifact.output, finalMetadata, policy.limits.maximum_comment_characters)
    : renderStatusComment(finalState, finalMetadata, policy.limits.maximum_comment_characters, managed?.body);
  const published = await updateManaged({ client, number: context.number, expected: managed, body });
  const state = published.state === 'published' ? (isStale ? 'stale' : 'published') : published.state;
  const reason = published.reason ?? (isStale ? staleReason : artifact.state === 'failed' ? artifact.failure.code : null);
  audit({ event: 'aeris_agent_run', state, reason, agent: context.agent, kind: context.kind, number: context.number });
  return { schema_version: SCHEMA_VERSION, artifact_type: 'publication', state, analysis: artifact, result: artifactResult(state, reason, published.commentId ?? null) };
}

export async function runAutomation(options) {
  const preflight = await runPreflightPhase(options);
  const reservation = await runReservationPhase({ ...options, artifact: preflight });
  const analysis = await runAnalysisPhase({ ...options, artifact: reservation });
  const publication = await runPublishPhase({ ...options, artifact: analysis });
  if (analysis.state === 'failed') {
    const error = new Error(analysis.failure.code);
    error.code = analysis.failure.code;
    throw error;
  }
  const result = publication.result;
  return {
    state: result.state,
    ...(result.reason === null ? {} : { reason: result.reason }),
    ...(result.comment_id === null || result.state === 'stale' ? {} : { commentId: result.comment_id }),
  };
}
