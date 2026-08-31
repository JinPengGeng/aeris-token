import assert from 'node:assert/strict';
import test from 'node:test';

import { AiRequestError } from '../src/openai-client.mjs';
import { createOpenAiResponsesExecutor } from '../src/ai-executors/openai-responses-v1.mjs';

const identity = { id: 'openai-responses-v1', protocol: 'openai-responses-v1' };
const response = (value, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'content-type': 'application/json' } });

test('Responses adapter normalizes completed output and JSON schema format', async () => {
  const calls = [];
  const executor = createOpenAiResponsesExecutor({
    baseUrl: 'https://ai.example.test/v1', apiKey: 'key', retryableStatuses: [429],
    fetchImpl: async (url, init) => {
      calls.push({ url, init });
      return url.endsWith('/models')
        ? response({ data: [{ id: 'model' }] })
        : response({ status: 'completed', output: [{ type: 'message', content: [{ type: 'output_text', text: '{"ok":true}' }] }], usage: { total_tokens: 3 } });
    },
  }, identity);
  const result = await executor.complete({
    candidates: [{ alias: 'role', id: 'model' }], messages: [{ role: 'user', content: 'x' }],
    responseFormat: { type: 'json_schema', json_schema: { name: 'output', strict: true, schema: { type: 'object' } } },
  });
  assert.equal(result.content, '{"ok":true}');
  assert.deepEqual(result.executor, identity);
  const body = JSON.parse(calls[1].init.body);
  assert.equal(calls[1].url.endsWith('/responses'), true);
  assert.deepEqual(body.text.format, { type: 'json_schema', name: 'output', strict: true, schema: { type: 'object' } });
});

test('Responses adapter fails closed on incomplete, refusal, and empty output', async () => {
  for (const payload of [
    { status: 'incomplete', incomplete_details: { reason: 'max_output_tokens' }, output: [] },
    { status: 'completed', output: [{ type: 'message', content: [{ type: 'refusal', refusal: 'no' }] }] },
    { status: 'completed', output: [] },
  ]) {
    const executor = createOpenAiResponsesExecutor({
      baseUrl: 'https://ai.example.test/v1', apiKey: 'key', retryableStatuses: [429],
      fetchImpl: async (url) => url.endsWith('/models') ? response({ data: [{ id: 'model' }] }) : response(payload),
    }, identity);
    await assert.rejects(
      () => executor.complete({ candidates: [{ alias: 'role', id: 'model' }], messages: [] }),
      (error) => error instanceof AiRequestError && ['output_truncated', 'model_refusal', 'invalid_responses_response'].includes(error.code),
    );
  }
});

test('Responses adapter shares one absolute deadline across discovery and completion', async () => {
  const stalled = (_url, init) => new Promise((resolve, reject) => {
    const timer = setTimeout(() => resolve(response({ status: 'completed', output: [] })), 500);
    init.signal.addEventListener('abort', () => {
      clearTimeout(timer);
      reject(new DOMException('aborted', 'AbortError'));
    }, { once: true });
  });
  const calls = [];
  const executor = createOpenAiResponsesExecutor({
    baseUrl: 'https://ai.example.test/v1', apiKey: 'key', retryableStatuses: [429],
    timeoutMs: 200, connectTimeoutMs: 200, deadlineAtMs: Date.now() + 25,
    fetchImpl: async (url, init) => {
      calls.push({ url, init });
      return url.endsWith('/models') ? response({ data: [{ id: 'model' }] }) : stalled(url, init);
    },
  }, identity);
  await assert.rejects(
    () => executor.complete({ candidates: [{ alias: 'role', id: 'model' }], messages: [] }),
    (error) => error instanceof AiRequestError && error.code === 'timeout' && !error.retryable,
  );
  assert.equal(calls.length, 2);
});
