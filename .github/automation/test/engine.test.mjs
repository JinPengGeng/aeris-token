import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import { loadContracts } from '../src/config.mjs';
import {
  runAnalysisPhase,
  runAutomation,
  runPreflightPhase,
  runPublishPhase,
  runReservationPhase,
} from '../src/engine.mjs';
import { validateReservationArtifact } from '../src/phase-contract.mjs';
import {
  decodeMetadata,
  MANAGED_MARKER,
  renderAnalysisComment,
  renderStatusComment,
} from '../src/managed-comment.mjs';
import { validateAgentOutput } from '../src/schemas.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, '..', '..', '..');
const policySha = 'a'.repeat(40);

function enabledContracts(...names) {
  const contracts = structuredClone(loadContracts(repoRoot));
  for (const name of names) contracts.agents.agents[name].enabled = true;
  return contracts;
}

class FakeGitHub {
  constructor() {
    this.issue = {
      id: 101,
      number: 1,
      html_url: 'https://github.test/example/repo/issues/1',
      title: 'Broken request',
      body: 'The request fails.',
      labels: [{ name: 'status:triage' }],
      author_association: 'MEMBER',
      updated_at: '2026-08-11T00:00:00Z',
    };
    this.pull = {
      id: 202,
      number: 7,
      html_url: 'https://github.test/example/repo/pull/7',
      title: 'Fix request',
      body: 'Fixes the request.',
      labels: [],
      author_association: 'MEMBER',
      draft: false,
      changed_files: 1,
      base: { ref: 'main', sha: 'b'.repeat(40) },
      head: { ref: 'fix', sha: 'c'.repeat(40) },
    };
    this.pullFiles = {
      files: [
        {
          filename: 'src/request.ts',
          status: 'modified',
          additions: 2,
          deletions: 1,
          changes: 3,
          patch: '@@ -1 +1 @@\n-old\n+new',
        },
      ],
      truncated: false,
    };
    this.comments = [];
    this.nextCommentId = 1;
    this.checkRuns = [
      { id: 1, name: 'Rust CI / check', status: 'completed', conclusion: 'success', completed_at: '2026-08-11T00:01:00Z' },
      { id: 2, name: 'Frontend CI / check', status: 'completed', conclusion: 'success', completed_at: '2026-08-11T00:01:00Z' },
    ];
    this.commitStatuses = [];
    this.requiredCheckQueries = 0;
    this.pullReads = 0;
    this.pullHeadAfterFirstRead = null;
  }

  async getIssue() {
    return structuredClone(this.issue);
  }

  async getPull() {
    this.pullReads += 1;
    if (this.pullReads > 1 && this.pullHeadAfterFirstRead) {
      this.pull.head.sha = this.pullHeadAfterFirstRead;
    }
    return structuredClone(this.pull);
  }

  async listRepositoryLabels() {
    return ['type:bug', 'status:triage', 'agent-ready'];
  }

  async listPullFiles() {
    return structuredClone(this.pullFiles);
  }

  async listCheckRunsForRef() {
    this.requiredCheckQueries += 1;
    return structuredClone(this.checkRuns);
  }

  async listCommitStatuses() {
    this.requiredCheckQueries += 1;
    return structuredClone(this.commitStatuses);
  }

  touchIssue(at = '2026-08-11T00:02:00Z') {
    this.issue.updated_at = at;
  }

  async listIssueComments() {
    return structuredClone(this.comments);
  }

  async createIssueComment(_number, body) {
    const comment = {
      id: this.nextCommentId++,
      body,
      user: { login: 'github-actions[bot]' },
      updated_at: `2026-08-11T00:00:0${this.nextCommentId}Z`,
    };
    this.comments.push(comment);
    return structuredClone(comment);
  }

  async updateIssueComment(commentId, body) {
    const comment = this.comments.find((entry) => entry.id === commentId);
    comment.body = body;
    comment.updated_at = '2026-08-11T00:02:00Z';
    return structuredClone(comment);
  }

  async getCollaboratorPermission() {
    return 'write';
  }
}

function environment(overrides = {}) {
  return {
    AERIS_AGENTS_ENABLED: 'true',
    AERIS_AI_BASE_URL: 'https://ai.example.test/v1',
    AERIS_AI_API_KEY: 'test-key',
    AERIS_AI_MODEL: 'test-model',
    GITHUB_ACTOR: 'maintainer',
    GITHUB_REPOSITORY: 'example/repo',
    GITHUB_RUN_ID: '500',
    GITHUB_RUN_ATTEMPT: '1',
    ...overrides,
  };
}

function triageCompletion(onComplete = null) {
  let calls = 0;
  return {
    factory: () => ({
      async complete(request) {
        calls += 1;
        onComplete?.(request);
        return {
          content: JSON.stringify({
            schema_version: 1,
            agent: 'triage',
            summary: 'Confirmed request failure.',
            risk: 'medium',
            proposed_labels: ['type:bug'],
            missing_information: ['Exact response status'],
            recommended_action: 'Collect the response status.',
            next_agent: 'planner',
          }),
          model: { alias: 'default', id: 'test-model' },
          durationMs: 12,
          usage: { total_tokens: 20 },
        };
      },
    }),
    calls: () => calls,
  };
}

function managedMetadata(github) {
  return decodeMetadata(github.comments[0]?.body);
}

function runningComment(github, metadata) {
  github.comments = [
    {
      id: 1,
      body: `${MANAGED_MARKER}\n<!-- aeris-agent-meta:${Buffer.from(
        JSON.stringify(metadata),
        'utf8',
      ).toString('base64url')} -->`,
      user: { login: 'github-actions[bot]' },
      updated_at: '2026-08-11T00:00:01Z',
    },
  ];
  github.nextCommentId = 2;
}

function fullReplayLedger() {
  return Array.from({ length: 32 }, (_, index) => ({
    source_key: `comment:${100000000000000000n + BigInt(index)}`,
    agent: 'reviewer',
    input_sha: String(index).padStart(64, '0'),
    policy_sha: policySha,
    result: 'completed',
    at: `2026-08-11T00:${String(index).padStart(2, '0')}:00.000Z`,
  }));
}

const issueEvent = {
  action: 'opened',
  sender: { login: 'maintainer' },
  issue: {
    id: 101,
    number: 1,
    updated_at: '2026-08-11T00:00:00Z',
    author_association: 'MEMBER',
    labels: [],
  },
};

test('Issue triage publishes one managed comment and replay is a no-op', async () => {
  const github = new FakeGitHub();
  const ai = triageCompletion();
  const options = {
    kind: 'issue',
    eventName: 'issues',
    event: issueEvent,
    environment: environment(),
    repoRoot,
    contracts: enabledContracts('triage'),
    policySha,
    github,
    aiClientFactory: ai.factory,
  };
  const first = await runAutomation(options);
  assert.equal(first.state, 'published');
  assert.equal(github.comments.length, 1);
  assert.equal(github.comments[0].body.startsWith(MANAGED_MARKER), true);
  assert.equal(decodeMetadata(github.comments[0].body).agent, 'triage');

  const replay = await runAutomation(options);
  assert.equal(replay.state, 'noop');
  assert.equal(github.comments.length, 1);
  assert.equal(ai.calls(), 1);
});

