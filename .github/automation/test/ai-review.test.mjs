import assert from 'node:assert/strict';
import test from 'node:test';

import { analyzeAiReview, buildReviewRequest, containsSensitiveReviewOutput, finalizeAiReview, prepareAiReview } from '../src/ai-review.mjs';
import { candidateSha } from '../src/ai-review-phase-contract.mjs';

const sha = (character) => character.repeat(40);
const repository = 'JinPengGeng/aeris-token';
const models = { reviewer: [{ alias: 'reviewer', id: 'reviewer-model' }], security: [{ alias: 'security', id: 'security-model' }] };
const lifecycle = { kind: 'initial', pull_node_id: 'PR_kwDOAerisToken37', pull_created_at: '2026-08-18T00:00:00Z', reopened_event_id: null, reopened_at: null };
const exactDiff = { files: [{ path: 'README.md', status: 'M', old_blob_sha: sha('b'), new_blob_sha: sha('a') }], patch: '@@ -1 +1 @@\n-old\n+new\n', evidence: { base_sha: sha('b'), head_sha: sha('a'), manifest_sha: 'd'.repeat(64), raw_diff_sha: 'e'.repeat(64), file_count: 1, patch_bytes: 24 } };

function client(overrides = {}) {
  return {
    getRepository: async () => ({ id: 123, full_name: repository, default_branch: 'main' }),
    getBranchHead: async () => sha('c'),
    getPull: async () => ({ number: 37, state: 'open', draft: false, title: 'Change', body: 'Body', changed_files: 1, author: { login: 'contributor', id: 456, type: 'User' }, head: { sha: sha('a'), ref: 'feature', repo: { id: 123, full_name: repository } }, base: { sha: sha('b'), ref: 'main', repo: { id: 123, full_name: repository } } }),
    listPullFiles: async () => ({ truncated: false, files: [{ filename: 'README.md', sha: sha('a'), previous_filename: null, status: 'modified', additions: 1, deletions: 0, changes: 1, patch: '@@ -1 +1 @@\n-old\n+new' }] }),
    getPullLifecycle: async () => ({ head_sha: sha('a'), base_sha: sha('b'), lifecycle_epoch: lifecycle }),
    ...overrides,
  };
}
async function candidate(api = client()) { return prepareAiReview({ client: api, exactDiff, repository, repositoryId: 123, pullNumber: 37, policySha: sha('c'), modelCandidates: models }); }
function completion(role, findings = []) {
  return {
    requested_model: { alias: role, id: `${role}-model` },
    provider_model: { response_id: `resp-${role}`, model: `${role}-model`, system_fingerprint: null },
    content: JSON.stringify({ schema_version: 1, role, verdict: findings.length === 0 ? 'pass' : 'fail', summary: findings.length === 0 ? 'No findings.' : 'Problem.', findings }),
  };
}

test('preparation binds lifecycle and profile/prompt hashes to the canonical input', async () => {
  const value = await candidate();
  assert.equal(value.generation.lifecycle_epoch.kind, 'initial');
  assert.equal(value.profile_shas.reviewer.length, 64);
  assert.equal(value.prompt_shas.security.length, 64);
  assert.equal(buildReviewRequest('reviewer', value.input, models.reviewer).prompt_sha, value.prompt_shas.reviewer);
  assert.deepEqual(value.input.author, { login: 'contributor', id: 456, type: 'User' });
  await assert.rejects(() => candidate(client({ getPullLifecycle: async () => ({ head_sha: sha('d'), base_sha: sha('b'), lifecycle_epoch: lifecycle }) })), /lifecycle snapshot/);
  let reads = 0;
  await assert.rejects(
    () => candidate(client({ getPull: async () => {
      reads += 1;
      const value = await client().getPull();
      return reads === 1 ? value : { ...value, author: { ...value.author, id: 999 } };
    } })),
    /pull snapshot changed/,
  );
});

