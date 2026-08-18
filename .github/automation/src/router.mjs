import { canonicalWriterCommand, evaluateWriterRequest } from './writer-guard.mjs';
import { validateContracts } from './config.mjs';

const ROLE_BY_COMMAND = Object.freeze({
  triage: 'triage',
  plan: 'planner',
  review: 'reviewer',
});

function labelNames(item) {
  return new Set((item?.labels ?? []).map((label) => (typeof label === 'string' ? label : label.name)));
}

function isIgnoredActor(login, policy) {
  if (!login) return false;
  return policy.commands.ignore_actors.includes(login) || login.endsWith('[bot]');
}

function isTrustedAssociation(association, policy) {
  return policy.commands.accepted_author_associations.includes(association);
}

export function parseAgentCommand(body, policy) {
  if (typeof body !== 'string') return null;
  const trimmed = body.trim();
  if (policy.commands.ignore_markers.some((marker) => trimmed.includes(marker))) return null;
  const escapedPrefix = policy.commands.prefix.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = new RegExp(`^${escapedPrefix}\\s+(triage|plan|review|status|cancel)$`, 'i').exec(trimmed);
  return match?.[1].toLowerCase() ?? null;
}

/**
 * Writer commands deliberately do not share the permissive read-only command
 * parser. A comment body is the authorization input, so whitespace, aliases,
 * extra arguments, and case changes must fail closed.
 */
export function parseWriterCommand(body, policy) {
  if (typeof body !== 'string' || policy?.commands?.prefix !== '/agent') return null;
  if (policy.commands.ignore_markers?.some((marker) => body.includes(marker))) return null;
  return canonicalWriterCommand(body);
}

function enabledValue(value, allowedValues) {
  return typeof value === 'string' && allowedValues.includes(value.trim().toLowerCase());
}

/**
 * The caller supplies contracts loaded from a protected ref and the workflow
 * environment. Webhook payload fields never participate in feature switches.
 */
export function writerSwitchesFromTrustedContracts({ trustedContracts, environment } = {}) {
  const agents = trustedContracts?.agents;
  const policy = trustedContracts?.policy;
  const writer = agents?.agents?.writer;
  const killSwitch = policy?.kill_switch;
  if (!writer || !killSwitch || !environment || typeof environment !== 'object') {
    return { globalEnabled: false, writerVariableEnabled: false, writerContractEnabled: false };
  }
  try {
    validateContracts(agents, policy);
  } catch {
    return { globalEnabled: false, writerVariableEnabled: false, writerContractEnabled: false };
  }
  return {
    globalEnabled: enabledValue(environment[killSwitch.repository_variable], killSwitch.enabled_values ?? []),
    writerVariableEnabled: enabledValue(environment[writer.enabled_variable], ['1', 'true']),
    writerContractEnabled: writer.enabled === true && policy.writer?.enabled === true,
  };
}

function validCommentId(value) {
  return Number.isSafeInteger(value) && value > 0;
}

function matchingCommentAuthor(event, comment) {
  const sender = event?.sender?.login;
  const payloadAuthor = event?.comment?.user?.login;
  const liveAuthor = comment?.user?.login;
  return typeof sender === 'string' && sender === payloadAuthor && sender === liveAuthor;
}

/**
 * Resolve the Writer authorization path from current GitHub state. This helper
 * intentionally re-reads the comment, Issue, and collaborator permission;
 * webhook payloads are only routing hints and never authorization evidence.
 */
export async function routeWriterInvocation({
  eventName, event, github, trustedContracts, environment, fixCycle, limits, changeSet = null, patchBytes,
} = {}) {
  if (eventName !== 'issue_comment' || event?.action !== 'created') return { action: 'skip', reason: 'unsupported_event' };
  if (event.issue?.pull_request) return { action: 'skip', reason: 'pull_request_comment' };
  const policy = trustedContracts?.policy;
  const commentId = event.comment?.id;
  const issueNumber = event.issue?.number;
  if (!github || typeof github.getIssueComment !== 'function' || typeof github.getIssue !== 'function' ||
    typeof github.getCollaboratorPermission !== 'function' || !validCommentId(commentId) ||
    !Number.isSafeInteger(issueNumber) || issueNumber <= 0) return { action: 'skip', reason: 'writer_live_validation_failed' };
  try {
    const comment = await github.getIssueComment(commentId);
    if (!matchingCommentAuthor(event, comment)) return { action: 'skip', reason: 'comment_author_mismatch' };
    const command = parseWriterCommand(comment.body, policy);
    if (!command) return { action: 'skip', reason: 'no_supported_command' };
    const [issue, actorPermission] = await Promise.all([
      github.getIssue(issueNumber),
      github.getCollaboratorPermission(comment.user.login),
    ]);
    const decision = evaluateWriterRequest({
      command,
      actorLogin: comment.user.login,
      actorPermission,
      issue: issue && {
        number: issue.number,
        state: issue.state,
        isPullRequest: Boolean(issue.pull_request),
        labels: issue.labels,
      },
      switches: writerSwitchesFromTrustedContracts({ trustedContracts, environment }),
      fixCycle,
      limits,
      changeSet,
      patchBytes,
    });
    return decision.allowed
      ? { action: 'write', command, branch: decision.branch, reason: 'writer_authorized' }
      : { action: 'skip', reason: decision.reason };
  } catch {
    return { action: 'skip', reason: 'writer_live_validation_failed' };
  }
}

