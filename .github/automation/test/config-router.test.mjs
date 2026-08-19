import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  ContractError,
  loadContracts,
  resolveModelCandidates,
  shouldUseStructuredOutput,
  validateContracts,
} from '../src/config.mjs';
import {
  parseAgentCommand,
  parseWriterCommand,
  revalidateWriterPublishBoundary,
  routeIssueInvocation,
  routePullInvocation,
  routeWriterInvocation,
  writerSwitchesFromTrustedContracts,
} from '../src/router.mjs';
import {
  buildIssueInput,
  buildPullInput,
  canonicalInput,
  hashInput,
  inputFingerprint,
  sourceKey,
} from '../src/input.mjs';
import { canonicalWriterCommand, evaluateWriterRequest } from '../src/writer-guard.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, '..', '..', '..');
const contracts = loadContracts(repoRoot);
const policy = contracts.policy;
const writerRepository = Object.freeze({ id: 1_316_750_512, full_name: 'JinPengGeng/aeris-token' });
const writerIssueUrl = `https://api.github.com/repos/${writerRepository.full_name}/issues/41`;

function writerComment(overrides = {}) {
  return {
    id: 91,
    body: '/agent implement',
    issue_url: writerIssueUrl,
    user: { login: 'maintainer' },
    ...overrides,
  };
}

function writerIssue(overrides = {}) {
  return {
    number: 41,
    state: 'open',
    updated_at: '2026-08-18T09:00:00Z',
    url: writerIssueUrl,
    labels: [{ name: 'agent-ready' }],
    ...overrides,
  };
}

test('trusted contracts load with only enabled agents declared', () => {
  assert.equal(Object.keys(contracts.agents.agents).length, 8);
  const enabled = Object.entries(contracts.agents.agents)
    .filter(([, agent]) => agent.enabled)
    .map(([name]) => name);
  assert.deepEqual(enabled, ['triage', 'planner', 'reviewer']);
  assert.deepEqual(contracts.agents.model_policy.structured_output, {
    canary_agents: ['planner'],
    approved_model_ids: ['gpt-5.6-sol'],
  });
});

test('writer policy command names canonicalize to the exact guarded commands', () => {
  const commands = policy.authorization.code_write_requires.exact_commands.map(canonicalWriterCommand);
  assert.deepEqual(commands, ['/agent implement', '/agent retry-write']);
  assert.equal(evaluateWriterRequest({
    command: commands[0],
    actorLogin: 'maintainer',
    actorPermission: 'write',
    issue: { number: 41, state: 'open', isPullRequest: false, labels: ['agent-ready'] },
    switches: { globalEnabled: true, writerVariableEnabled: true, writerContractEnabled: true },
    fixCycle: 0,
  }).allowed, true);
});

test('structured output is planner-only and rejects unapproved model candidates', () => {
  assert.equal(
    shouldUseStructuredOutput(
      'triage',
      [{ alias: 'default', id: 'any-model', variable: 'AERIS_AI_MODEL' }],
      contracts.agents,
    ),
    false,
  );
  assert.equal(
    shouldUseStructuredOutput(
      'planner',
      [{ alias: 'default', id: 'gpt-5.6-sol', variable: 'AERIS_AI_MODEL' }],
      contracts.agents,
    ),
    true,
  );
  assert.throws(
    () =>
      shouldUseStructuredOutput(
        'planner',
        [{ alias: 'default', id: 'unverified-model', variable: 'AERIS_AI_MODEL' }],
        contracts.agents,
      ),
    (error) =>
      error instanceof ContractError &&
      error.code === 'structured_output_model_not_approved',
  );

  const agents = structuredClone(contracts.agents);
  agents.model_policy.structured_output.canary_agents.push('triage');
  assert.throws(() => validateContracts(agents, contracts.policy), ContractError);
});

test('contract validation rejects broad fallback statuses', () => {
  const agents = structuredClone(contracts.agents);
  agents.model_policy.retryable_http_statuses.push(401);
  assert.throws(() => validateContracts(agents, contracts.policy), ContractError);
});

test('contract validation rejects divergent object concurrency limits', () => {
  const agents = structuredClone(contracts.agents);
  agents.runtime.limits.maximum_concurrent_runs_per_object = 2;
  assert.throws(() => validateContracts(agents, contracts.policy), ContractError);
});

