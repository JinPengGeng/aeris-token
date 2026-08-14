import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  ContractError,
  loadContracts,
  resolveModelCandidates,
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

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, '..', '..', '..');
const contracts = loadContracts(repoRoot);
const policy = contracts.policy;

test('trusted contracts load with only enabled agents declared', () => {
  assert.equal(Object.keys(contracts.agents.agents).length, 8);
  const enabled = Object.entries(contracts.agents.agents)
    .filter(([, agent]) => agent.enabled)
    .map(([name]) => name);
  assert.deepEqual(enabled, ['triage']);
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

test('source keys are stable derived identities for supported events', () => {
  const object = { id: 101, number: 3, updated_at: '2026-08-12T00:00:00Z', head: { sha: 'c'.repeat(40) } };
  assert.equal(
    sourceKey('workflow_run', { workflow_run: { id: 999 } }, object, {}),
    `pull:3:${'c'.repeat(40)}`,
  );
  assert.equal(sourceKey('issue_comment', { comment: { id: 44 } }, object, {}), 'comment:44');
});