function commandDecision(command, association, policy, allowedRoles) {
  if (!command) return { action: 'skip', reason: 'no_supported_command' };
  if (command === 'status') return { action: 'status', reason: 'status_command' };
  const trusted = isTrustedAssociation(association, policy);
  if (!trusted) return { action: 'skip', reason: 'command_not_authorized' };
  if (command === 'cancel') return { action: 'cancel', reason: 'cancel_command' };
  const agent = ROLE_BY_COMMAND[command];
  if (!allowedRoles.includes(agent)) return { action: 'skip', reason: 'command_wrong_object_type' };
  return { action: 'analyze', agent, reason: `command_${command}` };
}

export function routeIssueInvocation({ eventName, event, manualAgent, actorCanWrite, policy }) {
  const issue = event.issue;
  const actor = event.sender?.login;
  if (eventName !== 'workflow_dispatch' && isIgnoredActor(actor, policy)) {
    return { action: 'skip', reason: 'ignored_actor' };
  }
  if (issue?.pull_request) return { action: 'skip', reason: 'pull_request_comment' };

  if (eventName === 'issue_comment') {
    const command = parseAgentCommand(event.comment?.body, policy);
    return commandDecision(
      command,
      event.comment?.author_association,
      policy,
      ['triage', 'planner'],
    );
  }

  if (eventName === 'workflow_dispatch') {
    if (!actorCanWrite) return { action: 'skip', reason: 'dispatch_not_authorized' };
    const agent = ROLE_BY_COMMAND[manualAgent];
    if (!['triage', 'planner'].includes(agent)) {
      return { action: 'skip', reason: 'invalid_dispatch_agent' };
    }
    return { action: 'analyze', agent, reason: 'workflow_dispatch' };
  }

  if (eventName !== 'issues') return { action: 'skip', reason: 'unsupported_event' };
  const labels = labelNames(issue);
  const externalLabel = policy.authorization.external_issue_analysis_requires_label;
  if (event.action === 'labeled' && event.label?.name !== externalLabel) {
    return { action: 'skip', reason: 'unrelated_label' };
  }
  if (!['opened', 'reopened', 'edited', 'labeled'].includes(event.action)) {
    return { action: 'skip', reason: 'unsupported_issue_action' };
  }
  const trusted = isTrustedAssociation(issue?.author_association, policy);
  if (!trusted && !labels.has(externalLabel)) {
    return { action: 'skip', reason: 'external_issue_requires_label' };
  }
  return { action: 'analyze', agent: 'triage', reason: `issue_${event.action}` };
}

export function routePullInvocation({ eventName, event, pull, manualAgent, actorCanWrite, policy }) {
  const actor = event.sender?.login;
  if (eventName === 'issue_comment' && isIgnoredActor(actor, policy)) {
    return { action: 'skip', reason: 'ignored_actor' };
  }

  if (eventName === 'issue_comment') {
    if (!event.issue?.pull_request) return { action: 'skip', reason: 'issue_comment' };
    const command = parseAgentCommand(event.comment?.body, policy);
    return commandDecision(command, event.comment?.author_association, policy, ['reviewer']);
  }

  if (eventName === 'workflow_dispatch') {
    if (!actorCanWrite) return { action: 'skip', reason: 'dispatch_not_authorized' };
    if (ROLE_BY_COMMAND[manualAgent] !== 'reviewer') {
      return { action: 'skip', reason: 'invalid_dispatch_agent' };
    }
    return { action: 'analyze', agent: 'reviewer', reason: 'workflow_dispatch' };
  }

  if (eventName !== 'workflow_run' || event.action !== 'completed') {
    return { action: 'skip', reason: 'unsupported_event' };
  }
  const workflowHeadSha = event.workflow_run?.head_sha;
  if (typeof workflowHeadSha !== 'string' || workflowHeadSha.length === 0) {
    return { action: 'skip', reason: 'workflow_run_head_missing' };
  }
  if (!pull || pull.draft) return { action: 'skip', reason: 'pull_request_unavailable_or_draft' };
  const pullHeadSha = pull.head?.sha;
  if (typeof pullHeadSha !== 'string' || pullHeadSha.length === 0) {
    return { action: 'skip', reason: 'pull_request_head_missing' };
  }
  if (workflowHeadSha !== pullHeadSha) {
    return { action: 'skip', reason: 'workflow_run_head_stale' };
  }
  const trusted = isTrustedAssociation(pull.author_association, policy);
  const labels = labelNames(pull);
  const externalLabel = policy.authorization.external_pull_request_analysis_requires_label;
  if (!trusted && !labels.has(externalLabel)) {
    return { action: 'skip', reason: 'external_pull_request_requires_label' };
  }
  return { action: 'analyze', agent: 'reviewer', reason: 'required_workflow_completed' };
}
