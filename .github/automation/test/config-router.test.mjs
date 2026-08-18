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
  routeIssueInvocation,
  routePullInvocation,
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
  const commands = policy.authorization.code_write_requires.exact_commands.map(
    (command) => canonicalWriterCommand(policy.commands.prefix, command),
  );
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
  assert.equal(writer.private_key_secret, 'AERIS_WRITER_PRIVATE_KEY');
  assert.equal(writer.environment, 'writer');
  assert.deepEqual(writer.required_actor_permissions, ['admin', 'maintain', 'write']);
  assert.deepEqual(writer.required_commands, ['implement', 'retry-write']);
  assert.deepEqual(contracts.policy.authorization.code_write_requires, {
    actor_permission: ['admin', 'maintain', 'write'],
    exact_commands: ['implement', 'retry-write'],
    issue_labels: ['agent-ready'],
  });
  assert.deepEqual(writer.credentials, { allowed_jobs: ['publish'], github_token_write: false });
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
    allowed_operations: ['create_or_update_agent_ref', 'create_or_update_draft_pull_request'],
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
    (agents) => { agents.agents.writer.identity = 'github_token'; },
    (agents) => { agents.agents.writer.credentials.github_token_write = true; },
    (agents) => { agents.agents.writer.permissions.denied.pop(); },
    (agents) => { agents.agents.writer.permissions.issues = 'write'; },
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

  const policyReleaseSecretAccess = structuredClone(contracts.policy);
  policyReleaseSecretAccess.writer.release_secret_access = true;
  assert.throws(() => validateContracts(contracts.agents, policyReleaseSecretAccess), ContractError);

  const policyPullRequestTargetCheckout = structuredClone(contracts.policy);
  policyPullRequestTargetCheckout.writer.pull_request_target_checkout = true;
  assert.throws(() => validateContracts(contracts.agents, policyPullRequestTargetCheckout), ContractError);
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
