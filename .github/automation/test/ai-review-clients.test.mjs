import assert from 'node:assert/strict';
import test from 'node:test';

import { AiReviewGitHubClient } from '../src/ai-review-github-client.mjs';
import { AiReviewModelClient, AiReviewRequestError } from '../src/ai-review-model-client.mjs';

const sha = (character) => character.repeat(40);
const json = (value, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'content-type': 'application/json' } });
const profile = {
  request: { endpoint: '/chat/completions', temperature: 0.1, maximum_output_tokens: 4000 },
  response_format: { name: 'review', strict: true, schema: { type: 'object' } },
};

test('model client records requested and provider-reported identity and uses fallback only for retryable failures', async () => {
  const calls = [];
  const replies = [
    json({ error: { message: 'limited' } }, 429),
    json({ id: 'chatcmpl-42', model: 'fallback-model', system_fingerprint: 'fp-42', choices: [{ finish_reason: 'stop', message: { content: '{}' } }] }),
  ];
  const client = new AiReviewModelClient({
    baseUrl: 'https://ai.example.test/v1', apiKey: 'key',
    fetchImpl: async (url, init) => { calls.push({ url, body: JSON.parse(init.body) }); return replies.shift(); },
  });
  const result = await client.complete({ candidates: [{ alias: 'security', id: 'primary-model' }, { alias: 'fallback', id: 'fallback-model' }], messages: [], profile });
  assert.deepEqual(result.requested_model, { alias: 'fallback', id: 'fallback-model' });
  assert.deepEqual(result.provider_model, { response_id: 'chatcmpl-42', model: 'fallback-model', system_fingerprint: 'fp-42' });
  assert.deepEqual(calls.map((call) => call.body.model), ['primary-model', 'fallback-model']);
  assert.equal(calls[1].body.stream, false);
});

test('model client rejects an undeclared provider model alias', async () => {
  const client = new AiReviewModelClient({
    baseUrl: 'https://ai.example.test/v1', apiKey: 'key',
    fetchImpl: async () => json({ id: 'chatcmpl-42', model: 'provider-alias', system_fingerprint: null, choices: [{ finish_reason: 'stop', message: { content: '{}' } }] }),
  });
  await assert.rejects(
    () => client.complete({ candidates: [{ alias: 'reviewer', id: 'approved-model' }], messages: [], profile }),
    (error) => error instanceof AiReviewRequestError && error.code === 'provider_model_mismatch',
  );
});

test('model client reports a timeout while reading the response body', async () => {
  const client = new AiReviewModelClient({
    baseUrl: 'https://ai.example.test/v1', apiKey: 'key', timeoutMs: 1_000,
    fetchImpl: async (_url, init) => new Response(new ReadableStream({
      start(controller) {
        init.signal.addEventListener('abort', () => controller.error(new DOMException('aborted', 'AbortError')), { once: true });
      },
    })),
  });
  const started = Date.now();
  await assert.rejects(
    () => client.complete({ candidates: [{ alias: 'reviewer', id: 'approved-model' }], messages: [], profile }),
    (error) => error instanceof AiReviewRequestError && error.code === 'timeout',
  );
  assert.ok(Date.now() - started < 2_000);
});

test('model client fails closed when provider identity is missing', async () => {
  const client = new AiReviewModelClient({
    baseUrl: 'https://ai.example.test/v1', apiKey: 'key',
    fetchImpl: async () => json({ choices: [{ finish_reason: 'stop', message: { content: '{}' } }] }),
  });
  await assert.rejects(
    () => client.complete({ candidates: [{ alias: 'reviewer', id: 'model' }], messages: [], profile }),
    (error) => error instanceof AiReviewRequestError && error.code === 'invalid_model_response',
  );
});

test('GitHub client creates a completed check and verifies GitHub Actions App ownership and latest-ID dominance', async () => {
  const calls = [];
  let created = null;
  const fetchImpl = async (url, init) => {
    calls.push({ url, init });
    if (url.includes('/check-runs?')) return json({ check_runs: created === null ? [] : [created] });
    if (init.method === 'POST' && url.endsWith('/check-runs')) {
      const body = JSON.parse(init.body);
      created = { id: 71, ...body, app: { id: 15368, slug: 'github-actions' } };
      return json(created);
    }
    if (url.endsWith('/check-runs/71')) return json(created);
    throw new Error(`unexpected request: ${url}`);
  };
  const client = new AiReviewGitHubClient({ token: 'token', repository: 'JinPengGeng/aeris-token', repositoryId: 123, fetchImpl });
  const check = await client.publishCompletedReviewCheck({
    role: 'reviewer', headSha: sha('a'), conclusion: 'success', externalId: 'aeris-review-attestation:v1:reviewer:abc',
    output: { title: 'Automation reviewer attestation: pass', summary: '{}' }, detailsUrl: 'https://github.com/run', completedAt: '2026-08-20T00:00:00Z',
  });
  assert.equal(check.id, 71);
  assert.equal(check.status, 'completed');
  assert.equal(check.app.id, 15368);
  assert.equal(calls.filter((call) => call.init.method === 'POST').length, 1);
});

test('GitHub client rejects a same-name check owned by another App before publication', async () => {
  const foreign = { id: 3, name: 'Automation Review Attestation / security', head_sha: sha('a'), status: 'completed', conclusion: 'success', app: { id: 99, slug: 'other' } };
  const client = new AiReviewGitHubClient({ token: 'token', repository: 'JinPengGeng/aeris-token', repositoryId: 123, fetchImpl: async () => json({ check_runs: [foreign] }) });
  await assert.rejects(
    () => client.publishCompletedReviewCheck({ role: 'security', headSha: sha('a'), conclusion: 'failure', externalId: 'failure', output: { title: 'failed', summary: '{}' }, detailsUrl: 'https://github.com/run', completedAt: '2026-08-20T00:00:00Z' }),
    /non-GitHub-Actions/,
  );
});
