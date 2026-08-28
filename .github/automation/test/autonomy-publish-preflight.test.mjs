import assert from 'node:assert/strict';
import test from 'node:test';

import { AutonomyPublishPreflightError, evaluatePublishPreflight } from '../src/autonomy-publish-preflight.mjs';

const REPOSITORY = 'JinPengGeng/aeris-token';
const REPOSITORY_ID = 1310462380;
const RUN_ID = 12345;
const SHA = 'a'.repeat(40);

function candidateArtifact(overrides = {}) {
  return {
    id: 9,
    name: `agent-candidate-issue-17-run-${RUN_ID}-2`,
    expired: false,
    size_in_bytes: 1024,
    ...overrides,
  };
}

function runtimeArtifact(overrides = {}) {
  return {
    id: 8,
    name: `agent-candidate-runtime-${RUN_ID}-2`,
    expired: false,
    size_in_bytes: 8192,
    ...overrides,
  };
}

function artifactPage(artifacts, totalCount = artifacts.length) {
  return { total_count: totalCount, artifacts };
}

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
      total_count: 2,
      artifacts: [runtimeArtifact(), candidateArtifact()],
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

test('binds one successful default-branch candidate run and its exact artifact pair to a current ready Issue', async () => {
  for (const artifacts of [
    artifactPage([candidateArtifact(), runtimeArtifact()]),
    artifactPage([runtimeArtifact(), candidateArtifact()]),
  ]) {
    const result = await evaluatePublishPreflight(input, new FakeClient({ artifacts }));
    assert.equal(result.issue_number, 17);
    assert.equal(result.base_sha, SHA);
    assert.equal(result.trigger_run_id, String(RUN_ID));
    assert.equal(result.artifact_name, `agent-candidate-issue-17-run-${RUN_ID}-2`);
  }
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

test('rejects missing, extra, duplicate, stale, or ambiguously named artifact pairs', async () => {
  const invalid = [
    artifactPage([], 2),
    artifactPage([runtimeArtifact(), candidateArtifact()], 3),
    artifactPage([candidateArtifact()], 1),
    artifactPage([runtimeArtifact(), candidateArtifact({ name: 'wrong' })]),
    artifactPage([candidateArtifact(), candidateArtifact({ id: 10, name: `agent-candidate-issue-18-run-${RUN_ID}-2` })]),
    artifactPage([runtimeArtifact(), runtimeArtifact({ id: 10 })]),
    artifactPage([runtimeArtifact({ name: `agent-candidate-runtime-${RUN_ID}-3` }), candidateArtifact()]),
    artifactPage([runtimeArtifact(), candidateArtifact({ expired: true })]),
    artifactPage([runtimeArtifact(), candidateArtifact({ size_in_bytes: 3 * 1024 * 1024 })]),
    artifactPage([runtimeArtifact({ expired: true }), candidateArtifact()]),
    artifactPage([runtimeArtifact({ size_in_bytes: 2 * 1024 * 1024 }), candidateArtifact()]),
    artifactPage([runtimeArtifact({ id: 9 }), candidateArtifact()]),
  ];
  for (const artifacts of invalid) {
    await assert.rejects(() => evaluatePublishPreflight(input, new FakeClient({ artifacts })), AutonomyPublishPreflightError);
  }
});
