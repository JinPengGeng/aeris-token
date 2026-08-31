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

const completionResponse = (content = '{"ok":true}', options = {}) => {
  const message = { content };
  if (Object.hasOwn(options, 'refusal')) message.refusal = options.refusal;
  return jsonResponse({
    choices: [{ finish_reason: options.finishReason ?? 'stop', message }],
    ...(options.usage ? { usage: options.usage } : {}),
  });
};

test('429 switches to the approved fallback model', async () => {
  const calls = [];
  const api = client(
    [
      jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }),
      jsonResponse({ error: { message: 'limited' } }, 429),
      completionResponse('{"ok":true}', { usage: { total_tokens: 4 } }),
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
      completionResponse(),
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
      completionResponse(),
    ],
    calls,
    { connectTimeoutMs: 10, timeoutMs: 200, deadlineAtMs: Date.now() + 400 },
  );

  const result = await api.complete({ candidates, messages: [] });
  assert.equal(result.model.alias, 'fallback');
  assert.equal(calls.length, 3);
});

test('shared total timeout includes model discovery', async (t) => {
  t.mock.timers.enable({ apis: ['Date', 'setTimeout'] });
  const calls = [];
  const stalledModels = (_url, init) =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => resolve(jsonResponse({ data: [] })), 500);
      init.signal.addEventListener(
        'abort',
        () => {
          clearTimeout(timer);
          reject(new DOMException('aborted', 'AbortError'));
        },
        { once: true },
      );
    });
  const api = client([stalledModels], calls, { timeoutMs: 200, deadlineAtMs: Date.now() + 20 });

  const completion = api.complete({ candidates, messages: [] });
  const assertion = assert.rejects(
    completion,
    (error) => error instanceof AiRequestError && error.code === 'timeout' && !error.retryable,
  );
  t.mock.timers.tick(20);
  await assertion;
  assert.equal(calls.length, 1);
});

test('shared total timeout prevents another fallback request after the deadline', async (t) => {
  t.mock.timers.enable({ apis: ['Date', 'setTimeout'] });
  const calls = [];
  const stalledCompletion = (_url, init) =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => resolve(completionResponse()), 500);
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
      stalledCompletion,
      completionResponse(),
    ],
    calls,
    { timeoutMs: 200, deadlineAtMs: Date.now() + 20 },
  );

  const completion = api.complete({ candidates, messages: [] });
  const assertion = assert.rejects(
    completion,
    (error) => error instanceof AiRequestError && error.code === 'timeout' && !error.retryable,
  );
  // Model discovery resolves without timers; wait until the first completion
  // request is in flight so only its deadline timers remain pending.
  while (calls.length < 2) await new Promise((resolve) => setImmediate(resolve));
  t.mock.timers.tick(20);
  await assertion;
  assert.equal(calls.length, 2);
});

test('absolute deadline is not extended when completion starts later', async () => {
  const calls = [];
  const api = client(
    [jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] })],
    calls,
    { timeoutMs: 200, deadlineAtMs: Date.now() + 5 },
  );
  await new Promise((resolve) => setTimeout(resolve, 15));

  await assert.rejects(
    () => api.complete({ candidates, messages: [] }),
    (error) => error instanceof AiRequestError && error.code === 'timeout' && !error.retryable,
  );
  assert.equal(calls.length, 0);
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
      completionResponse(),
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
      completionResponse(),
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
      completionResponse(),
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
      completionResponse(),
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
  const responseFormat = {
    type: 'json_schema',
    json_schema: { name: 'test_output', strict: true, schema: { type: 'object' } },
  };
  const result = await api.complete({
    candidates: [{ alias: 'role', id: 'fast-model' }],
    messages: [],
    responseFormat,
  });
  assert.equal(result.content, '{"ok":true}');
  assert.deepEqual(result.usage, { total_tokens: 7 });
  const body = JSON.parse(calls[1].init.body);
  assert.equal(body.stream, true);
  assert.deepEqual(body.stream_options, { include_usage: true });
  assert.deepEqual(body.response_format, responseFormat);
});

test('streamed refusal fails without exposing refusal text or using fallback', async () => {
  const refusalText = 'private refusal details';
  const refusalChunks = ['private refusal ', 'details'];
  const body = [
    `data: ${JSON.stringify({ choices: [{ delta: { content: '{"ok":true}' } }] })}`,
    '',
    ...refusalChunks.flatMap((refusal) => [
      `data: ${JSON.stringify({ choices: [{ delta: { refusal } }] })}`,
      '',
    ]),
    `data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: 'stop' }] })}`,
    '',
    'data: [DONE]',
    '',
  ].join('\n');
  const calls = [];
  const api = client(
    [
      jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }),
      new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } }),
      completionResponse(),
    ],
    calls,
  );
  let captured;
  await assert.rejects(
    () => api.complete({ candidates, messages: [] }),
    (error) => {
      captured = error;
      return error instanceof AiRequestError && error.code === 'model_refusal' && !error.retryable;
    },
  );
  assert.equal(calls.length, 2);
  assert.equal(captured.message.includes(refusalText), false);
});