test('Writer Phase 3 foundation stays independently disabled and least-privileged', () => {
  const writer = contracts.agents.agents.writer;
  assert.equal(writer.enabled, false);
  assert.equal(contracts.policy.writer.enabled, false);
  assert.equal(writer.enabled_variable, 'AERIS_WRITER_ENABLED');
  assert.equal(writer.identity, 'github_app');
  assert.equal(writer.app_id_variable, 'AERIS_WRITER_APP_ID');
  assert.equal(writer.app_slug_variable, 'AERIS_WRITER_APP_SLUG');
  assert.equal(writer.private_key_secret, 'AERIS_WRITER_PRIVATE_KEY');
  assert.equal(writer.environment, 'writer');
  assert.deepEqual(writer.timeouts, {
    github_api_total_seconds: 30,
    github_response_headers_seconds: 10,
    github_response_body_seconds: 15,
    publish_job_minutes: 15,
  });
  assert.deepEqual(writer.required_actor_permissions, ['admin', 'maintain', 'write']);
  assert.deepEqual(writer.required_commands, ['/agent implement', '/agent retry-write']);
  assert.deepEqual(contracts.policy.authorization.code_write_requires, {
    actor_permission: ['admin', 'maintain', 'write'],
    exact_commands: ['/agent implement', '/agent retry-write'],
    issue_labels: ['agent-ready'],
  });
  assert.deepEqual(writer.credentials, { allowed_jobs: ['publish'], github_token_write: false });
  assert.equal(writer.repository_id, writerRepository.id);
  assert.equal(writer.repository_name, writerRepository.full_name);
  assert.equal(contracts.policy.writer.repository_id, writerRepository.id);
  assert.equal(contracts.policy.writer.repository_name, writerRepository.full_name);
  assert.deepEqual(writer.permissions, {
    metadata: 'read',
    contents: 'write',
    pull_requests: 'write',
    denied: [
      'checks', 'actions', 'workflows', 'administration', 'deployments', 'environments', 'secrets',
      'members', 'packages', 'issues',
    ],
  });
  assert.deepEqual(writer.capability_residuals, {
    pull_requests_write_can_review_or_merge: true,
    contents_write_not_branch_scoped: true,
    app_has_branch_protection_bypass: false,
  });
  assert.deepEqual(writer.deterministic_client_mitigations, {
    identity_verification: 'app_jwt_mints_installation_token_then_verify',
    ambiguous_create_recovery: 'unique_attempt_marker_then_read_only_reconcile_or_fail_closed_residue',
    allowed_operations: [
      'create_or_update_agent_ref',
      'create_or_update_draft_pull_request',
    ],
    denied_operations: ['review', 'approve', 'merge', 'enable_auto_merge', 'mark_ready', 'close_pr', 'delete_branch'],
  });
  assert.deepEqual(writer.limits, {
    maximum_files: 50,
    maximum_patch_bytes: 65536,
    maximum_file_size_bytes: 524288,
    maximum_total_file_bytes: 2097152,
    maximum_fix_cycles: 2,
  });
  assert.equal(contracts.policy.writer.maximum_open_pull_requests_per_issue, 1);
  assert.deepEqual(writer.denied_paths, [
    '.github/**', '**/CODEOWNERS', '.gitmodules', '**/.git', '**/.git/**',
  ]);
  assert.deepEqual(contracts.policy.writer.forbidden_paths, writer.denied_paths);
});

test('Writer Phase 3 foundation rejects drift, broad permissions, and unsafe limits', () => {
  const mutations = [
    (agents) => { agents.agents.writer.enabled = true; },
    (agents) => { agents.agents.writer.repository_id += 1; },
    (agents) => { agents.agents.writer.repository_name = 'attacker/repository'; },
    (agents) => { agents.agents.writer.identity = 'github_token'; },
    (agents) => { agents.agents.writer.app_slug_variable = 'AERIS_OTHER_APP_SLUG'; },
    (agents) => { agents.agents.writer.timeouts.github_response_body_seconds = 16; },
    (agents) => { agents.agents.writer.timeouts.publish_job_minutes = 16; },
    (agents) => { agents.agents.writer.credentials.github_token_write = true; },
    (agents) => { agents.agents.writer.credentials.private_key = 'AERIS_WRITER_PRIVATE_KEY'; },
    (agents) => { agents.agents.writer.permissions.denied.pop(); },
    (agents) => { agents.agents.writer.permissions.issues = 'write'; },
    (agents) => { agents.agents.writer.deterministic_client_mitigations.identity_verification = 'installation_token_only'; },
    (agents) => { agents.agents.writer.deterministic_client_mitigations.ambiguous_create_recovery = 'retry_post'; },
    (agents) => { agents.agents.writer.deterministic_client_mitigations.unapproved = true; },
    (agents) => { agents.agents.writer.capability_residuals.app_has_branch_protection_bypass = true; },
    (agents) => { agents.agents.writer.deterministic_client_mitigations.denied_operations.pop(); },
    (agents) => { agents.agents.writer.required_actor_permissions.pop(); },
    (agents) => { agents.agents.writer.limits.maximum_files = 100; },
    (agents) => { agents.agents.writer.limits.maximum_patch_bytes = 65_535; },
    (agents) => { agents.agents.writer.limits.maximum_file_size_bytes = 1_048_576; },
    (agents) => { agents.agents.writer.limits.maximum_total_file_bytes = 2_097_151; },
    (agents) => { agents.agents.writer.limits.maximum_fix_cycles = 1; },
    (agents) => { agents.agents.writer.denied_paths.pop(); },
    (agents) => { agents.agents.writer.tools = ['exact_sha_merge']; },
    (agents) => { agents.agents.writer.unrecognized_capability = true; },
  ];
  for (const mutate of mutations) {
    const agents = structuredClone(contracts.agents);
    mutate(agents);
    assert.throws(() => validateContracts(agents, contracts.policy), ContractError);
  }

  const policy = structuredClone(contracts.policy);
  policy.writer.app_id_variable = 'AERIS_OTHER_APP_ID';
  assert.throws(() => validateContracts(contracts.agents, policy), ContractError);

  const reorderedPermissionKeys = structuredClone(contracts.agents);
  reorderedPermissionKeys.agents.writer.permissions = {
    denied: reorderedPermissionKeys.agents.writer.permissions.denied,
    pull_requests: 'write',
    contents: 'write',
    metadata: 'read',
  };
  assert.doesNotThrow(() => validateContracts(reorderedPermissionKeys, contracts.policy));

  const policyCapabilities = structuredClone(contracts.policy);
  policyCapabilities.writer.deterministic_client_mitigations.allowed_operations.push('merge');
  assert.throws(() => validateContracts(contracts.agents, policyCapabilities), ContractError);

  const policyAuthorization = structuredClone(contracts.policy);
  policyAuthorization.authorization.code_write_requires.actor_permission = ['admin'];
  assert.throws(() => validateContracts(contracts.agents, policyAuthorization), ContractError);

  const policyLabels = structuredClone(contracts.policy);
  policyLabels.authorization.code_write_requires.issue_labels = ['other-label'];
  assert.throws(() => validateContracts(contracts.agents, policyLabels), ContractError);

  const policyOpenPullRequests = structuredClone(contracts.policy);
  policyOpenPullRequests.writer.maximum_open_pull_requests_per_issue = 2;
  assert.throws(() => validateContracts(contracts.agents, policyOpenPullRequests), ContractError);

  const policyUnknownWriterField = structuredClone(contracts.policy);
  policyUnknownWriterField.writer.unrecognized_capability = true;
  assert.throws(() => validateContracts(contracts.agents, policyUnknownWriterField), ContractError);

  const policyRepositoryId = structuredClone(contracts.policy);
  policyRepositoryId.writer.repository_id += 1;
  assert.throws(() => validateContracts(contracts.agents, policyRepositoryId), ContractError);

  const policyCredentialField = structuredClone(contracts.policy);
  policyCredentialField.writer.credentials.installation_token = 'AERIS_WRITER_TOKEN';
  assert.throws(() => validateContracts(contracts.agents, policyCredentialField), ContractError);

  const policyReleaseSecretAccess = structuredClone(contracts.policy);
  policyReleaseSecretAccess.writer.release_secret_access = true;
  assert.throws(() => validateContracts(contracts.agents, policyReleaseSecretAccess), ContractError);

  const policyPullRequestTargetCheckout = structuredClone(contracts.policy);
  policyPullRequestTargetCheckout.writer.pull_request_target_checkout = true;
  assert.throws(() => validateContracts(contracts.agents, policyPullRequestTargetCheckout), ContractError);
});

