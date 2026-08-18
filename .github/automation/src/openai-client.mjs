const encoder = new TextEncoder();

export class AiRequestError extends Error {
  constructor(message, { code, status = null, retryable = false } = {}) {
    super(message);
    this.name = 'AiRequestError';
    this.code = code ?? 'ai_request_error';
    this.status = status;
    this.retryable = retryable;
  }
}

function joinUrl(baseUrl, suffix) {
  return `${baseUrl.replace(/\/+$/, '')}/${suffix.replace(/^\/+/, '')}`;
}

async function readBoundedText(response, maximumBytes) {
  const contentLength = Number(response.headers.get('content-length'));
  if (Number.isFinite(contentLength) && contentLength > maximumBytes) {
    throw new AiRequestError('AI response exceeds the configured byte limit', {
      code: 'response_too_large',
    });
  }
  if (!response.body) return '';

  const reader = response.body.getReader();
  const chunks = [];
  let size = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > maximumBytes) {
      await reader.cancel();
      throw new AiRequestError('AI response exceeds the configured byte limit', {
        code: 'response_too_large',
      });
    }
    chunks.push(value);
  }
  const combined = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    combined.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return new TextDecoder().decode(combined);
}

function parseJson(text, code) {
  try {
    return JSON.parse(text);
  } catch {
    throw new AiRequestError('AI service returned invalid JSON', { code });
  }
}

function parseSseEvents(text) {
  const events = [];
  let data = null;
  for (const line of text.split(/\r?\n/)) {
    if (line.startsWith('data:')) {
      const payload = line.slice(5).trimStart();
      data = data === null ? payload : `${data}\n${payload}`;
    } else if (line === '' && data !== null) {
      events.push(data);
      data = null;
    }
  }
  if (data !== null) events.push(data);
  return events;
}

function requireCompleteFinishReason(finishReason, responseKind) {
  if (finishReason === 'length') {
    throw new AiRequestError(`AI ${responseKind} stopped at the token limit before completing`, {
      code: 'output_truncated',
      retryable: false,
    });
  }
  if (finishReason !== 'stop') {
    throw new AiRequestError(`AI ${responseKind} ended with an unsupported finish reason`, {
      code: 'invalid_chat_response',
    });
  }
}

function aggregateSseCompletion(text) {
  let content = '';
  let usage = null;
  let finishReason = null;
  let sawDone = false;
  for (const payload of parseSseEvents(text)) {
    if (payload === '[DONE]') {
      sawDone = true;
      continue;
    }
    if (sawDone) {
      throw new AiRequestError('AI stream returned data after the completion marker', {
        code: 'invalid_chat_response',
      });
    }
    let event;
    try {
      event = JSON.parse(payload);
    } catch {
      throw new AiRequestError('AI stream returned invalid JSON', { code: 'invalid_service_json' });
    }
    const delta = event.choices?.[0]?.delta?.content;
    if (typeof delta === 'string') content += delta;
    if (event.usage && typeof event.usage === 'object') usage = event.usage;
    const nextFinishReason = event.choices?.[0]?.finish_reason;
    if (nextFinishReason) {
      if (finishReason && finishReason !== nextFinishReason) {
        if (finishReason === 'length' || nextFinishReason === 'length') {
          finishReason = 'length';
        } else {
          throw new AiRequestError('AI stream returned conflicting finish reasons', {
            code: 'invalid_chat_response',
          });
        }
      } else {
        finishReason = nextFinishReason;
      }
    }
  }
  if (!finishReason && !sawDone) {
    throw new AiRequestError('AI stream ended without completion', {
      code: 'invalid_chat_response',
    });
  }
  requireCompleteFinishReason(finishReason ?? 'stop', 'stream');
  return { content, usage };
}

function createTimeout(controller, timeoutMs, message) {
  let timer;
  const promise = new Promise((_, reject) => {
    timer = setTimeout(() => {
      const error = new AiRequestError(message, { code: 'timeout', retryable: true });
      reject(error);
      controller.abort(error);
    }, timeoutMs);
  });
  return {
    promise,
    cancel() {
      clearTimeout(timer);
    },
  };
}

function canUseFallback(error, retryableStatuses) {
  if (!(error instanceof AiRequestError) || !error.retryable) return false;
  if (error.code === 'connect_error' || error.code === 'timeout') return true;
  return error.code === 'http_error' && retryableStatuses.has(error.status);
}

export class OpenAICompatibleClient {
  constructor({
    baseUrl,
    apiKey,
    endpoint = '/chat/completions',
    retryableStatuses,
    timeoutMs = 120_000,
    connectTimeoutMs = timeoutMs,
    maximumResponseBytes = 1_048_576,
    fetchImpl = globalThis.fetch,
  }) {
    const parsedBase = new URL(baseUrl);
    if (parsedBase.protocol !== 'https:') {
      throw new AiRequestError('AI base URL must use HTTPS', { code: 'invalid_base_url' });
    }
    if (parsedBase.username || parsedBase.password || parsedBase.search || parsedBase.hash) {
      throw new AiRequestError('AI base URL contains unsupported URL components', {
        code: 'invalid_base_url',
      });
    }
    if (!apiKey) throw new AiRequestError('AI API key is not configured', { code: 'missing_api_key' });
    this.baseUrl = parsedBase.toString().replace(/\/$/, '');
    this.apiKey = apiKey;
    this.endpoint = endpoint;
    this.retryableStatuses = new Set(retryableStatuses);
    this.timeoutMs = timeoutMs;
    this.connectTimeoutMs = connectTimeoutMs;
    this.maximumResponseBytes = maximumResponseBytes;
    this.fetchImpl = fetchImpl;
  }