test('analysis passes the output budget without enabling structured output for triage', async () => {
  const github = new FakeGitHub();
  const currentContracts = enabledContracts('triage');
  let request;
  const ai = triageCompletion((value) => { request = value; });
  await runAutomation({
    kind: 'issue', eventName: 'issues', event: issueEvent, environment: environment(), repoRoot,
    contracts: currentContracts, policySha, github, aiClientFactory: ai.factory,
  });
  assert.equal(request.maxTokens, currentContracts.agents.runtime.limits.maximum_output_tokens);
  assert.equal(request.responseFormat, undefined);
});

test('planner canary sends its strict response format to the AI client', async () => {
  const github = new FakeGitHub();
  const now = new Date('2026-08-11T01:00:00Z');
  let request;
  let clientOptions;
  const planEvent = {
    sender: { login: 'maintainer' },
    issue: { number: 1 },
    comment: { id: 34, body: '/agent plan', author_association: 'MEMBER' },
  };
  await runAutomation({
    kind: 'issue',
    eventName: 'issue_comment',
    event: planEvent,
    environment: environment({ AERIS_AI_MODEL: 'gpt-5.6-sol' }),
    repoRoot,
    contracts: enabledContracts('planner'),
    policySha,
    github,
    clock: () => now,
    aiClientFactory: (options) => {
      clientOptions = options;
      return {
        async complete(value) {
          request = value;
          return {
            content: JSON.stringify({
              schema_version: 1,
              agent: 'planner',
              summary: 'Implement the bounded change.',
              acceptance_criteria: ['The response is schema-valid.'],
              implementation_steps: ['Send the strict planner schema.'],
              validation_plan: ['Run the automation tests.'],
              risks: ['The provider capability requires a live canary.'],
              next_agent: null,
            }),
            model: { alias: 'default', id: 'gpt-5.6-sol' },
            durationMs: 12,
            usage: { total_tokens: 20 },
          };
        },
      };
    },
  });
  assert.equal(clientOptions.timeoutMs, 120_000);
  assert.equal(clientOptions.deadlineAtMs, Date.parse('2026-08-11T01:10:00Z'));
  assert.equal(request.responseFormat.type, 'json_schema');
  assert.equal(request.responseFormat.json_schema.name, 'aeris_planner_output');
  assert.equal(request.responseFormat.json_schema.strict, true);
  assert.equal(request.responseFormat.json_schema.schema.additionalProperties, false);
});

test('unapproved planner models fail before reservation or model construction', async () => {
  const github = new FakeGitHub();
  const planEvent = {
    sender: { login: 'maintainer' },
    issue: { number: 1 },
    comment: { id: 35, body: '/agent plan', author_association: 'MEMBER' },
  };
  await assert.rejects(
    () =>
      runPreflightPhase({
        kind: 'issue',
        eventName: 'issue_comment',
        event: planEvent,
        environment: environment({ AERIS_AI_MODEL: 'unverified-model' }),
        repoRoot,
        contracts: enabledContracts('planner'),
        policySha,
        github,
      }),
    (error) => error.code === 'structured_output_model_not_approved',
  );
  assert.equal(github.comments.length, 0);
});

test('generation change during model call prevents writeback', async () => {
  const github = new FakeGitHub();
  const ai = triageCompletion(() => {
    github.issue.body = 'The request changed while analysis was running.';
    github.touchIssue('2026-08-11T00:09:00Z');
  });
  const result = await runAutomation({
    kind: 'issue',
    eventName: 'issues',
    event: issueEvent,
    environment: environment(),
    repoRoot,
    contracts: enabledContracts('triage'),
    policySha,
    github,
    aiClientFactory: ai.factory,
  });
  assert.deepEqual(result, { state: 'stale', reason: 'input_fingerprint_changed' });
  assert.equal(github.comments.length, 1);
  assert.equal(decodeMetadata(github.comments[0].body).result, 'stale');
  assert.equal(decodeMetadata(github.comments[0].body).lease_expires_at, null);
});

test('disabled agent does not construct or call the AI client', async () => {
  const github = new FakeGitHub();
  let constructed = false;
  const disabledContracts = structuredClone(loadContracts(repoRoot));
  disabledContracts.agents.agents.triage.enabled = false;
  const result = await runAutomation({
    kind: 'issue',
    eventName: 'issues',
    event: issueEvent,
    environment: environment(),
    repoRoot,
    contracts: disabledContracts,
    policySha,
    github,
    aiClientFactory: () => {
      constructed = true;
      throw new Error('must not construct');
    },
  });
  assert.deepEqual(result, { state: 'disabled', reason: 'agent_disabled' });
  assert.equal(constructed, false);
  assert.equal(github.comments.length, 0);
});

test('invalid AI configuration fails before writing a reservation', async () => {
  for (const overrides of [
    { AERIS_AI_API_KEY: '', AERIS_AI_API_KEY_PRESENT: 'false' },
    { AERIS_AI_BASE_URL: 'http://ai.example.test/v1', AERIS_AI_API_KEY_PRESENT: 'true' },
    { AERIS_AI_MODEL: '', AERIS_AI_MODEL_TRIAGE: '', AERIS_AI_MODEL_FALLBACK: '', AERIS_AI_API_KEY_PRESENT: 'true' },
  ]) {
    const github = new FakeGitHub();
    await assert.rejects(
      runPreflightPhase({
        kind: 'issue', eventName: 'issues', event: issueEvent,
        environment: environment(overrides), repoRoot,
        contracts: enabledContracts('triage'), policySha, github,
      }),
    );
    assert.equal(github.comments.length, 0);
  }
});

test('status command preserves the prior analysis in the single managed comment', async () => {
  const github = new FakeGitHub();
  const ai = triageCompletion();
  await runAutomation({
    kind: 'issue',
    eventName: 'issues',
    event: issueEvent,
    environment: environment(),
    repoRoot,
    contracts: enabledContracts('triage'),
    policySha,
    github,
    aiClientFactory: ai.factory,
  });
  const previousBody = github.comments[0].body;
  const statusEvent = {
    sender: { login: 'outside-user' },
    issue: { number: 1 },
    comment: { id: 33, body: '/agent status', author_association: 'NONE' },
  };
  const result = await runAutomation({
    kind: 'issue',
    eventName: 'issue_comment',
    event: statusEvent,
    environment: environment(),
    repoRoot,
    contracts: loadContracts(repoRoot),
    policySha,
    github,
  });
  assert.equal(result.state, 'published');
  assert.equal(github.comments.length, 1);
  assert.equal(github.comments[0].body.includes('Aeris Agent 状态'), true);
  assert.equal(github.comments[0].body.includes('Confirmed request failure.'), true);
  assert.notEqual(github.comments[0].body, previousBody);
});

test('status during an active run preserves the reservation fence', async () => {
  const github = new FakeGitHub();
  const prior = {
    source_key: 'issues:active-delivery', agent: 'triage', input_sha: 'd'.repeat(64), policy_sha: policySha,
    object_generation: github.issue.updated_at, result: 'running', lease_token: 'e'.repeat(48),
    cancel_epoch: 2, lease_expires_at: '2026-08-11T01:15:00Z', recent_model_runs: [],
    processed_identities: [], reason_codes: ['issue_opened'],
  };
  runningComment(github, prior);
  const result = await runAutomation({
    kind: 'issue', eventName: 'issue_comment',
    event: {
      sender: { login: 'outside-user' }, issue: { number: 1 },
      comment: { id: 46, body: '/agent status', author_association: 'NONE' },
    },
    environment: environment(), repoRoot, contracts: loadContracts(repoRoot), policySha, github,
    clock: () => new Date('2026-08-11T01:00:00Z'),
  });
  assert.equal(result.state, 'published');
  assert.equal(managedMetadata(github).result, 'running');
  assert.equal(managedMetadata(github).lease_token, prior.lease_token);
  assert.equal(managedMetadata(github).lease_expires_at, prior.lease_expires_at);
  assert.equal(managedMetadata(github).cancel_epoch, prior.cancel_epoch);
  assert.equal(managedMetadata(github).source_key, prior.source_key);
});