test('Writer contracts reject retired close compensation even under coordinated drift', () => {
  const retiredRecovery = structuredClone(contracts);
  retiredRecovery.agents.agents.writer.deterministic_client_mitigations.ambiguous_create_recovery =
    'unique_attempt_marker_then_verified_close';
  retiredRecovery.policy.writer.deterministic_client_mitigations.ambiguous_create_recovery =
    'unique_attempt_marker_then_verified_close';
  assert.throws(
    () => validateContracts(retiredRecovery.agents, retiredRecovery.policy),
    ContractError,
  );

  const retiredOperation = structuredClone(contracts);
  for (const writer of [retiredOperation.agents.agents.writer, retiredOperation.policy.writer]) {
    writer.deterministic_client_mitigations.allowed_operations.push(
      'compensate_close_just_created_verified_draft_pull',
    );
  }
  assert.throws(
    () => validateContracts(retiredOperation.agents, retiredOperation.policy),
    ContractError,
  );

  const weakenedCloseDenial = structuredClone(contracts);
  for (const writer of [weakenedCloseDenial.agents.agents.writer, weakenedCloseDenial.policy.writer]) {
    writer.deterministic_client_mitigations.denied_operations =
      writer.deterministic_client_mitigations.denied_operations.filter((operation) => operation !== 'close_pr');
  }
  assert.throws(
    () => validateContracts(weakenedCloseDenial.agents, weakenedCloseDenial.policy),
    ContractError,
  );
});

test('protected global and Writer contracts reject coordinated cross-file drift', () => {
  const mutations = [
    (value) => {
      value.agents.runtime.enabled_variable = 'ATTACKER_AGENTS_ENABLED';
      value.policy.kill_switch.repository_variable = 'ATTACKER_AGENTS_ENABLED';
    },
    (value) => {
      value.agents.agents.writer.enabled_variable = 'ATTACKER_WRITER_ENABLED';
      value.policy.writer.enabled_variable = 'ATTACKER_WRITER_ENABLED';
    },
    (value) => {
      value.agents.agents.writer.repository_id = 42;
      value.policy.writer.repository_id = 42;
    },
    (value) => {
      value.agents.agents.writer.repository_name = 'attacker/repository';
      value.policy.writer.repository_name = 'attacker/repository';
    },
    (value) => {
      value.agents.agents.writer.app_id_variable = 'ATTACKER_WRITER_APP_ID';
      value.policy.writer.app_id_variable = 'ATTACKER_WRITER_APP_ID';
    },
    (value) => {
      value.agents.agents.writer.private_key_secret = 'ATTACKER_WRITER_PRIVATE_KEY';
      value.policy.writer.private_key_secret = 'ATTACKER_WRITER_PRIVATE_KEY';
    },
    (value) => {
      value.agents.agents.writer.environment = 'attacker-writer';
      value.policy.writer.environment = 'attacker-writer';
    },
    (value) => {
      value.agents.agents.writer.allowed_branch_prefixes = ['attacker/'];
      value.policy.writer.branch_prefix = 'attacker/';
    },
    (value) => {
      value.agents.agents.writer.denied_paths = ['attacker/**'];
      value.policy.writer.forbidden_paths = ['attacker/**'];
    },
  ];
  for (const mutate of mutations) {
    const forged = structuredClone(contracts);
    mutate(forged);
    assert.throws(() => validateContracts(forged.agents, forged.policy), ContractError);
  }

  for (const enabledValues of [
    ['true', '1'],
    ['1', 'TRUE'],
    ['1', 'true', 'yes'],
    ['1'],
    [],
    ['1', true],
    '1,true',
    null,
  ]) {
    const forged = structuredClone(contracts);
    forged.policy.kill_switch.enabled_values = enabledValues;
    assert.throws(() => validateContracts(forged.agents, forged.policy), ContractError);
  }
});

test('contract validation bounds the model output token budget', () => {
  for (const value of [undefined, null, '4000', 0, -1, 1.5, 16_385]) {
    const agents = structuredClone(contracts.agents);
    agents.runtime.limits.maximum_output_tokens = value;
    assert.throws(() => validateContracts(agents, contracts.policy), ContractError);
  }
  const agents = structuredClone(contracts.agents);
  agents.runtime.limits.maximum_output_tokens = 16_384;
  assert.doesNotThrow(() => validateContracts(agents, contracts.policy));
});

