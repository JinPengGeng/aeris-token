import assert from 'node:assert/strict';
import test from 'node:test';

import { AiRequestError, OpenAICompatibleClient } from '../src/openai-client.mjs';

const jsonResponse = (payload, status = 200, headers = {}) =>
  new Response(JSON.stringify(payload), {
    status,
    headers: { 'content-type': 'application/json', ...headers },
  });

function queuedFetch(entries, calls) {
  return async (url, init) => {
    calls.push({ url, init });
    const next = entries.shift();
    if (next instanceof Error) throw next;
    if (typeof next === 'function') return next(url, init);
    return next;
  };
}

function client(entries, calls, overrides = {}) {
  return new OpenAICompatibleClient({
    baseUrl: 'https://ai.example.test/v1',
    apiKey: 'test-key',
    retryableStatuses: [408, 429, 500, 502, 503, 504],
    fetchImpl: queuedFetch(entries, calls),
    ...overrides,
  });
}

const candidates = [
  { alias: 'role', id: 'fast-model' },
  { alias: 'fallback', id: 'strong-model' },
];

test('429 switches to the approved fallback model', async () => {
  const calls = [];
  const api = client(
    [
      jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }),
      jsonResponse({ error: { message: 'limited' } }, 429),
      jsonResponse({ choices: [{ message: { content: '{"ok":true}' } }], usage: { total_tokens: 4 } }),
    ],
    calls,
  );
  const result = await api.complete({ candidates, messages: [{ role: 'user', content: 'test' }] });
  assert.equal(result.model.id, 'strong-model');
  assert.equal(calls.length, 3);
  assert.equal(JSON.parse(calls[1].init.body).model, 'fast-model');
  assert.equal(JSON.parse(calls[2].init.body).model, 'strong-model');
});

test('401 is terminal and does not switch models', async () => {
  const calls = [];
  const api = client(
    [
      jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }),
      jsonResponse({ error: { message: 'unauthorized' } }, 401),
    ],
    calls,
  );
  await assert.rejects(
    () => api.complete({ candidates, messages: [] }),
    (error) => error instanceof AiRequestError && error.status === 401 && !error.retryable,
  );
  assert.equal(calls.length, 2);
});

test('connection failure can switch to fallback', async () => {
  const calls = [];
  const api = client(
    [
      jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }),
      new TypeError('offline'),
      jsonResponse({ choices: [{ message: { content: '{"ok":true}' } }] }),
    ],
    calls,
  );
  const result = await api.complete({ candidates, messages: [] });
  assert.equal(result.model.alias, 'fallback');
});

test('connect timeout can switch to fallback while the total timeout remains available', async () => {
  const calls = [];
  const stalledHeaders = (_url, init) =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => resolve(jsonResponse({ choices: [] })), 500);
      init.signal.addEventListener(
        'abort',
        () => {
          clearTimeout(timer);
          reject(new DOMException('aborted', 'AbortError'));
        },
        { once: true },
      );
    });
  const api = client(
    [
      jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }),
      stalledHeaders,
      jsonResponse({ choices: [{ message: { content: '{"ok":true}' } }] }),
    ],
    calls,
    { connectTimeoutMs: 10, timeoutMs: 200 },
  );

  const result = await api.complete({ candidates, messages: [] });
  assert.equal(result.model.alias, 'fallback');
  assert.equal(calls.length, 3);
});

test('connect timer stops after headers while the response body is still streaming', async () => {
  const calls = [];
  const delayedBody = (_url, init) =>
    new Response(
      new ReadableStream({
        start(controller) {
          const timer = setTimeout(() => {
            controller.enqueue(new TextEncoder().encode('{"data":[{"id":"fast-model"}]}'));
            controller.close();
          }, 40);
          init.signal.addEventListener(
            'abort',
            () => {
              clearTimeout(timer);
              controller.error(new DOMException('aborted', 'AbortError'));
            },
            { once: true },
          );
        },
      }),
      { status: 200, headers: { 'content-type': 'application/json' } },
    );
  const api = client([delayedBody], calls, { connectTimeoutMs: 10, timeoutMs: 200 });

  const ids = await api.listModelIds();
  assert.deepEqual([...ids], ['fast-model']);
  assert.equal(calls[0].init.signal.aborted, false);
});

test('response body timeout can switch to fallback', async () => {
  const calls = [];
  const stalledResponse = (_url, init) => {
    const body = new ReadableStream({
      start(controller) {
        init.signal.addEventListener(
          'abort',
          () => controller.error(new DOMException('aborted', 'AbortError')),
          { once: true },
        );
      },
    });
    return new Response(body, { status: 200, headers: { 'content-type': 'application/json' } });
  };
  const api = client(
    [
      jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }),
      stalledResponse,
      jsonResponse({ choices: [{ message: { content: '{"ok":true}' } }] }),
    ],
    calls,
    { timeoutMs: 10 },
  );

  const result = await api.complete({ candidates, messages: [] });
  assert.equal(result.model.alias, 'fallback');
  assert.equal(calls.length, 3);
});