test('cancel bypasses an active lease and fences a late analysis result', async () => {
  const github = new FakeGitHub();
  const now = new Date('2026-08-11T01:00:00Z');
  runningComment(github, {
    source_key: 'issues:active-delivery',
    agent: 'triage',
    input_sha: 'd'.repeat(64),
    policy_sha: policySha,
    object_generation: github.issue.updated_at,
    result: 'running',
    lease_token: 'e'.repeat(48),
    cancel_epoch: 2,
    lease_expires_at: '2026-08-11T01:15:00Z',
    recent_model_runs: [],
    processed_identities: [],
  });
  const cancelEvent = {
    sender: { login: 'maintainer' },
    issue: { number: 1 },
    comment: { id: 44, body: '/agent cancel', author_association: 'MEMBER' },
  };
  const result = await runAutomation({
    kind: 'issue',
    eventName: 'issue_comment',
    event: cancelEvent,
    environment: environment(),
    repoRoot,
    contracts: loadContracts(repoRoot),
    policySha,
    github,
    clock: () => now,
  });
  assert.equal(result.state, 'published');
  assert.equal(managedMetadata(github).result, 'cancelled');
  assert.equal(managedMetadata(github).lease_expires_at, null);
  assert.equal(managedMetadata(github).cancel_epoch, 3);
  assert.equal(managedMetadata(github).processed_identities.at(-1).result, 'cancelled');
});

test('cancel remains available after the kill switch is turned off', async () => {
  const github = new FakeGitHub();
  runningComment(github, {
    source_key: 'issues:active-delivery', agent: 'triage', input_sha: 'd'.repeat(64), policy_sha: policySha,
    object_generation: github.issue.updated_at, result: 'running', lease_token: 'e'.repeat(48),
    cancel_epoch: 0, lease_expires_at: '2026-08-11T01:15:00Z', recent_model_runs: [], processed_identities: [],
  });
  const result = await runAutomation({
    kind: 'issue', eventName: 'issue_comment',
    event: {
      sender: { login: 'maintainer' }, issue: { number: 1 },
      comment: { id: 45, body: '/agent cancel', author_association: 'MEMBER' },
    },
    environment: environment({ AERIS_AGENTS_ENABLED: 'false' }), repoRoot,
    contracts: loadContracts(repoRoot), policySha, github,
    clock: () => new Date('2026-08-11T01:00:00Z'),
  });
  assert.equal(result.state, 'published');
  assert.equal(managedMetadata(github).result, 'cancelled');
});

test('workflow_run reviewer reads patches and publishes advisory output', async () => {
  const github = new FakeGitHub();
  const now = new Date('2026-08-11T01:00:00Z');
  let calls = 0;
  let clientOptions;
  const result = await runAutomation({
    kind: 'pull',
    eventName: 'workflow_run',
    event: {
      action: 'completed',
      sender: { login: 'github-actions[bot]' },
      workflow_run: {
        conclusion: 'success',
        head_sha: 'c'.repeat(40),
        pull_requests: [{ number: 7 }],
      },
    },
    environment: environment(),
    repoRoot,
    contracts: enabledContracts('reviewer'),
    policySha,
    github,
    clock: () => now,
    aiClientFactory: (options) => {
      clientOptions = options;
      return {
        async complete({ messages }) {
          calls += 1;
          assert.equal(messages[1].content.includes('src/request.ts'), true);
          return {
            content: JSON.stringify({
              schema_version: 1,
              agent: 'reviewer',
              summary: 'No blocking issue found.',
              verdict: 'ready_for_human_review',
              findings: [],
              test_recommendations: ['Run the request regression test.'],
              next_agent: null,
            }),
            model: { alias: 'default', id: 'test-model' },
            durationMs: 10,
            usage: null,
          };
        },
      };
    },
  });
  assert.equal(result.state, 'published');
  assert.equal(calls, 1);
  assert.equal(clientOptions.connectTimeoutMs, 120_000);
  assert.equal(clientOptions.timeoutMs, 300_000);
  assert.equal(clientOptions.deadlineAtMs, Date.parse('2026-08-11T01:10:00Z'));
  assert.equal(github.comments[0].body.includes('ready_for_human_review'), true);
});

test('stale workflow_run head skips required-check queries', async () => {
  const github = new FakeGitHub();
  const result = await runAutomation({
    kind: 'pull',
    eventName: 'workflow_run',
    event: {
      action: 'completed',
      sender: { login: 'github-actions[bot]' },
      workflow_run: {
        conclusion: 'success',
        head_sha: 'd'.repeat(40),
        pull_requests: [{ number: 7 }],
      },
    },
    environment: environment(),
    repoRoot,
    contracts: enabledContracts('reviewer'),
    policySha,
    github,
  });
  assert.deepEqual(result, { state: 'skipped', reason: 'workflow_run_head_stale' });
  assert.equal(github.requiredCheckQueries, 0);
  assert.equal(github.comments.length, 0);
});

test('workflow_run fails closed when the PR head changes during preflight', async () => {
  const github = new FakeGitHub();
  github.pullHeadAfterFirstRead = 'd'.repeat(40);
  const result = await runAutomation({
    kind: 'pull',
    eventName: 'workflow_run',
    event: {
      action: 'completed',
      sender: { login: 'github-actions[bot]' },
      workflow_run: {
        conclusion: 'success',
        head_sha: 'c'.repeat(40),
        pull_requests: [{ number: 7 }],
      },
    },
    environment: environment(),
    repoRoot,
    contracts: enabledContracts('reviewer'),
    policySha,
    github,
  });
  assert.deepEqual(result, {
    state: 'skipped',
    reason: 'pull_request_head_changed_during_preflight',
  });
  assert.equal(github.requiredCheckQueries, 2);
  assert.equal(github.comments.length, 0);
});

test('a later workflow_run event can proceed after required checks transition to success', async () => {
  const github = new FakeGitHub();
  const event = {
    action: 'completed',
    sender: { login: 'github-actions[bot]' },
    workflow_run: {
      conclusion: 'success',
      head_sha: github.pull.head.sha,
      pull_requests: [{ number: 7 }],
    },
  };
  const common = {
    kind: 'pull', eventName: 'workflow_run', event, environment: environment(), repoRoot,
    contracts: enabledContracts('reviewer'), policySha, github,
  };

  github.checkRuns[1] = {
    ...github.checkRuns[1],
    status: 'in_progress',
    conclusion: null,
    completed_at: null,
    started_at: '2026-08-11T00:01:00Z',
  };
  const first = await runAutomation(common);
  assert.deepEqual(first, { state: 'skipped', reason: 'required_checks_not_successful' });
  assert.equal(github.comments.length, 0);

  github.checkRuns[1] = {
    ...github.checkRuns[1],
    status: 'completed',
    conclusion: 'success',
    completed_at: '2026-08-11T00:02:00Z',
  };
  let calls = 0;
  const second = await runAutomation({
    ...common,
    aiClientFactory: () => ({
      async complete() {
        calls += 1;
        return {
          content: JSON.stringify({
            schema_version: 1,
            agent: 'reviewer',
            summary: 'Checks are green.',
            verdict: 'ready_for_human_review',
            findings: [],
            test_recommendations: [],
            next_agent: null,
          }),
          model: { alias: 'default', id: 'test-model' },
          durationMs: 1,
          usage: null,
        };
      },
    }),
  });
  assert.equal(second.state, 'published');
  assert.equal(calls, 1);
  assert.equal(github.comments.length, 1);
});