test('contract validation requires bounded exact reviewer limits', () => {
  const validLimits = {
    maximum_input_characters: 262_144,
    maximum_patch_characters_per_file: 65_536,
    request_timeout_seconds: 300,
  };
  const invalidValues = [
    { ...validLimits, maximum_input_characters: 23_999 },
    { ...validLimits, maximum_input_characters: 262_145 },
    { ...validLimits, maximum_patch_characters_per_file: 0 },
    { ...validLimits, maximum_patch_characters_per_file: 65_537 },
    { ...validLimits, maximum_input_characters: 24_000, maximum_patch_characters_per_file: 24_001 },
    { ...validLimits, maximum_input_characters: '262144' },
    { ...validLimits, request_timeout_seconds: 119 },
    { ...validLimits, request_timeout_seconds: 601 },
    { ...validLimits, request_timeout_seconds: '300' },
    { ...validLimits, extra: 1 },
    { maximum_input_characters: 262_144 },
  ];
  for (const limits of invalidValues) {
    const agents = structuredClone(contracts.agents);
    agents.runtime.reviewer_limits = limits;
    assert.throws(() => validateContracts(agents, contracts.policy), ContractError);
  }

  const agents = structuredClone(contracts.agents);
  agents.runtime.reviewer_limits = validLimits;
  assert.doesNotThrow(() => validateContracts(agents, contracts.policy));

  for (const boundary of [120, 600]) {
    const boundaryAgents = structuredClone(contracts.agents);
    boundaryAgents.runtime.reviewer_limits.request_timeout_seconds = boundary;
    assert.doesNotThrow(() => validateContracts(boundaryAgents, contracts.policy));
  }

  const equalTimeoutAgents = structuredClone(contracts.agents);
  equalTimeoutAgents.runtime.api.request_timeout_seconds = 300;
  equalTimeoutAgents.runtime.reviewer_limits.request_timeout_seconds = 300;
  assert.doesNotThrow(() => validateContracts(equalTimeoutAgents, contracts.policy));

  agents.runtime.api.request_timeout_seconds = 301;
  assert.throws(() => validateContracts(agents, contracts.policy), ContractError);
});

test('model candidates follow role, default, fallback order and deduplicate IDs', () => {
  const candidates = resolveModelCandidates('triage', contracts.agents.agents.triage, {
    AERIS_AI_MODEL_TRIAGE: 'fast-model',
    AERIS_AI_MODEL: 'fast-model',
    AERIS_AI_MODEL_FALLBACK: 'strong-model',
  });
  assert.deepEqual(candidates, [
    { alias: 'role', id: 'fast-model', variable: 'AERIS_AI_MODEL_TRIAGE' },
    { alias: 'fallback', id: 'strong-model', variable: 'AERIS_AI_MODEL_FALLBACK' },
  ]);
});

test('command parser requires one exact command and ignores managed content', () => {
  assert.equal(parseAgentCommand('/agent plan', policy), 'plan');
  assert.equal(parseAgentCommand('/agent plan\nplease do more', policy), null);
  assert.equal(parseAgentCommand('<!-- aeris-agent-managed -->\n/agent plan', policy), null);
});

test('Writer parser and route keep the valid disabled contract disabled', async () => {
  for (const value of [
    '/agent implement', '/agent retry-write',
  ]) assert.equal(parseWriterCommand(value, policy), value);
  for (const value of [
    'implement', 'retry-write', '/agent Implement', '/agent implement ',
    ' /agent implement', '/agent implement now', '/agent retry-write\nplease',
  ]) assert.equal(parseWriterCommand(value, policy), null, value);

  const event = {
    sender: { login: 'maintainer' },
    action: 'created',
    issue: { number: 41, state: 'closed', labels: [] },
    comment: { id: 91, body: '/agent retry-write', user: { login: 'maintainer' } },
  };
  const trustedContracts = structuredClone(contracts);
  const environment = { AERIS_AGENTS_ENABLED: 'true', AERIS_WRITER_ENABLED: 'true' };
  const liveComment = writerComment();
  const liveIssue = writerIssue();
  const github = {
    getRepository: async () => writerRepository,
    getIssueComment: async () => liveComment,
    getIssue: async () => liveIssue,
    getCollaboratorPermission: async () => 'write',
  };
  assert.deepEqual(writerSwitchesFromTrustedContracts({ trustedContracts, environment }), {
    globalEnabled: true, writerVariableEnabled: true, writerContractEnabled: false,
  });
  assert.deepEqual(await routeWriterInvocation({
    eventName: 'issue_comment', event, github, trustedContracts, environment, fixCycle: 0,
  }), {
    action: 'skip', reason: 'writer_disabled',
  });
  assert.equal((await routeWriterInvocation({
    eventName: 'issue_comment', event, github: { ...github, getCollaboratorPermission: async () => 'read' },
    trustedContracts, environment, fixCycle: 0,
  })).reason, 'insufficient_permission');
  assert.equal((await routeWriterInvocation({
    eventName: 'issue_comment', event, github, trustedContracts,
    environment: { ...environment, AERIS_WRITER_ENABLED: 'false' }, fixCycle: 0,
  })).reason, 'writer_disabled');
});

