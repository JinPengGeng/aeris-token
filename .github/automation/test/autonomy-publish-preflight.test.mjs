import assert from 'node:assert/strict';
import test from 'node:test';

import { AutonomyPublishPreflightError, evaluatePublishPreflight } from '../src/autonomy-publish-preflight.mjs';

const REPOSITORY = 'JinPengGeng/aeris-token';
const REPOSITORY_ID = 1310462380;
const RUN_ID = 12345;
const SHA = 'a'.repeat(40);

class FakeClient {
  constructor(overrides = {}) {
    this.overrides = overrides;
  }

  async getWorkflowRun() {
    return {
      repository: { full_name: REPOSITORY, id: REPOSITORY_ID },
      head_repository: { full_name: REPOSITORY, id: REPOSITORY_ID },
      id: RUN_ID, run_attempt: 2, event: 'workflow_dispatch', status: 'completed', conclusion: 'success',
      name: 'Agent candidate', path: '.github/workflows/agent-candidate.yml', head_branch: 'main', head_sha: SHA,
      actor: { login: 'maintainer' }, ...this.overrides.run,
    };
  }

  async getRunArtifacts() {
    return this.overrides.artifacts ?? {
      total_count: 1,
      artifacts: [{ id: 9, name: `agent-candidate-issue-17-run-${RUN_ID}-2`, expired: false, size_in_bytes: 1024 }],
    };
  }

  async request(method, endpoint) {
    if (method !== 'GET') throw new Error('unexpected mutation');
    if (endpoint === `/repos/${REPOSITORY}`) return { id: REPOSITORY_ID, full_name: REPOSITORY, default_branch: 'main' };
    if (endpoint === `/repos/${REPOSITORY}/git/ref/heads/main`) return { object: { sha: SHA } };
    throw new Error(`unexpected endpoint ${endpoint}`);
  }

  async getIssue() {
    return { number: 17, state: 'open', updated_at: '2026-08-20T00:00:00Z', labels: [{ name: 'agent-ready' }] };
  }

  async getCollaboratorPermission() {
    return 'write';
  }
}

const input = Object.freeze({ repository: REPOSITORY, repository_id: REPOSITORY_ID, run_id: RUN_ID, run_attempt: 2 });

test('binds one successful default-branch candidate run and artifact to a current ready Issue', async () => {
  const result = await evaluatePublishPreflight(input, new FakeClient());
  assert.equal(result.issue_number, 17);
  assert.equal(result.base_sha, SHA);
  assert.equal(result.trigger_run_id, String(RUN_ID));
  assert.equal(result.artifact_name, `agent-candidate-issue-17-run-${RUN_ID}-2`);
});

test('rejects stale, non-default, failed, or repository-mismatched runs', async () => {
  for (const run of [
    { conclusion: 'failure' },
    { head_branch: 'feature/untrusted' },
    { path: '.github/workflows/other.yml' },
    { head_repository: { full_name: 'outside/fork', id: 1 } },
    { head_sha: 'b'.repeat(40) },
  ]) {
    await assert.rejects(
      () => evaluatePublishPreflight(input, new FakeClient({ run })),
      (error) => error instanceof AutonomyPublishPreflightError || /stale/.test(error.message),
    );
  }
});

test('rejects extra, expired, oversized, or ambiguously named artifacts', async () => {
  const invalid = [
    { total_count: 2, artifacts: [] },
    { total_count: 1, artifacts: [{ id: 9, name: 'wrong', expired: false, size_in_bytes: 1 }] },
    { total_count: 1, artifacts: [{ id: 9, name: `agent-candidate-issue-17-run-${RUN_ID}-2`, expired: true, size_in_bytes: 1 }] },
    { total_count: 1, artifacts: [{ id: 9, name: `agent-candidate-issue-17-run-${RUN_ID}-2`, expired: false, size_in_bytes: 3 * 1024 * 1024 }] },
  ];
  for (const artifacts of invalid) {
    await assert.rejects(() => evaluatePublishPreflight(input, new FakeClient({ artifacts })), AutonomyPublishPreflightError);
  }
});