test('required-check regression before analysis defers without consuming the delivery', async () => {
  const github = new FakeGitHub();
  const event = {
    action: 'completed',
    sender: { login: 'github-actions[bot]' },
    workflow_run: {
      conclusion: 'success',
      head_sha: github.pull.head.sha,
      pull_requests: [{ number: 7 }],
    },
  };
  const common = {
    kind: 'pull', eventName: 'workflow_run', event, environment: environment(), repoRoot,
    contracts: enabledContracts('reviewer'), policySha, github,
  };
  const preflight = await runPreflightPhase(common);
  const reservation = await runReservationPhase({ ...common, artifact: preflight });
  github.checkRuns.push({
    id: 3,
    name: 'Frontend CI / check',
    status: 'in_progress',
    conclusion: null,
    started_at: '2026-08-11T00:02:00Z',
  });

  let constructed = false;
  const analysis = await runAnalysisPhase({
    ...common,
    artifact: reservation,
    aiClientFactory: () => {
      constructed = true;
      throw new Error('must not construct');
    },
  });
  assert.equal(analysis.state, 'failed');
  assert.deepEqual(analysis.failure, { code: 'required_checks_not_successful' });
  assert.equal(constructed, false);

  const publication = await runPublishPhase({ ...common, artifact: analysis });
  assert.deepEqual(publication.result, {
    state: 'skipped',
    reason: 'required_checks_not_successful',
    comment_id: 1,
  });
  assert.equal(managedMetadata(github).result, 'deferred');
  assert.equal(managedMetadata(github).processed_identities.length, 0);

  github.checkRuns[2] = {
    ...github.checkRuns[2],
    status: 'completed',
    conclusion: 'success',
    completed_at: '2026-08-11T00:03:00Z',
  };
  let calls = 0;
  const retried = await runAutomation({
    ...common,
    aiClientFactory: () => ({
      async complete() {
        calls += 1;
        return {
          content: JSON.stringify({
            schema_version: 1,
            agent: 'reviewer',
            summary: 'Checks recovered.',
            verdict: 'ready_for_human_review',
            findings: [],
            test_recommendations: [],
            next_agent: null,
          }),
          model: { alias: 'default', id: 'test-model' },
          durationMs: 1,
          usage: null,
        };
      },
    }),
  });
  assert.equal(retried.state, 'published');
  assert.equal(calls, 1);
});

test('analysis rechecks cancellation after required checks before constructing the AI client', async () => {
  const github = new FakeGitHub();
  const event = {
    action: 'completed',
    sender: { login: 'github-actions[bot]' },
    workflow_run: {
      conclusion: 'success',
      head_sha: github.pull.head.sha,
      pull_requests: [{ number: 7 }],
    },
  };
  const common = {
    kind: 'pull', eventName: 'workflow_run', event, environment: environment(), repoRoot,
    contracts: enabledContracts('reviewer'), policySha, github,
  };
  const preflight = await runPreflightPhase(common);
  const reservation = await runReservationPhase({ ...common, artifact: preflight });
  const listCommitStatuses = github.listCommitStatuses.bind(github);
  github.listCommitStatuses = async (...args) => {
    const statuses = await listCommitStatuses(...args);
    const metadata = managedMetadata(github);
    runningComment(github, {
      ...metadata,
      result: 'cancelled',
      lease_token: null,
      lease_expires_at: null,
      cancel_epoch: metadata.cancel_epoch + 1,
    });
    return statuses;
  };

  let constructed = false;
  const analysis = await runAnalysisPhase({
    ...common,
    artifact: reservation,
    aiClientFactory: () => {
      constructed = true;
      throw new Error('must not construct');
    },
  });
  assert.equal(analysis.state, 'failed');
  assert.deepEqual(analysis.failure, { code: 'cancelled_before_analysis' });
  assert.equal(constructed, false);
});

test('analysis check deferrals release only their own reservation from the hourly limit', async () => {
  const github = new FakeGitHub();
  const event = {
    action: 'completed',
    sender: { login: 'github-actions[bot]' },
    workflow_run: {
      conclusion: 'success',
      head_sha: github.pull.head.sha,
      pull_requests: [{ number: 7 }],
    },
  };
  const retainedRun = {
    at: '2026-08-11T00:30:00.000Z',
    source_key: 'pull:7:previous-head',
    agent: 'reviewer',
    reservation_token: 'completed-reservation',
  };
  runningComment(github, {
    source_key: 'pull:7:previous-head', agent: 'reviewer', input_sha: 'd'.repeat(64), policy_sha: policySha,
    result: 'completed', lease_token: null, lease_expires_at: null, recent_model_runs: [retainedRun],
  });
  const common = {
    kind: 'pull', eventName: 'workflow_run', event, environment: environment(), repoRoot,
    contracts: enabledContracts('reviewer'), policySha, github, clock: () => new Date('2026-08-11T00:45:00Z'),
  };

  for (let index = 0; index < 4; index += 1) {
    const preflight = await runPreflightPhase(common);
    const reservation = await runReservationPhase({ ...common, artifact: preflight });
    assert.equal(reservation.state, 'reserved');
    github.checkRuns.push({
      id: 10 + index,
      name: 'Frontend CI / check',
      status: 'in_progress',
      conclusion: null,
      started_at: '2026-08-11T00:02:00Z',
    });
    const analysis = await runAnalysisPhase({ ...common, artifact: reservation });
    assert.deepEqual(analysis.failure, { code: 'required_checks_not_successful' });
    const publication = await runPublishPhase({ ...common, artifact: analysis });
    assert.equal(publication.state, 'skipped');
    assert.deepEqual(managedMetadata(github).recent_model_runs, [retainedRun]);
    github.checkRuns[github.checkRuns.length - 1] = {
      ...github.checkRuns[github.checkRuns.length - 1], status: 'completed', conclusion: 'success',
      completed_at: '2026-08-11T00:03:00Z',
    };
  }

  let calls = 0;
  const finalRun = await runAutomation({
    ...common,
    aiClientFactory: () => ({
      async complete() {
        calls += 1;
        return {
          content: JSON.stringify({
            schema_version: 1, agent: 'reviewer', summary: 'Checks recovered.',
            verdict: 'ready_for_human_review', findings: [], test_recommendations: [], next_agent: null,
          }),
          model: { alias: 'default', id: 'test-model' }, durationMs: 1, usage: null,
        };
      },
    }),
  });
  assert.equal(finalRun.state, 'published');
  assert.equal(calls, 1);
  assert.equal(managedMetadata(github).recent_model_runs.length, 2);
  assert.deepEqual(managedMetadata(github).recent_model_runs[0], retainedRun);
});