test('Writer rejects non-created events and live-state drift', async () => {
  const trustedContracts = structuredClone(contracts);
  const environment = { AERIS_AGENTS_ENABLED: 'true', AERIS_WRITER_ENABLED: '1' };
  const event = {
    sender: { login: 'maintainer' }, action: 'created',
    issue: { number: 41, state: 'open', labels: [{ name: 'agent-ready' }] },
    comment: { id: 91, body: '/agent implement', user: { login: 'maintainer' } },
  };
  const github = {
    getRepository: async () => writerRepository,
    getIssueComment: async () => writerComment(),
    getIssue: async () => writerIssue(),
    getCollaboratorPermission: async () => 'write',
  };
  assert.equal((await routeWriterInvocation({
    eventName: 'issue_comment', event: { ...event, action: 'edited' }, github, trustedContracts, environment, fixCycle: 0,
  })).reason, 'unsupported_event');
  assert.equal((await routeWriterInvocation({
    eventName: 'issue_comment', event: { ...event, action: 'deleted' }, github, trustedContracts, environment, fixCycle: 0,
  })).reason, 'unsupported_event');
  assert.equal((await routeWriterInvocation({
    eventName: 'issue_comment', event: { ...event, sender: { login: 'other-user' } }, github, trustedContracts, environment, fixCycle: 0,
  })).reason, 'comment_author_mismatch');
  assert.equal((await routeWriterInvocation({
    eventName: 'issue_comment', event, github: { ...github, getIssueComment: async () => writerComment({ user: { login: 'other-user' } }) }, trustedContracts, environment, fixCycle: 0,
  })).reason, 'comment_author_mismatch');
  let permissionCalls = 0;
  assert.equal((await routeWriterInvocation({
    eventName: 'issue_comment', event,
    github: {
      ...github,
      getIssueComment: async () => writerComment({ id: 92 }),
      getCollaboratorPermission: async () => { permissionCalls += 1; return 'write'; },
    },
    trustedContracts, environment, fixCycle: 0,
  })).reason, 'writer_live_validation_failed');
  assert.equal(permissionCalls, 0);
  assert.equal((await routeWriterInvocation({
    eventName: 'issue_comment', event,
    github: {
      ...github,
      getIssue: async () => writerIssue({ number: 42 }),
      getCollaboratorPermission: async () => { permissionCalls += 1; return 'write'; },
    },
    trustedContracts, environment, fixCycle: 0,
  })).reason, 'writer_live_validation_failed');
  assert.equal(permissionCalls, 0);
  assert.equal((await routeWriterInvocation({
    eventName: 'issue_comment', event, github: { ...github, getIssue: async () => writerIssue({ state: 'closed' }) }, trustedContracts, environment, fixCycle: 0,
  })).reason, 'writer_disabled');
  assert.equal((await routeWriterInvocation({
    eventName: 'issue_comment', event, github: { ...github, getIssue: async () => writerIssue({ labels: [] }) }, trustedContracts, environment, fixCycle: 0,
  })).reason, 'writer_disabled');
  assert.equal((await routeWriterInvocation({
    eventName: 'issue_comment', event, github: { ...github, getCollaboratorPermission: async () => null }, trustedContracts, environment, fixCycle: 0,
  })).reason, 'insufficient_permission');
  assert.equal((await routeWriterInvocation({
    eventName: 'issue_comment', event, github: { ...github, getIssueComment: async () => { throw new Error('deleted'); } }, trustedContracts, environment, fixCycle: 0,
  })).reason, 'writer_live_validation_failed');
  assert.equal((await routeWriterInvocation({
    eventName: 'issue_comment', event,
    github: { ...github, getRepository: async () => ({ ...writerRepository, id: writerRepository.id + 1 }) },
    trustedContracts, environment, fixCycle: 0,
  })).reason, 'writer_repository_mismatch');

  for (const mismatch of [
    writerComment({ issue_url: `https://api.github.com/repos/${writerRepository.full_name}/issues/42` }),
    writerComment({ issue_url: 'https://api.github.com/repos/attacker/repository/issues/41' }),
    writerComment({ issue_url: `${writerIssueUrl}?redirect=1` }),
    writerComment({ issue_url: 'https://api.github.com/repos/jinpenggeng/aeris-token/issues/41' }),
    writerComment({ issue_url: 'https://api.github.com/repos/JinPengGeng/aeris-token.evil/issues/41' }),
    writerComment({ issue_url: 'https://api.github.com/repos/JinPengGeng/aeris%2Dtoken/issues/41' }),
    writerComment({ issue_url: null }),
  ]) {
    assert.equal((await routeWriterInvocation({
      eventName: 'issue_comment', event,
      github: { ...github, getIssueComment: async () => mismatch },
      trustedContracts, environment, fixCycle: 0,
    })).reason, 'comment_issue_mismatch');
  }

  const coordinatedCrossRepository = 'https://api.github.com/repos/attacker/repository/issues/41';
  assert.equal((await routeWriterInvocation({
    eventName: 'issue_comment', event,
    github: {
      ...github,
      getIssueComment: async () => writerComment({ issue_url: coordinatedCrossRepository }),
      getIssue: async () => writerIssue({ url: coordinatedCrossRepository }),
    },
    trustedContracts, environment, fixCycle: 0,
  })).reason, 'comment_issue_mismatch');
});

test('Writer publish boundary re-reads and binds comment, Issue, actor permission, and command', async () => {
  const environment = { AERIS_AGENTS_ENABLED: 'true', AERIS_WRITER_ENABLED: 'true' };
  const intent = {
    repository_id: writerRepository.id,
    repository_name: writerRepository.full_name,
    issue_number: 41,
    issue_updated_at: '2026-08-18T09:00:00Z',
    comment_id: 91,
    actor: 'maintainer',
    command: '/agent implement',
  };
  let permissionCalls = 0;
  const github = {
    getRepository: async () => writerRepository,
    getIssueComment: async () => writerComment(),
    getIssue: async () => writerIssue(),
    getCollaboratorPermission: async () => { permissionCalls += 1; return 'write'; },
  };

  assert.equal((await revalidateWriterPublishBoundary({
    intent, github, trustedContracts: contracts, environment, fixCycle: 0,
  })).reason, 'writer_disabled');
  assert.equal(permissionCalls, 1);

  const adversarial = [
    [{ ...intent, comment_id: 0 }, github, 'writer_publish_binding_invalid'],
    [{ ...intent, repository_id: writerRepository.id + 1 }, github, 'writer_publish_binding_invalid'],
    [{ ...intent, repository_name: 'attacker/repository' }, github, 'writer_publish_binding_invalid'],
    [intent, { ...github, getRepository: async () => ({ ...writerRepository, id: writerRepository.id + 1 }) }, 'writer_publish_repository_changed'],
    [intent, { ...github, getRepository: async () => ({ ...writerRepository, full_name: 'jinpenggeng/aeris-token' }) }, 'writer_publish_repository_changed'],
    [intent, { ...github, getIssueComment: async () => writerComment({ issue_url: 'https://api.github.com/repos/attacker/repository/issues/41' }) }, 'writer_publish_binding_invalid'],
    [intent, { ...github, getIssueComment: async () => writerComment({ user: { login: 'other-user' } }) }, 'writer_publish_actor_changed'],
    [intent, { ...github, getIssueComment: async () => writerComment({ body: '/agent retry-write' }) }, 'writer_publish_command_changed'],
    [intent, { ...github, getIssue: async () => writerIssue({ updated_at: '2026-08-18T09:00:01Z' }) }, 'writer_publish_issue_changed'],
    [intent, { ...github, getIssue: async () => writerIssue({ state: 'closed' }) }, 'issue_not_open'],
    [intent, { ...github, getIssue: async () => writerIssue({ labels: [] }) }, 'missing_agent_ready_label'],
    [intent, { ...github, getCollaboratorPermission: async () => 'read' }, 'insufficient_permission'],
  ];
  for (const [candidateIntent, candidateGitHub, reason] of adversarial) {
    assert.equal((await revalidateWriterPublishBoundary({
      intent: candidateIntent,
      github: candidateGitHub,
      trustedContracts: contracts,
      environment,
      fixCycle: 0,
    })).reason, reason);
  }

  const coordinatedCrossRepository = 'https://api.github.com/repos/attacker/repository/issues/41';
  assert.equal((await revalidateWriterPublishBoundary({
    intent,
    github: {
      ...github,
      getIssueComment: async () => writerComment({ issue_url: coordinatedCrossRepository }),
      getIssue: async () => writerIssue({ url: coordinatedCrossRepository }),
    },
    trustedContracts: contracts,
    environment,
    fixCycle: 0,
  })).reason, 'writer_publish_binding_invalid');
});