  async request(path, init) {
    const { response, readBody, dispose } = await this.openRequest(path, init);
    try {
      if (!response.ok) {
        const errorText = await readBody();
        parseJson(errorText, 'invalid_service_json');
        throw new AiRequestError(`AI service returned HTTP ${response.status}`, {
          code: 'http_error',
          status: response.status,
          retryable: this.retryableStatuses.has(response.status),
        });
      }
      const text = await readBody();
      return parseJson(text, 'invalid_service_json');
    } catch (error) {
      return Promise.reject(this.normalizeTransportError(error, response));
    } finally {
      dispose();
    }
  }

  async openRequest(path, init) {
    const controller = new AbortController();
    const requestTimeout = createTimeout(controller, this.timeoutMs, 'AI request timed out');
    const connectTimeout = createTimeout(
      controller,
      this.connectTimeoutMs,
      'AI response headers timed out',
    );
    let response;
    try {
      try {
        const fetchPromise = this.fetchImpl(joinUrl(this.baseUrl, path), {
          ...init,
          signal: controller.signal,
          headers: {
            authorization: `Bearer ${this.apiKey}`,
            'content-type': 'application/json',
            ...(init.headers ?? {}),
          },
        });
        response = await Promise.race([
          fetchPromise,
          connectTimeout.promise,
          requestTimeout.promise,
        ]);
      } catch (error) {
        throw this.normalizeTransportError(error, null);
      } finally {
        connectTimeout.cancel();
      }
      const readBody = () =>
        Promise.race([
          readBoundedText(response, this.maximumResponseBytes),
          requestTimeout.promise.then(() => {
            throw new AiRequestError('AI request timed out', { code: 'timeout', retryable: true });
          }),
        ]);
      const dispose = () => {
        connectTimeout.cancel();
        requestTimeout.cancel();
      };
      return { response, readBody, dispose };
    } catch (error) {
      connectTimeout.cancel();
      requestTimeout.cancel();
      throw error;
    }
  }

  normalizeTransportError(error, response) {
    if (error instanceof AiRequestError) return error;
    if (error?.name === 'AbortError') {
      return new AiRequestError('AI request timed out', { code: 'timeout', retryable: true });
    }
    return new AiRequestError('AI connection failed', {
      code: 'connect_error',
      retryable: true,
    });
  }

  async listModelIds() {
    const payload = await this.request('/models', { method: 'GET', headers: {} });
    if (!Array.isArray(payload.data)) {
      throw new AiRequestError('AI model list has an invalid shape', { code: 'invalid_model_list' });
    }
    const ids = payload.data.map((entry) => entry?.id).filter((id) => typeof id === 'string');
    if (ids.length !== payload.data.length) {
      throw new AiRequestError('AI model list contains an invalid model entry', {
        code: 'invalid_model_list',
      });
    }
    return new Set(ids);
  }

  async complete({ candidates, messages, maxTokens = 1800, responseFormat = undefined }) {
    const availableModels = await this.listModelIds();
    for (const candidate of candidates) {
      if (!availableModels.has(candidate.id)) {
        throw new AiRequestError(`configured model is not present in /models: ${candidate.alias}`, {
          code: 'model_not_allowed',
        });
      }
    }

    let lastError;
    for (let index = 0; index < candidates.length; index += 1) {
      const candidate = candidates[index];
      const startedAt = Date.now();
      let opened = null;
      try {
        opened = await this.openRequest(this.endpoint, {
          method: 'POST',
          body: JSON.stringify({
            model: candidate.id,
            messages,
            temperature: 0.1,
            max_tokens: maxTokens,
            stream: true,
            stream_options: { include_usage: true },
            ...(responseFormat === undefined ? {} : { response_format: responseFormat }),
          }),
          headers: { accept: 'text/event-stream' },
        });
        const { response, readBody, dispose } = opened;
        try {
          if (!response.ok) {
            const errorText = await readBody();
            parseJson(errorText, 'invalid_service_json');
            throw new AiRequestError(`AI service returned HTTP ${response.status}`, {
              code: 'http_error',
              status: response.status,
              retryable: this.retryableStatuses.has(response.status),
            });
          }
          const contentType = response.headers.get('content-type') ?? '';
          const text = await readBody();
          if (contentType.includes('text/event-stream')) {
            const aggregated = aggregateSseCompletion(text);
            return {
              content: aggregated.content,
              model: candidate,
              durationMs: Date.now() - startedAt,
              usage: aggregated.usage,
            };
          }
          const payload = parseJson(text, 'invalid_service_json');
          const choice = payload.choices?.[0];
          requireCompleteFinishReason(choice?.finish_reason, 'response');
          const content = choice?.message?.content;
          if (typeof content !== 'string') {
            throw new AiRequestError('AI response does not contain message content', {
              code: 'invalid_chat_response',
            });
          }
          return {
            content,
            model: candidate,
            durationMs: Date.now() - startedAt,
            usage: payload.usage ?? null,
          };
        } finally {
          dispose();
        }
      } catch (error) {
        lastError = this.normalizeTransportError(error, null);
        if (!canUseFallback(lastError, this.retryableStatuses) || index === candidates.length - 1) {
          throw lastError;
        }
      }
    }
    throw lastError;
  }
}

export function byteLength(value) {
  return encoder.encode(value).byteLength;
}