test('required-check regression after analysis blocks publication', async () => {
  const github = new FakeGitHub();
  const event = {
    action: 'completed',
    sender: { login: 'github-actions[bot]' },
    workflow_run: {
      conclusion: 'success',
      head_sha: github.pull.head.sha,
      pull_requests: [{ number: 7 }],
    },
  };
  const common = {
    kind: 'pull', eventName: 'workflow_run', event, environment: environment(), repoRoot,
    contracts: enabledContracts('reviewer'), policySha, github,
  };
  const preflight = await runPreflightPhase(common);
  const reservation = await runReservationPhase({ ...common, artifact: preflight });
  const analysis = await runAnalysisPhase({
    ...common,
    artifact: reservation,
    aiClientFactory: () => ({
      async complete() {
        return {
          content: JSON.stringify({
            schema_version: 1,
            agent: 'reviewer',
            summary: 'Checks were green during analysis.',
            verdict: 'ready_for_human_review',
            findings: [],
            test_recommendations: [],
            next_agent: null,
          }),
          model: { alias: 'default', id: 'test-model' },
          durationMs: 1,
          usage: null,
        };
      },
    }),
  });
  assert.equal(analysis.state, 'completed');

  github.checkRuns.push({
    id: 3,
    name: 'Rust CI / check',
    status: 'completed',
    conclusion: 'failure',
    started_at: '2026-08-11T00:02:00Z',
    completed_at: '2026-08-11T00:03:00Z',
  });
  const publication = await runPublishPhase({ ...common, artifact: analysis });
  assert.equal(publication.state, 'skipped');
  assert.equal(publication.result.reason, 'required_checks_not_successful');
  assert.equal(managedMetadata(github).result, 'deferred');
  assert.equal(github.comments[0].body.includes('ready_for_human_review'), false);
});

test('a running lease blocks a distinct delivery from starting another model run', async () => {
  const github = new FakeGitHub();
  const ai = triageCompletion();
  const now = new Date('2026-08-11T01:00:00Z');
  runningComment(github, {
    source_key: 'issues:earlier-delivery',
    agent: 'triage',
    input_sha: 'different-input',
    policy_sha: policySha,
    result: 'running',
    lease_expires_at: '2026-08-11T01:15:00Z',
    recent_model_runs: [],
  });
  const result = await runAutomation({
    kind: 'issue', eventName: 'issues', event: issueEvent, environment: environment(), repoRoot,
    contracts: enabledContracts('triage'), policySha, github, aiClientFactory: ai.factory, clock: () => now,
  });
  assert.deepEqual(result, { state: 'in_progress', reason: 'running_lease_active' });
  assert.equal(ai.calls(), 0);
  assert.equal(managedMetadata(github).result, 'running');
});

test('an expired running lease is recovered with a new reservation and completed result', async () => {
  const github = new FakeGitHub();
  const ai = triageCompletion();
  const now = new Date('2026-08-11T01:00:00Z');
  runningComment(github, {
    source_key: 'issues:previous-delivery', agent: 'triage', input_sha: 'old-input', policy_sha: policySha,
    result: 'running', lease_expires_at: '2026-08-11T00:59:59Z',
    recent_model_runs: [{ at: '2026-08-11T00:30:00Z', source_key: 'issues:previous-delivery', agent: 'triage' }],
  });
  const result = await runAutomation({
    kind: 'issue', eventName: 'issues', event: issueEvent, environment: environment(), repoRoot,
    contracts: enabledContracts('triage'), policySha, github, aiClientFactory: ai.factory, clock: () => now,
  });
  assert.equal(result.state, 'published');
  assert.equal(ai.calls(), 1);
  assert.equal(managedMetadata(github).result, 'completed');
  assert.equal(managedMetadata(github).recent_model_runs.length, 2);
});

test('model and schema failures replace the reservation with failed metadata', async () => {
  for (const [completion, expectedCode] of [
    [() => { throw Object.assign(new Error('connection lost'), { code: 'connect_error' }); }, 'connect_error'],
    [() => ({ content: '{not json}', model: { alias: 'default', id: 'test-model' }, durationMs: 1, usage: null }), 'invalid_model_output'],
    [() => { throw Object.assign(new Error('truncated'), { code: 'output_truncated' }); }, 'output_truncated'],
  ]) {
    const github = new FakeGitHub();
    const modelEvents = [];
    await assert.rejects(
      runAutomation({
        kind: 'issue', eventName: 'issues', event: issueEvent, environment: environment(), repoRoot,
        contracts: enabledContracts('triage'), policySha, github,
        aiClientFactory: () => ({ async complete() { return completion(); } }),
        auditEvent: (event) => modelEvents.push(event),
      }),
      (error) => error.code === expectedCode,
    );
    assert.equal(managedMetadata(github).result, 'failed');
    assert.equal(managedMetadata(github).reason_codes.at(-1), expectedCode);
    assert.equal(managedMetadata(github).lease_expires_at, null);
    assert.equal(modelEvents.length, 1);
    assert.equal(modelEvents[0].code, expectedCode);
    if (expectedCode === 'invalid_model_output') {
      assert.equal(modelEvents[0].diagnostic, 'json_syntax');
      assert.equal(modelEvents[0].completion_received, true);
      assert.equal(modelEvents[0].content_bytes, 10);
      assert.equal(modelEvents[0].model_alias, 'default');
      assert.equal(modelEvents[0].model_id, 'test-model');
      assert.equal(JSON.stringify(modelEvents[0]).includes('{not json}'), false);
    }
  }
});

test('model refusal is published as a code-only failure without leaking refusal text', async () => {
  const refusalText = 'private model refusal details';
  const github = new FakeGitHub();
  const modelEvents = [];
  const common = {
    kind: 'issue',
    eventName: 'issues',
    event: issueEvent,
    environment: environment(),
    repoRoot,
    contracts: enabledContracts('triage'),
    policySha,
    github,
  };
  const preflight = await runPreflightPhase(common);
  const reservation = await runReservationPhase({ ...common, artifact: preflight });
  const analysis = await runAnalysisPhase({
    ...common,
    artifact: reservation,
    aiClientFactory: () => ({
      async complete() {
        throw Object.assign(new Error(refusalText), {
          code: 'model_refusal',
          retryable: false,
        });
      },
    }),
    auditEvent: (event) => modelEvents.push(event),
  });

  assert.equal(analysis.state, 'failed');
  assert.equal(analysis.output, null);
  assert.equal(analysis.model, null);
  assert.deepEqual(analysis.failure, { code: 'model_refusal' });
  assert.equal(JSON.stringify(analysis).includes(refusalText), false);
  assert.equal(modelEvents.length, 1);
  assert.equal(modelEvents[0].code, 'model_refusal');
  assert.equal(modelEvents[0].completion_received, false);
  assert.equal(JSON.stringify(modelEvents[0]).includes(refusalText), false);

  const publication = await runPublishPhase({ ...common, artifact: analysis });
  assert.equal(publication.state, 'published');
  assert.equal(publication.result.reason, 'model_refusal');
  assert.equal(managedMetadata(github).result, 'failed');
  assert.equal(managedMetadata(github).reason_codes.at(-1), 'model_refusal');
  assert.equal(managedMetadata(github).lease_expires_at, null);
  assert.equal(github.comments[0].body.includes(refusalText), false);
  assert.equal(JSON.stringify(publication).includes(refusalText), false);
});