test('Writer switches accept only exact literal enable values', async () => {
  for (const value of ['1', 'true']) {
    assert.deepEqual(writerSwitchesFromTrustedContracts({
      trustedContracts: contracts,
      environment: { AERIS_AGENTS_ENABLED: value, AERIS_WRITER_ENABLED: value },
    }), {
      globalEnabled: true, writerVariableEnabled: true, writerContractEnabled: false,
    });
  }
  for (const value of [' 1', '1 ', ' TRUE ', 'True', 'TRUE', ' true']) {
    assert.deepEqual(writerSwitchesFromTrustedContracts({
      trustedContracts: contracts,
      environment: { AERIS_AGENTS_ENABLED: value, AERIS_WRITER_ENABLED: value },
    }), {
      globalEnabled: false, writerVariableEnabled: false, writerContractEnabled: false,
    }, value);
    const route = await routeWriterInvocation({
      eventName: 'issue_comment',
      event: {
        sender: { login: 'maintainer' }, action: 'created', issue: { number: 41 },
        comment: { id: 91, user: { login: 'maintainer' } },
      },
      github: {
        getRepository: async () => writerRepository,
        getIssueComment: async () => writerComment(),
        getIssue: async () => writerIssue(),
        getCollaboratorPermission: async () => 'write',
      },
      trustedContracts: contracts,
      environment: { AERIS_AGENTS_ENABLED: value, AERIS_WRITER_ENABLED: value },
      fixCycle: 0,
    });
    assert.notEqual(route.action, 'write', value);
  }
});

test('Writer switches fail closed for forged or malformed contracts', async () => {
  const environment = {
    AERIS_AGENTS_ENABLED: 'true',
    AERIS_WRITER_ENABLED: 'true',
    ATTACKER_AGENTS_ENABLED: 'true',
  };
  const invalidContracts = [
    (value) => { value.agents.agents.writer.enabled = true; },
    (value) => { value.agents.agents.writer.enabled_variable = 'ATTACKER_WRITER_ENABLED'; },
    (value) => { value.policy.kill_switch.repository_variable = 'ATTACKER_AGENTS_ENABLED'; },
    (value) => {
      value.agents.runtime.enabled_variable = 'ATTACKER_AGENTS_ENABLED';
      value.policy.kill_switch.repository_variable = 'ATTACKER_AGENTS_ENABLED';
    },
    (value) => { value.policy.kill_switch.enabled_values = ['enable-me']; },
    (value) => { value.policy.kill_switch.enabled_values = ['1', 'true', 'yes']; },
    (value) => { value.agents.agents.writer.permissions.issues = 'write'; },
    (value) => { value.policy.writer.unapproved_field = true; },
  ];
  for (const mutate of invalidContracts) {
    const forged = structuredClone(contracts);
    mutate(forged);
    assert.deepEqual(writerSwitchesFromTrustedContracts({ trustedContracts: forged, environment }), {
      globalEnabled: false, writerVariableEnabled: false, writerContractEnabled: false,
    });
  }

  const event = {
    sender: { login: 'maintainer' }, action: 'created',
    issue: { number: 41 }, comment: { id: 91, user: { login: 'maintainer' } },
  };
  const github = {
    getRepository: async () => writerRepository,
    getIssueComment: async () => writerComment(),
    getIssue: async () => writerIssue(),
    getCollaboratorPermission: async () => 'write',
  };
  const forged = structuredClone(contracts);
  forged.agents.agents.writer.enabled = true;
  assert.equal((await routeWriterInvocation({
    eventName: 'issue_comment', event, github, trustedContracts: forged, environment, fixCycle: 0,
  })).reason, 'writer_disabled');
});

test('Issue routing gates external analysis with agent-analyze', () => {
  const baseEvent = {
    action: 'opened',
    sender: { login: 'outside-user' },
    issue: { author_association: 'NONE', labels: [] },
  };
  assert.equal(
    routeIssueInvocation({ eventName: 'issues', event: baseEvent, policy }).reason,
    'external_issue_requires_label',
  );
  const labeled = structuredClone(baseEvent);
  labeled.issue.labels.push({ name: 'agent-analyze' });
  assert.deepEqual(routeIssueInvocation({ eventName: 'issues', event: labeled, policy }), {
    action: 'analyze',
    agent: 'triage',
    reason: 'issue_opened',
  });
});