test('streamed truncation takes precedence over refusal', async () => {
  for (const refusal of ['do not expose', {}]) {
    const body = [
      `data: ${JSON.stringify({ choices: [{ delta: { refusal } }] })}`,
      '',
      `data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: 'length' }] })}`,
      '',
      'data: [DONE]',
      '',
    ].join('\n');
    const calls = [];
    const api = client(
      [
        jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }),
        new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } }),
        completionResponse(),
      ],
      calls,
    );
    await assert.rejects(
      () => api.complete({ candidates, messages: [] }),
      (error) => error instanceof AiRequestError && error.code === 'output_truncated',
    );
    assert.equal(calls.length, 2);
  }
});

test('stream accepts empty or null refusal and rejects invalid refusal types', async () => {
  for (const refusal of [null, '']) {
    const validBody = [
      `data: ${JSON.stringify({ choices: [{ delta: { content: '{"ok":true}', refusal } }] })}`,
      '',
      `data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: 'stop' }] })}`,
      '',
      'data: [DONE]',
      '',
    ].join('\n');
    const validApi = client(
      [
        jsonResponse({ data: [{ id: 'fast-model' }] }),
        new Response(validBody, { status: 200, headers: { 'content-type': 'text/event-stream' } }),
      ],
      [],
    );
    const result = await validApi.complete({
      candidates: [{ alias: 'role', id: 'fast-model' }],
      messages: [],
    });
    assert.equal(result.content, '{"ok":true}');
  }

  for (const refusal of [{}, [], 1]) {
    const invalidBody = [
      `data: ${JSON.stringify({ choices: [{ delta: { refusal } }] })}`,
      '',
      `data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: 'stop' }] })}`,
      '',
      'data: [DONE]',
      '',
    ].join('\n');
    const calls = [];
    const api = client(
      [
        jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }),
        new Response(invalidBody, { status: 200, headers: { 'content-type': 'text/event-stream' } }),
        completionResponse(),
      ],
      calls,
    );
    await assert.rejects(
      () => api.complete({ candidates, messages: [] }),
      (error) => error instanceof AiRequestError && error.code === 'invalid_chat_response',
    );
    assert.equal(calls.length, 2);
  }
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

test('stream truncated at the token limit fails without fallback', async () => {
  const truncatedBody = [
    `data: ${JSON.stringify({ choices: [{ delta: { content: '{"partial":' } }] })}`,
    '',
    `data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: 'length' }] })}`,
    '',
    `data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: 'stop' }] })}`,
    '',
    'data: [DONE]',
    '',
  ].join('\n');
  const sseResponse = () =>
    new Response(truncatedBody, {
      status: 200,
      headers: { 'content-type': 'text/event-stream' },
    });
  const calls = [];
  const api = client(
    [jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }), sseResponse()],
    calls,
  );
  await assert.rejects(
    () => api.complete({ candidates, messages: [] }),
    (error) => error instanceof AiRequestError && error.code === 'output_truncated' && !error.retryable,
  );
  assert.equal(calls.length, 2);
});

test('stream reports truncation when length follows an earlier stop reason', async () => {
  const body = [
    `data: ${JSON.stringify({ choices: [{ delta: { content: '{"partial":' } }] })}`,
    '',
    `data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: 'stop' }] })}`,
    '',
    `data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: 'length' }] })}`,
    '',
    'data: [DONE]',
    '',
  ].join('\n');
  const api = client(
    [
      jsonResponse({ data: [{ id: 'fast-model' }] }),
      new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } }),
    ],
    [],
  );
  await assert.rejects(
    () => api.complete({ candidates: [{ alias: 'role', id: 'fast-model' }], messages: [] }),
    (error) => error instanceof AiRequestError && error.code === 'output_truncated',
  );
});

test('stream rejects unsupported finish reasons and data after DONE', async () => {
  const bodies = [
    [
      `data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: 'content_filter' }] })}`,
      '',
      'data: [DONE]',
      '',
    ].join('\n'),
    [
      `data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: 'stop' }] })}`,
      '',
      'data: [DONE]',
      '',
      `data: ${JSON.stringify({ choices: [{ delta: { content: 'late' } }] })}`,
      '',
    ].join('\n'),
  ];
  for (const body of bodies) {
    const api = client(
      [
        jsonResponse({ data: [{ id: 'fast-model' }] }),
        new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } }),
      ],
      [],
    );
    await assert.rejects(
      () => api.complete({ candidates: [{ alias: 'role', id: 'fast-model' }], messages: [] }),
      (error) => error instanceof AiRequestError && error.code === 'invalid_chat_response',
    );
  }
});