test('response body connection failure can switch to fallback', async () => {
  const calls = [];
  const brokenResponse = () =>
    new Response(
      new ReadableStream({
        start(controller) {
          controller.error(new TypeError('socket closed'));
        },
      }),
      { status: 200, headers: { 'content-type': 'application/json' } },
    );
  const api = client(
    [
      jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }),
      brokenResponse,
      jsonResponse({ choices: [{ message: { content: '{"ok":true}' } }] }),
    ],
    calls,
  );

  const result = await api.complete({ candidates, messages: [] });
  assert.equal(result.model.alias, 'fallback');
  assert.equal(calls.length, 3);
});

test('abort errors can switch to fallback even when a fetch mock does not observe the signal', async () => {
  const calls = [];
  const api = client(
    [
      jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }),
      new DOMException('upstream timeout', 'AbortError'),
      jsonResponse({ choices: [{ message: { content: '{"ok":true}' } }] }),
    ],
    calls,
  );

  const result = await api.complete({ candidates, messages: [] });
  assert.equal(result.model.alias, 'fallback');
  assert.equal(calls.length, 3);
});

test('arbitrary retryable errors cannot switch to fallback', async () => {
  const calls = [];
  const api = client(
    [
      jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }),
      new AiRequestError('unexpected retry request', {
        code: 'invalid_chat_response',
        retryable: true,
      }),
      jsonResponse({ choices: [{ message: { content: '{"ok":true}' } }] }),
    ],
    calls,
  );

  await assert.rejects(
    () => api.complete({ candidates, messages: [] }),
    (error) => error instanceof AiRequestError && error.code === 'invalid_chat_response',
  );
  assert.equal(calls.length, 2);
});

test('unknown configured model fails before chat completion', async () => {
  const calls = [];
  const api = client([jsonResponse({ data: [{ id: 'fast-model' }] })], calls);
  await assert.rejects(
    () => api.complete({ candidates, messages: [] }),
    (error) => error instanceof AiRequestError && error.code === 'model_not_allowed',
  );
  assert.equal(calls.length, 1);
});

test('oversized response is terminal', async () => {
  const calls = [];
  const api = client(
    [jsonResponse({ data: [{ id: 'fast-model' }] }, 200, { 'content-length': '999' })],
    calls,
    { maximumResponseBytes: 20 },
  );
  await assert.rejects(
    () => api.listModelIds(),
    (error) => error instanceof AiRequestError && error.code === 'response_too_large',
  );
});

test('base URL must use HTTPS', () => {
  assert.throws(
    () =>
      new OpenAICompatibleClient({
        baseUrl: 'http://ai.example.test/v1',
        apiKey: 'test-key',
        retryableStatuses: [],
      }),
    (error) => error instanceof AiRequestError && error.code === 'invalid_base_url',
  );
});

test('streamed SSE completion aggregates deltas, usage, and DONE', async () => {
  const calls = [];
  const sseBody = [
    `data: ${JSON.stringify({ choices: [{ delta: { content: '{"ok":' } }] })}`,
    '',
    `data: ${JSON.stringify({ choices: [{ delta: { content: 'true}' } }] })}`,
    '',
    `data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: 'stop' }], usage: { total_tokens: 7 } })}`,
    '',
    'data: [DONE]',
    '',
    '',
  ].join('\n');
  const sseResponse = () =>
    new Response(sseBody, {
      status: 200,
      headers: { 'content-type': 'text/event-stream' },
    });
  const api = client(
    [jsonResponse({ data: [{ id: 'fast-model' }] }), sseResponse()],
    calls,
  );
  const result = await api.complete({ candidates: [{ alias: 'role', id: 'fast-model' }], messages: [] });
  assert.equal(result.content, '{"ok":true}');
  assert.deepEqual(result.usage, { total_tokens: 7 });
  const body = JSON.parse(calls[1].init.body);
  assert.equal(body.stream, true);
  assert.deepEqual(body.stream_options, { include_usage: true });
});

test('stream that ends without DONE or finish_reason is invalid', async () => {
  const sseResponse = () =>
    new Response('data: {"choices":[{"delta":{"content":"partial"}}]}\n\n', {
      status: 200,
      headers: { 'content-type': 'text/event-stream' },
    });
  const api = client(
    [jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }), sseResponse()],
    [],
  );
  await assert.rejects(
    () => api.complete({ candidates, messages: [] }),
    (error) => error instanceof AiRequestError && error.code === 'invalid_chat_response',
  );
});

test('non-stream JSON response still works when the gateway ignores stream', async () => {
  const calls = [];
  const api = client(
    [
      jsonResponse({ data: [{ id: 'fast-model' }] }),
      jsonResponse({ choices: [{ message: { content: '{"ok":true}' } }], usage: { total_tokens: 3 } }),
    ],
    calls,
  );
  const result = await api.complete({ candidates: [{ alias: 'role', id: 'fast-model' }], messages: [] });
  assert.equal(result.content, '{"ok":true}');
  assert.equal(JSON.parse(calls[1].init.body).stream, true);
});