test('sensitive model output is rejected before it reaches an artifact or comment', async () => {
  const secret = 'test-secret-value-1234567890';
  for (const leakedSummary of [
    `The upstream repeated ${secret}`,
    'Authorization: Bearer attacker-visible-token',
    'Cookie=session-value-12345678',
  ]) {
    const github = new FakeGitHub();
    const common = {
      environment: environment({ AERIS_AI_API_KEY: secret, AERIS_AI_API_KEY_PRESENT: 'true' }),
      repoRoot,
      contracts: enabledContracts('triage'),
      policySha,
      github,
    };
    const preflight = await runPreflightPhase({
      ...common, kind: 'issue', eventName: 'issues', event: issueEvent,
    });
    const reservation = await runReservationPhase({ ...common, artifact: preflight });
    const analysis = await runAnalysisPhase({
      ...common,
      artifact: reservation,
      aiClientFactory: () => ({ async complete() {
        return {
          content: JSON.stringify({
            schema_version: 1,
            agent: 'triage',
            summary: leakedSummary,
            risk: 'high',
            proposed_labels: [],
            missing_information: [],
            recommended_action: 'Escalate safely.',
            next_agent: null,
          }),
          model: { alias: 'default', id: 'test-model' },
          durationMs: 1,
          usage: null,
        };
      } }),
    });
    assert.equal(analysis.state, 'failed');
    assert.deepEqual(analysis.failure, { code: 'sensitive_model_output' });
    assert.equal(analysis.output, null);
    assert.equal(JSON.stringify(analysis).includes(secret), false);

    const publication = await runPublishPhase({ ...common, artifact: analysis });
    assert.equal(publication.state, 'published');
    assert.equal(managedMetadata(github).result, 'failed');
    assert.equal(github.comments[0].body.includes(leakedSummary), false);
    assert.equal(github.comments[0].body.includes(secret), false);
  }
});

test('the per-object hourly limit counts recent model reservations', async () => {
  const github = new FakeGitHub();
  const ai = triageCompletion();
  const now = new Date('2026-08-11T01:00:00Z');
  runningComment(github, {
    source_key: 'issues:previous-delivery', agent: 'triage', input_sha: 'old-input', policy_sha: policySha,
    result: 'failed', lease_expires_at: null,
    recent_model_runs: Array.from({ length: 4 }, (_, index) => ({
      at: `2026-08-11T00:${10 + index}:00Z`, source_key: `issues:${index}`, agent: 'triage',
    })),
  });
  const result = await runAutomation({
    kind: 'issue', eventName: 'issues', event: issueEvent, environment: environment(), repoRoot,
    contracts: enabledContracts('triage'), policySha, github, aiClientFactory: ai.factory, clock: () => now,
  });
  assert.deepEqual(result, { state: 'rate_limited', reason: 'object_hourly_limit' });
  assert.equal(ai.calls(), 0);
});

test('the hourly limit includes a reservation exactly at the one-hour boundary', async () => {
  const github = new FakeGitHub();
  const ai = triageCompletion();
  const now = new Date('2026-08-11T01:00:00Z');
  runningComment(github, {
    source_key: 'issues:previous-delivery', agent: 'triage', input_sha: 'old-input', policy_sha: policySha,
    result: 'failed', lease_expires_at: null,
    recent_model_runs: [
      { at: '2026-08-11T00:00:00.000Z', source_key: 'issues:0', agent: 'triage' },
      { at: '2026-08-11T00:10:00.000Z', source_key: 'issues:1', agent: 'triage' },
      { at: '2026-08-11T00:20:00.000Z', source_key: 'issues:2', agent: 'triage' },
      { at: '2026-08-11T00:30:00.000Z', source_key: 'issues:3', agent: 'triage' },
    ],
  });
  const result = await runAutomation({
    kind: 'issue', eventName: 'issues', event: issueEvent, environment: environment(), repoRoot,
    contracts: enabledContracts('triage'), policySha, github, aiClientFactory: ai.factory, clock: () => now,
  });
  assert.deepEqual(result, { state: 'rate_limited', reason: 'object_hourly_limit' });
  assert.equal(ai.calls(), 0);
});

test('the four phases exchange fingerprint-only artifacts and wire the connect timeout', async () => {
  const github = new FakeGitHub();
  const now = new Date('2026-08-11T01:00:00Z');
  const contracts = enabledContracts('triage');
  const common = {
    environment: environment({ AERIS_AI_API_KEY_PRESENT: 'true' }),
    repoRoot,
    contracts,
    policySha,
    github,
    clock: () => now,
  };
  const preflight = await runPreflightPhase({
    ...common,
    kind: 'issue',
    eventName: 'issues',
    event: issueEvent,
  });
  assert.equal(preflight.state, 'ready');
  assert.equal(preflight.input, null);
  const reservation = await runReservationPhase({ ...common, artifact: preflight });
  assert.equal(reservation.state, 'reserved');
  assert.equal(typeof reservation.reservation.lease_token, 'string');
  let clientOptions;
  const analysis = await runAnalysisPhase({
    ...common,
    artifact: reservation,
    aiClientFactory: (options) => {
      clientOptions = options;
      return triageCompletion().factory();
    },
  });
  assert.equal(analysis.state, 'completed');
  assert.equal(clientOptions.connectTimeoutMs, 120_000);
  assert.equal(clientOptions.timeoutMs, 120_000);
  assert.equal(clientOptions.deadlineAtMs, Date.parse('2026-08-11T01:10:00Z'));
  assert.equal(analysis.reservation.preflight.input, null);
  const publication = await runPublishPhase({ ...common, artifact: analysis });
  assert.equal(publication.state, 'published');
});

test('analysis does not call the model after cancellation wins the lease fence', async () => {
  const github = new FakeGitHub();
  const contracts = enabledContracts('triage');
  const common = {
    environment: environment({ AERIS_AI_API_KEY_PRESENT: 'true' }), repoRoot,
    contracts, policySha, github,
  };
  const preflight = await runPreflightPhase({ ...common, kind: 'issue', eventName: 'issues', event: issueEvent });
  const reservation = await runReservationPhase({ ...common, artifact: preflight });
  const metadata = managedMetadata(github);
  metadata.result = 'cancelled';
  metadata.cancel_epoch += 1;
  metadata.lease_expires_at = null;
  runningComment(github, metadata);
  let constructed = false;
  const analysis = await runAnalysisPhase({
    ...common,
    artifact: reservation,
    aiClientFactory: () => {
      constructed = true;
      throw new Error('must not construct');
    },
  });
  assert.equal(analysis.state, 'failed');
  assert.equal(analysis.failure.code, 'cancelled_before_analysis');
  assert.equal(constructed, false);
});

test('analysis does not construct the AI client for an expired reservation', async () => {
  const github = new FakeGitHub();
  const contracts = enabledContracts('triage');
  const reservationTime = new Date('2026-08-11T01:00:00Z');
  const common = {
    environment: environment({ AERIS_AI_API_KEY_PRESENT: 'true' }), repoRoot,
    contracts, policySha, github,
  };
  const preflight = await runPreflightPhase({ ...common, kind: 'issue', eventName: 'issues', event: issueEvent });
  const reservation = await runReservationPhase({ ...common, artifact: preflight, clock: () => reservationTime });
  let constructed = false;
  const analysis = await runAnalysisPhase({
    ...common,
    artifact: reservation,
    clock: () => new Date('2026-08-11T01:15:00.001Z'),
    aiClientFactory: () => {
      constructed = true;
      throw new Error('must not construct');
    },
  });
  assert.equal(analysis.state, 'failed');
  assert.equal(analysis.failure.code, 'lease_expired');
  assert.equal(constructed, false);
});