test('analysis records provider evidence and fails every role on incomplete coverage or sensitive output', async () => {
  const value = await candidate();
  const calls = [];
  const analysis = await analyzeAiReview({ candidate: value, apiKey: 'secret-key', modelClient: { complete: async ({ candidates }) => { calls.push(candidates[0].id); return completion(candidates[0].alias); } } });
  assert.deepEqual(calls, ['reviewer-model', 'security-model']);
  assert.equal(analysis.results.reviewer.provider_model.response_id, 'resp-reviewer');
  const incomplete = structuredClone(value); incomplete.coverage.complete = false;
  const failed = await analyzeAiReview({ candidate: incomplete, apiKey: 'secret-key', modelClient: { complete: async () => { throw new Error('must not run'); } } });
  assert.equal(failed.results.reviewer.failure.code, 'incomplete_coverage');
  assert.equal(containsSensitiveReviewOutput({ details: 'Authorization: Bearer abcdefgh' }, ''), true);
  const sensitive = await analyzeAiReview({ candidate: value, apiKey: 'secret-key', modelClient: { complete: async ({ candidates }) => ({ ...completion(candidates[0].alias), content: JSON.stringify({ schema_version: 1, role: candidates[0].alias, verdict: 'pass', summary: 'Bearer abcdefgh', findings: [] }) }) } });
  assert.equal(sensitive.results.reviewer.failure.code, 'sensitive_model_output');
  const providerLeak = await analyzeAiReview({ candidate: value, apiKey: 'secret-key', modelClient: { complete: async ({ candidates }) => ({ ...completion(candidates[0].alias), provider_model: { response_id: 'secret-key', model: `${candidates[0].alias}-model`, system_fingerprint: null } }) } });
  assert.equal(providerLeak.results.reviewer.failure.code, 'sensitive_model_output');
});

test('finalization publishes both terminal checks and fails closed on findings or stale generation', async () => {
  const value = await candidate();
  const analysis = await analyzeAiReview({ candidate: value, apiKey: 'secret-key', modelClient: { complete: async ({ candidates }) => completion(candidates[0].alias) } });
  const calls = [];
  const publisher = { publishCompletedReviewCheck: async (entry) => { calls.push(entry); return { id: calls.length, conclusion: entry.conclusion }; } };
  const result = await finalizeAiReview({ client: publisher, candidate: value, analysis, freshCandidate: async () => structuredClone(value), runGroupId: '42', runId: '42.1', detailsUrl: 'https://github.com/run', clock: () => new Date('2026-08-20T00:00:00Z') });
  assert.equal(result.state, 'success');
  assert.deepEqual(calls.map((call) => [call.role, call.conclusion]), [['reviewer', 'success'], ['security', 'success']]);
  const findings = structuredClone(analysis);
  findings.results.reviewer.output = { schema_version: 1, role: 'reviewer', verdict: 'fail', summary: 'Problem.', findings: [{ severity: 'low', title: 'Bug', details: 'Details', path: 'README.md', line: 1 }] };
  findings.results.reviewer.result_sha = (await import('../src/review-attestation-contract.mjs')).sha256((await import('../src/review-attestation-contract.mjs')).canonicalJson(findings.results.reviewer.output));
  calls.length = 0;
  const failed = await finalizeAiReview({ client: publisher, candidate: value, analysis: findings, freshCandidate: async () => structuredClone(value), runGroupId: '42', runId: '42.2', detailsUrl: 'https://github.com/run' });
  assert.equal(failed.state, 'failure');
  assert.deepEqual(calls.map((call) => call.conclusion), ['failure', 'success']);
  calls.length = 0;
  const stale = await finalizeAiReview({ client: publisher, candidate: value, analysis, freshCandidate: async () => { const next = structuredClone(value); next.generation.head_sha = sha('d'); return next; }, forcedFailureCode: null, runGroupId: '42', runId: '42.3', detailsUrl: 'https://github.com/run' });
  assert.equal(stale.state, 'failure');
  assert.deepEqual(calls.map((call) => call.conclusion), ['failure', 'failure']);
  for (const [code, expected] of [['cancelled', 'cancelled'], ['timeout', 'timed_out'], ['analysis_artifact_missing', 'failure'], ['candidate_artifact_missing', 'failure']]) {
    calls.length = 0;
    const terminal = await finalizeAiReview({ client: publisher, candidate: value, analysis: null, freshCandidate: async () => structuredClone(value), forcedFailureCode: code, runGroupId: '42', runId: `42.${code}`, detailsUrl: 'https://github.com/run' });
    assert.equal(terminal.state, 'failure');
    assert.deepEqual(calls.map((call) => call.conclusion), [expected, expected]);
  }
  assert.equal(candidateSha(value).length, 64);
});