test('Issue commands allow public status but restrict model calls', () => {
  const event = {
    sender: { login: 'outside-user' },
    issue: { number: 3 },
    comment: { body: '/agent status', author_association: 'NONE' },
  };
  assert.equal(
    routeIssueInvocation({ eventName: 'issue_comment', event, policy }).action,
    'status',
  );
  event.comment.body = '/agent triage';
  assert.equal(
    routeIssueInvocation({ eventName: 'issue_comment', event, policy }).reason,
    'command_not_authorized',
  );
  event.comment.author_association = 'MEMBER';
  assert.equal(
    routeIssueInvocation({ eventName: 'issue_comment', event, policy }).agent,
    'triage',
  );
});

test('PR workflow routing gates external authors and never routes to writer', () => {
  const headSha = 'c'.repeat(40);
  const event = {
    action: 'completed',
    sender: { login: 'github-actions' },
    workflow_run: { conclusion: 'success', head_sha: headSha },
  };
  const pull = {
    author_association: 'CONTRIBUTOR',
    labels: [],
    draft: false,
    head: { sha: headSha },
  };
  assert.equal(
    routePullInvocation({ eventName: 'workflow_run', event, pull, policy }).reason,
    'external_pull_request_requires_label',
  );
  pull.labels.push({ name: 'agent-analyze' });
  const decision = routePullInvocation({ eventName: 'workflow_run', event, pull, policy });
  assert.equal(decision.agent, 'reviewer');
  assert.notEqual(decision.agent, 'writer');
});

test('PR workflow routing accepts any completed run for the current head', () => {
  const currentHead = 'c'.repeat(40);
  const pull = {
    author_association: 'MEMBER',
    labels: [],
    draft: false,
    head: { sha: currentHead },
  };
  const event = {
    action: 'completed',
    workflow_run: { conclusion: 'failure', head_sha: currentHead },
  };

  for (const conclusion of ['failure', 'cancelled']) {
    event.workflow_run.conclusion = conclusion;
    assert.deepEqual(routePullInvocation({ eventName: 'workflow_run', event, pull, policy }), {
      action: 'analyze',
      agent: 'reviewer',
      reason: 'required_workflow_completed',
    });
  }

  delete event.workflow_run.head_sha;
  assert.equal(
    routePullInvocation({ eventName: 'workflow_run', event, pull, policy }).reason,
    'workflow_run_head_missing',
  );

  event.workflow_run.head_sha = 'd'.repeat(40);
  assert.equal(
    routePullInvocation({ eventName: 'workflow_run', event, pull, policy }).reason,
    'workflow_run_head_stale',
  );

  event.workflow_run.head_sha = currentHead;
  assert.deepEqual(routePullInvocation({ eventName: 'workflow_run', event, pull, policy }), {
    action: 'analyze',
    agent: 'reviewer',
    reason: 'required_workflow_completed',
  });
});

test('PR workflow routing rejects a missing current pull head', () => {
  const event = {
    action: 'completed',
    workflow_run: { conclusion: 'success', head_sha: 'c'.repeat(40) },
  };
  const pull = { author_association: 'MEMBER', labels: [], draft: false, head: {} };
  assert.equal(
    routePullInvocation({ eventName: 'workflow_run', event, pull, policy }).reason,
    'pull_request_head_missing',
  );
});

test('PR workflow freshness checks do not apply to authorized commands or dispatches', () => {
  const commandEvent = {
    sender: { login: 'maintainer' },
    issue: { number: 7, pull_request: {} },
    comment: { body: '/agent review', author_association: 'MEMBER' },
  };
  assert.equal(
    routePullInvocation({ eventName: 'issue_comment', event: commandEvent, policy }).agent,
    'reviewer',
  );
  assert.equal(
    routePullInvocation({
      eventName: 'workflow_dispatch',
      event: {},
      pull: { draft: false },
      manualAgent: 'review',
      actorCanWrite: true,
      policy,
    }).agent,
    'reviewer',
  );
});

test('bot-authored managed comments cannot trigger routing', () => {
  const event = {
    sender: { login: 'github-actions[bot]' },
    issue: { number: 1 },
    comment: { body: '/agent plan', author_association: 'MEMBER' },
  };
  assert.equal(
    routeIssueInvocation({ eventName: 'issue_comment', event, policy }).reason,
    'ignored_actor',
  );
});

test('command authorization remains required for cancel and PR review', () => {
  const issueEvent = {
    sender: { login: 'outside-user' },
    issue: { number: 1 },
    comment: { body: '/agent cancel', author_association: 'NONE' },
  };
  assert.equal(
    routeIssueInvocation({ eventName: 'issue_comment', event: issueEvent, policy }).reason,
    'command_not_authorized',
  );

  const pullEvent = {
    sender: { login: 'outside-user' },
    issue: { number: 7, pull_request: {} },
    comment: { body: '/agent review', author_association: 'NONE' },
  };
  assert.equal(
    routePullInvocation({ eventName: 'issue_comment', event: pullEvent, policy }).reason,
    'command_not_authorized',
  );
  pullEvent.comment.author_association = 'MEMBER';
  assert.equal(
    routePullInvocation({ eventName: 'issue_comment', event: pullEvent, policy }).agent,
    'reviewer',
  );
});

test('canonical input fingerprints are stable across object key order', () => {
  const first = { z: 1, nested: { b: 2, a: 1 }, list: [{ d: 4, c: 3 }] };
  const reordered = { list: [{ c: 3, d: 4 }], nested: { a: 1, b: 2 }, z: 1 };
  assert.equal(canonicalInput(first), canonicalInput(reordered));
  assert.equal(inputFingerprint(first), inputFingerprint(reordered));
  assert.equal(hashInput(first), inputFingerprint(first));
});