test('analysis derives an absolute deadline from the remaining lease with publish headroom', async () => {
  const github = new FakeGitHub();
  const contracts = enabledContracts('triage');
  const reservationTime = new Date('2026-08-11T01:00:00Z');
  const common = {
    environment: environment(), repoRoot, contracts, policySha, github,
  };
  const preflight = await runPreflightPhase({
    ...common, kind: 'issue', eventName: 'issues', event: issueEvent,
  });
  const reservation = await runReservationPhase({
    ...common, artifact: preflight, clock: () => reservationTime,
  });
  let clientOptions;
  const analysis = await runAnalysisPhase({
    ...common,
    artifact: reservation,
    clock: () => new Date('2026-08-11T01:11:59Z'),
    aiClientFactory: (options) => {
      clientOptions = options;
      return triageCompletion().factory();
    },
  });
  assert.equal(analysis.state, 'completed');
  assert.equal(clientOptions.deadlineAtMs, Date.parse('2026-08-11T01:12:00Z'));
});

test('analysis fails before model construction when only publish headroom remains', async () => {
  const github = new FakeGitHub();
  const contracts = enabledContracts('triage');
  const reservationTime = new Date('2026-08-11T01:00:00Z');
  const common = {
    environment: environment(), repoRoot, contracts, policySha, github,
  };
  const preflight = await runPreflightPhase({
    ...common, kind: 'issue', eventName: 'issues', event: issueEvent,
  });
  const reservation = await runReservationPhase({
    ...common, artifact: preflight, clock: () => reservationTime,
  });
  let constructed = false;
  const analysis = await runAnalysisPhase({
    ...common,
    artifact: reservation,
    clock: () => new Date('2026-08-11T01:12:00Z'),
    aiClientFactory: () => {
      constructed = true;
      throw new Error('must not construct');
    },
  });
  assert.equal(analysis.state, 'failed');
  assert.equal(analysis.failure.code, 'lease_expiring');
  assert.equal(constructed, false);
});

test('publish rejects a completed analysis after cancellation wins the lease fence', async () => {
  const github = new FakeGitHub();
  const common = {
    environment: environment(), repoRoot, contracts: enabledContracts('triage'), policySha, github,
  };
  const preflight = await runPreflightPhase({ ...common, kind: 'issue', eventName: 'issues', event: issueEvent });
  const reservation = await runReservationPhase({ ...common, artifact: preflight });
  const analysis = await runAnalysisPhase({
    ...common,
    artifact: reservation,
    aiClientFactory: triageCompletion().factory,
  });
  assert.equal(analysis.state, 'completed');

  const beforeCancel = managedMetadata(github);
  const cancelled = {
    ...beforeCancel,
    result: 'cancelled',
    lease_token: null,
    lease_expires_at: null,
    cancel_epoch: beforeCancel.cancel_epoch + 1,
  };
  runningComment(github, cancelled);
  const bodyBeforePublish = github.comments[0].body;

  const publication = await runPublishPhase({ ...common, artifact: analysis });
  assert.equal(publication.state, 'cancelled');
  assert.equal(publication.result.reason, 'cancelled_after_reservation');
  assert.equal(github.comments[0].body, bodyBeforePublish);
});

test('publish rejects a completed analysis after its lease expires', async () => {
  const github = new FakeGitHub();
  const reservationTime = new Date('2026-08-11T01:00:00Z');
  const common = {
    environment: environment(), repoRoot, contracts: enabledContracts('triage'), policySha, github,
  };
  const preflight = await runPreflightPhase({ ...common, kind: 'issue', eventName: 'issues', event: issueEvent });
  const reservation = await runReservationPhase({ ...common, artifact: preflight, clock: () => reservationTime });
  const analysis = await runAnalysisPhase({
    ...common,
    artifact: reservation,
    clock: () => reservationTime,
    aiClientFactory: triageCompletion().factory,
  });
  assert.equal(analysis.state, 'completed');
  const bodyBeforePublish = github.comments[0].body;

  const publication = await runPublishPhase({
    ...common,
    artifact: analysis,
    clock: () => new Date('2026-08-11T01:15:00.001Z'),
  });
  assert.equal(publication.state, 'stale');
  assert.equal(publication.result.reason, 'lease_fence_changed');
  assert.equal(github.comments[0].body, bodyBeforePublish);
});

test('managed comments trim the oldest replay identities to fit the configured limit', () => {
  const processedIdentities = fullReplayLedger();
  const metadata = {
    schema_version: 1,
    source_key: 'comment:100000000000000031',
    object_id: 'pull:123456',
    object_generation: 'c'.repeat(64),
    input_sha: 'f'.repeat(64),
    policy_sha: policySha,
    agent: 'reviewer',
    reason_codes: Array.from({ length: 40 }, (_, index) => `reason_${index}`),
    run_id: '1234567890123456789',
    recent_model_runs: [],
    processed_identities: processedIdentities,
    cancel_epoch: 0,
    result: 'completed',
    next_agent: null,
    lease_token: null,
    lease_expires_at: null,
    cancelled_at: null,
    model_alias: 'default',
    model_id: 'test-model',
  };
  const output = {
    schema_version: 1,
    agent: 'reviewer',
    summary: 'Reviewed.',
    verdict: 'ready_for_human_review',
    findings: [],
    test_recommendations: [],
    next_agent: null,
  };

  for (const body of [
    renderStatusComment('completed', metadata, 8000),
    renderAnalysisComment('reviewer', output, metadata, 8000),
  ]) {
    assert.ok(body.length <= 8000);
    const fitted = decodeMetadata(body);
    assert.ok(fitted.processed_identities.length > 0);
    assert.ok(fitted.processed_identities.length < processedIdentities.length);
    assert.equal(fitted.processed_identities.at(-1).source_key, processedIdentities.at(-1).source_key);
    assert.ok(fitted.reason_codes.length <= 16);
    assert.equal(fitted.reason_codes.at(-1), 'reason_39');
  }
});

test('managed comments compact the largest valid agent outputs to fit the configured limit', () => {
  const metadata = {
    schema_version: 1,
    source_key: 'comment:100000000000000031',
    object_id: 'pull:123456',
    object_generation: 'c'.repeat(64),
    input_sha: 'f'.repeat(64),
    policy_sha: policySha,
    agent: 'reviewer',
    reason_codes: ['pr_ci_completed'],
    run_id: '1234567890123456789',
    recent_model_runs: [],
    processed_identities: fullReplayLedger(),
    cancel_epoch: 0,
    result: 'completed',
    next_agent: null,
    lease_token: null,
    lease_expires_at: null,
    cancelled_at: null,
    model_alias: 'default',
    model_id: 'test-model',
  };
  const repeated = (value, length) => Array.from({ length }, () => value);
  const cases = [
    ['triage', {
      schema_version: 1,
      agent: 'triage',
      summary: '@'.repeat(1200),
      risk: 'high',
      proposed_labels: [],
      missing_information: repeated('@'.repeat(500), 12),
      recommended_action: '@'.repeat(1200),
      next_agent: null,
    }, [], /high/],
    ['planner', {
      schema_version: 1,
      agent: 'planner',
      summary: '@'.repeat(1200),
      acceptance_criteria: repeated('@'.repeat(500), 12),
      implementation_steps: repeated('@'.repeat(500), 12),
      validation_plan: repeated('@'.repeat(500), 12),
      risks: repeated('@'.repeat(500), 12),
      next_agent: null,
    }, [], /紧凑摘要/],
    ['reviewer', {
      schema_version: 1,
      agent: 'reviewer',
      summary: '@'.repeat(1200),
      verdict: 'changes_requested',
      findings: Array.from({ length: 20 }, (_, index) => ({
        severity: 'high',
        title: '@'.repeat(200),
        details: '@'.repeat(1000),
        path: '@'.repeat(300),
        line: index + 1,
      })),
      test_recommendations: repeated('@'.repeat(500), 12),
      next_agent: null,
    }, [], /changes_requested/],
  ];

  for (const [agent, rawOutput, repositoryLabels, expected] of cases) {
    const output = validateAgentOutput(agent, rawOutput, repositoryLabels);
    const body = renderAnalysisComment(agent, output, { ...metadata, agent }, 8000);
    assert.ok(body.length <= 8000, agent);
    assert.match(body, /摘要（截断）/, agent);
    assert.match(body, expected, agent);
    assert.equal(
      decodeMetadata(body).processed_identities.at(-1).source_key,
      'comment:100000000000000031',
      agent,
    );
  }
});

