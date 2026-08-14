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
    const controller = new AbortController();
    const requestTimeout = createTimeout(controller, this.timeoutMs, 'AI request timed out');
    const connectTimeout = createTimeout(
      controller,
      this.connectTimeoutMs,
      'AI response headers timed out',
    );
    try {
      try {
        let response;
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
        } finally {
          connectTimeout.cancel();
        }

        const text = await Promise.race([
          readBoundedText(response, this.maximumResponseBytes),
          requestTimeout.promise,
        ]);
        if (!response.ok) {
          throw new AiRequestError(`AI service returned HTTP ${response.status}`, {
            code: 'http_error',
            status: response.status,
            retryable: this.retryableStatuses.has(response.status),
          });
        }
        return parseJson(text, 'invalid_service_json');
      } catch (error) {
        if (error instanceof AiRequestError) throw error;
        if (controller.signal.aborted || error?.name === 'AbortError') {
          throw new AiRequestError('AI request timed out', { code: 'timeout', retryable: true });
        }
        throw new AiRequestError('AI connection failed', {
          code: 'connect_error',
          retryable: true,
        });
      }
    } finally {
      connectTimeout.cancel();
      requestTimeout.cancel();
    }
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

  async complete({ candidates, messages, maxTokens = 1800 }) {
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
      try {
        const payload = await this.request(this.endpoint, {
          method: 'POST',
          body: JSON.stringify({
            model: candidate.id,
            messages,
            temperature: 0.1,
            max_tokens: maxTokens,
          }),
        });
        const content = payload.choices?.[0]?.message?.content;
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
      } catch (error) {
        lastError = error;
        if (!canUseFallback(error, this.retryableStatuses) || index === candidates.length - 1) {
          throw error;
        }
      }
    }
    throw lastError;
  }
}

export function byteLength(value) {
  return encoder.encode(value).byteLength;
}