test('non-stream JSON response still works when the gateway ignores stream', async () => {
  const calls = [];
  const api = client(
    [
      jsonResponse({ data: [{ id: 'fast-model' }] }),
      completionResponse('{"ok":true}', { usage: { total_tokens: 3 } }),
    ],
    calls,
  );
  const result = await api.complete({ candidates: [{ alias: 'role', id: 'fast-model' }], messages: [] });
  assert.equal(result.content, '{"ok":true}');
  assert.equal(JSON.parse(calls[1].init.body).stream, true);
});

test('non-stream refusal fails before content handling and without fallback', async () => {
  const refusalText = 'private refusal details';
  for (const content of [null, '{"ok":true}']) {
    const calls = [];
    const api = client(
      [
        jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }),
        completionResponse(content, { refusal: refusalText }),
        completionResponse(),
      ],
      calls,
    );
    let captured;
    await assert.rejects(
      () => api.complete({ candidates, messages: [] }),
      (error) => {
        captured = error;
        return error instanceof AiRequestError && error.code === 'model_refusal' && !error.retryable;
      },
    );
    assert.equal(calls.length, 2);
    assert.equal(captured.message.includes(refusalText), false);
  }
});

test('non-stream truncation takes precedence over refusal', async () => {
  for (const refusal of ['do not expose', {}]) {
    const calls = [];
    const api = client(
      [
        jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }),
        completionResponse(null, { finishReason: 'length', refusal }),
        completionResponse(),
      ],
      calls,
    );
    await assert.rejects(
      () => api.complete({ candidates, messages: [] }),
      (error) => error instanceof AiRequestError && error.code === 'output_truncated',
    );
    assert.equal(calls.length, 2);
  }
});

test('non-stream accepts empty or null refusal and rejects invalid refusal types', async () => {
  for (const refusal of [null, '']) {
    const validApi = client(
      [
        jsonResponse({ data: [{ id: 'fast-model' }] }),
        completionResponse('{"ok":true}', { refusal }),
      ],
      [],
    );
    const result = await validApi.complete({
      candidates: [{ alias: 'role', id: 'fast-model' }],
      messages: [],
    });
    assert.equal(result.content, '{"ok":true}');
  }

  for (const refusal of [{}, [], 1]) {
    const calls = [];
    const api = client(
      [
        jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }),
        completionResponse('{"ok":true}', { refusal }),
        completionResponse(),
      ],
      calls,
    );
    await assert.rejects(
      () => api.complete({ candidates, messages: [] }),
      (error) => error instanceof AiRequestError && error.code === 'invalid_chat_response',
    );
    assert.equal(calls.length, 2);
  }
});

test('non-stream response truncated at the token limit fails without fallback', async () => {
  for (const content of ['{"partial":', null]) {
    const calls = [];
    const api = client(
      [
        jsonResponse({ data: [{ id: 'fast-model' }, { id: 'strong-model' }] }),
        completionResponse(content, { finishReason: 'length' }),
      ],
      calls,
    );
    await assert.rejects(
      () => api.complete({ candidates, messages: [] }),
      (error) => error instanceof AiRequestError && error.code === 'output_truncated' && !error.retryable,
    );
    assert.equal(calls.length, 2);
  }
});

test('non-stream response without a finish reason fails closed', async () => {
  const api = client(
    [
      jsonResponse({ data: [{ id: 'fast-model' }] }),
      jsonResponse({ choices: [{ message: { content: '{"ok":true}' } }] }),
    ],
    [],
  );
  await assert.rejects(
    () => api.complete({ candidates: [{ alias: 'role', id: 'fast-model' }], messages: [] }),
    (error) => error instanceof AiRequestError && error.code === 'invalid_chat_response',
  );
});

test('unsupported finish reasons fail closed even when content is valid JSON', async () => {
  const calls = [];
  const api = client(
    [
      jsonResponse({ data: [{ id: 'fast-model' }] }),
      completionResponse('{"ok":true}', { finishReason: 'content_filter' }),
    ],
    calls,
  );
  await assert.rejects(
    () => api.complete({ candidates: [{ alias: 'role', id: 'fast-model' }], messages: [] }),
    (error) => error instanceof AiRequestError && error.code === 'invalid_chat_response',
  );
});