test('reservation rejects an issue whose input changes after preflight', async () => {
  const github = new FakeGitHub();
  const common = {
    environment: environment(), repoRoot, contracts: enabledContracts('triage'), policySha, github,
  };
  const preflight = await runPreflightPhase({ ...common, kind: 'issue', eventName: 'issues', event: issueEvent });
  github.issue.body = 'The request changed after preflight.';
  const reservation = await runReservationPhase({ ...common, artifact: preflight });
  assert.equal(reservation.state, 'terminal');
  assert.deepEqual(reservation.result, { state: 'stale', reason: 'input_fingerprint_changed', comment_id: null });
  assert.equal(github.comments.length, 0);
});

test('default lease tokens satisfy the reservation artifact contract', async () => {
  const github = new FakeGitHub();
  const common = {
    environment: environment(), repoRoot, contracts: enabledContracts('triage'), policySha, github,
  };
  const preflight = await runPreflightPhase({ ...common, kind: 'issue', eventName: 'issues', event: issueEvent });
  const reservation = await runReservationPhase({ ...common, artifact: preflight });
  assert.equal(reservation.state, 'reserved');
  const validated = validateReservationArtifact(reservation);
  assert.equal(validated.reservation.lease_token.length >= 43, true);
});

test('publish rejects each mutable pull input component changed after analysis', async () => {
  const prEvent = {
    action: 'completed',
    sender: { login: 'github-actions[bot]' },
    workflow_run: { conclusion: 'success', head_sha: 'c'.repeat(40), pull_requests: [{ number: 7 }] },
  };
  const changes = [
    ['title', (github) => { github.pull.title = 'Changed title'; }],
    ['body', (github) => { github.pull.body = 'Changed body'; }],
    ['labels', (github) => { github.pull.labels = [{ name: 'agent-ready' }]; }],
    ['files', (github) => { github.pullFiles.files[0].filename = 'src/changed.ts'; }],
    ['patch', (github) => { github.pullFiles.files[0].patch = '@@ -1 +1 @@\n-old\n+replacement'; }],
  ];
  for (const [name, change] of changes) {
    const github = new FakeGitHub();
    const common = {
      environment: environment(), repoRoot, contracts: enabledContracts('reviewer'), policySha, github,
    };
    const preflight = await runPreflightPhase({ ...common, kind: 'pull', eventName: 'workflow_run', event: prEvent });
    const reservation = await runReservationPhase({ ...common, artifact: preflight });
    const analysis = await runAnalysisPhase({
      ...common,
      artifact: reservation,
      aiClientFactory: () => ({ async complete() {
        return {
          content: JSON.stringify({ schema_version: 1, agent: 'reviewer', summary: 'Reviewed.', verdict: 'ready_for_human_review', findings: [], test_recommendations: [], next_agent: null }),
          model: { alias: 'default', id: 'test-model' }, durationMs: 1, usage: null,
        };
      } }),
    });
    assert.equal(analysis.state, 'completed', name);
    change(github);
    const publication = await runPublishPhase({ ...common, artifact: analysis });
    assert.equal(publication.state, 'stale', name);
    assert.equal(publication.result.reason, 'input_fingerprint_changed', name);
    assert.equal(managedMetadata(github).result, 'stale', name);
  }
});

test('publish fences an analysis when the trusted policy SHA changes', async () => {
  const github = new FakeGitHub();
  const common = {
    environment: environment(), repoRoot, contracts: enabledContracts('triage'), policySha, github,
  };
  const preflight = await runPreflightPhase({ ...common, kind: 'issue', eventName: 'issues', event: issueEvent });
  const reservation = await runReservationPhase({ ...common, artifact: preflight });
  const analysis = await runAnalysisPhase({ ...common, artifact: reservation, aiClientFactory: triageCompletion().factory });
  const publication = await runPublishPhase({ ...common, policySha: 'b'.repeat(40), artifact: analysis });
  assert.equal(publication.state, 'stale');
  assert.equal(publication.result.reason, 'policy_sha_changed');
  assert.equal(managedMetadata(github).result, 'stale');
  assert.equal(managedMetadata(github).processed_identities.at(-1).result, 'stale');
});

test('analysis and publish refuse a reservation whose lease token was replaced', async () => {
  const github = new FakeGitHub();
  const common = {
    environment: environment(), repoRoot, contracts: enabledContracts('triage'), policySha, github,
  };
  const preflight = await runPreflightPhase({ ...common, kind: 'issue', eventName: 'issues', event: issueEvent });
  const reservation = await runReservationPhase({ ...common, artifact: preflight });
  const completedAnalysis = await runAnalysisPhase({
    ...common,
    artifact: reservation,
    aiClientFactory: triageCompletion().factory,
  });
  assert.equal(completedAnalysis.state, 'completed');
  const metadata = managedMetadata(github);
  metadata.lease_token = 'z'.repeat(48);
  runningComment(github, metadata);
  const bodyBeforePublish = github.comments[0].body;
  let constructed = false;
  const analysis = await runAnalysisPhase({
    ...common,
    artifact: reservation,
    aiClientFactory: () => { constructed = true; throw new Error('must not construct'); },
  });
  assert.equal(analysis.state, 'failed');
  assert.equal(analysis.failure.code, 'lease_fence_changed');
  assert.equal(constructed, false);
  const publication = await runPublishPhase({ ...common, artifact: completedAnalysis });
  assert.equal(publication.state, 'stale');
  assert.equal(publication.result.reason, 'lease_fence_changed');
  assert.equal(github.comments[0].body, bodyBeforePublish);
});

test('reservation treats a processed ledger identity as a replay even after current metadata changes', async () => {
  const github = new FakeGitHub();
  const common = {
    environment: environment(), repoRoot, contracts: enabledContracts('triage'), policySha, github,
  };
  const preflight = await runPreflightPhase({ ...common, kind: 'issue', eventName: 'issues', event: issueEvent });
  const context = preflight.context;
  runningComment(github, {
    source_key: 'issues:newer-delivery', agent: 'triage', input_sha: 'f'.repeat(64), policy_sha: policySha,
    result: 'failed', lease_token: null, lease_expires_at: null, cancel_epoch: 0, recent_model_runs: [],
    processed_identities: [{
      source_key: context.source_key, agent: context.agent, input_sha: context.input_sha, policy_sha: context.policy_sha,
      result: 'completed', at: '2026-08-11T00:00:00Z',
    }],
  });
  const reservation = await runReservationPhase({ ...common, artifact: preflight });
  assert.equal(reservation.state, 'terminal');
  assert.deepEqual(reservation.result, { state: 'noop', reason: 'event_replayed', comment_id: null });
  assert.equal(managedMetadata(github).source_key, 'issues:newer-delivery');
});