test('Issue fingerprint covers title, body, labels, and available labels', () => {
  const input = buildIssueInput(
    {
      number: 3,
      html_url: 'https://github.test/example/repo/issues/3',
      title: 'Request fails',
      body: 'Failure details',
      labels: [{ name: 'type:bug' }],
      author_association: 'MEMBER',
    },
    { maximumCharacters: 20_000, repositoryLabels: ['type:bug', 'agent-ready'] },
  );
  const fingerprint = inputFingerprint(input);
  const mutations = [
    (candidate) => { candidate.title = 'Different title'; },
    (candidate) => { candidate.body = 'Different body'; },
    (candidate) => { candidate.labels.push('priority:high'); },
    (candidate) => { candidate.available_labels.push('priority:high'); },
  ];
  for (const mutate of mutations) {
    const candidate = structuredClone(input);
    mutate(candidate);
    assert.notEqual(inputFingerprint(candidate), fingerprint);
  }
});

test('PR fingerprint covers title, body, labels, refs, SHAs, files, and patches', () => {
  const input = buildPullInput(
    {
      number: 7,
      html_url: 'https://github.test/example/repo/pull/7',
      title: 'Fix request',
      body: 'Fixes the request.',
      author_association: 'MEMBER',
      labels: [{ name: 'type:bug' }],
      changed_files: 1,
      base: { ref: 'main', sha: 'b'.repeat(40) },
      head: { ref: 'fix', sha: 'c'.repeat(40) },
    },
    {
      files: [{
        filename: 'src/request.ts',
        status: 'modified',
        additions: 2,
        deletions: 1,
        changes: 3,
        patch: '@@ -1 +1 @@\n-old\n+new',
      }],
      truncated: false,
    },
    { maximumCharacters: 20_000 },
  );
  const fingerprint = inputFingerprint(input);
  const mutations = [
    (candidate) => { candidate.title = 'Different title'; },
    (candidate) => { candidate.body = 'Different body'; },
    (candidate) => { candidate.labels.push('priority:high'); },
    (candidate) => { candidate.base.ref = 'release'; },
    (candidate) => { candidate.base.sha = 'd'.repeat(40); },
    (candidate) => { candidate.head.ref = 'other-fix'; },
    (candidate) => { candidate.head.sha = 'e'.repeat(40); },
    (candidate) => { candidate.files[0].additions += 1; },
    (candidate) => { candidate.files[0].patch = '@@ -1 +1 @@\n-old\n+different'; },
  ];
  for (const mutate of mutations) {
    const candidate = structuredClone(input);
    mutate(candidate);
    assert.notEqual(inputFingerprint(candidate), fingerprint);
  }
});

test('Reviewer pull input retains every file record while bounding patches to its dedicated budget', () => {
  const input = buildPullInput(
    {
      number: 7,
      html_url: 'https://github.test/example/repo/pull/7',
      title: 'Review a large pull request',
      body: '',
      author_association: 'MEMBER',
      labels: [],
      changed_files: 3,
      base: { ref: 'main', sha: 'b'.repeat(40) },
      head: { ref: 'review/full-diff', sha: 'c'.repeat(40) },
    },
    {
      files: [
        { filename: 'src/first.ts', status: 'modified', additions: 1, deletions: 1, changes: 2, patch: 'a'.repeat(100) },
        { filename: 'src/second.ts', status: 'modified', additions: 2, deletions: 2, changes: 4, patch: 'b'.repeat(100) },
        { filename: 'src/third.ts', status: 'added', additions: 3, deletions: 0, changes: 3, patch: 'c'.repeat(100) },
      ],
      truncated: false,
    },
    { maximumCharacters: 1_000, maximumPatchCharactersPerFile: 64 },
  );

  assert.deepEqual(input.files.map((file) => file.path), [
    'src/first.ts', 'src/second.ts', 'src/third.ts',
  ]);
  assert.equal(input.files.length, 3);
  assert.ok(input.files.every((file) => file.patch === null || file.patch.length <= 64));
  assert.ok(input.files.some((file) => file.patch_truncated));
  assert.equal(input.truncated, true);
  assert.ok(JSON.stringify(input).length <= 1_000);
});

test('Reviewer pull input fails closed when all file metadata cannot fit the total budget', () => {
  const files = Array.from({ length: 4 }, (_, index) => ({
    filename: `src/${index}.ts`, status: 'modified', additions: index, deletions: 0, changes: index,
    patch: 'x'.repeat(128),
  }));
  assert.throws(
    () => buildPullInput(
      {
        number: 7, html_url: 'https://github.test/example/repo/pull/7', title: 'Review', body: '',
        author_association: 'MEMBER', labels: [], changed_files: files.length,
        base: { ref: 'main', sha: 'b'.repeat(40) }, head: { ref: 'fix', sha: 'c'.repeat(40) },
      },
      { files, truncated: false },
      { maximumCharacters: 1, maximumPatchCharactersPerFile: 64 },
    ),
    /metadata exceeds the maximum input size/,
  );
});

test('Reviewer pull input fails closed on incomplete file pagination or count mismatch', () => {
  const pull = {
    number: 7, html_url: 'https://github.test/example/repo/pull/7', title: 'Review', body: '',
    author_association: 'MEMBER', labels: [], changed_files: 1,
    base: { ref: 'main', sha: 'b'.repeat(40) }, head: { ref: 'fix', sha: 'c'.repeat(40) },
  };
  const file = {
    filename: 'src/index.ts', status: 'modified', additions: 1, deletions: 0, changes: 1, patch: '+value',
  };

  for (const pullFiles of [
    { files: [file], truncated: true },
    { files: [], truncated: false },
  ]) {
    assert.throws(
      () => buildPullInput(
        pull,
        pullFiles,
        { maximumCharacters: 1_000, maximumPatchCharactersPerFile: 64 },
      ),
      /file list is incomplete/,
    );
  }
});

test('source keys are stable derived identities for supported events', () => {
  const object = { id: 101, number: 3, updated_at: '2026-08-12T00:00:00Z', head: { sha: 'c'.repeat(40) } };
  assert.equal(
    sourceKey('workflow_run', { workflow_run: { id: 999 } }, object, {}),
    `pull:3:${'c'.repeat(40)}`,
  );
  assert.equal(sourceKey('issue_comment', { comment: { id: 44 } }, object, {}), 'comment:44');
});
