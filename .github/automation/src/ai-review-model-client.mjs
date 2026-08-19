const MAXIMUM_RESPONSE_BYTES = 1024 * 1024;

export class AiReviewRequestError extends Error {
  constructor(message, { code = 'model_request_failed', status = null, retryable = false } = {}) {
    super(message);
    this.name = 'AiReviewRequestError';
    this.code = code;
    this.status = status;
    this.retryable = retryable;
  }
}

function requireCondition(condition, message, code = 'invalid_model_response') {
  if (!condition) throw new AiReviewRequestError(message, { code });
}

async function boundedText(response, maximumBytes) {
  const declared = Number(response.headers.get('content-length'));
  if (Number.isFinite(declared) && declared > maximumBytes) throw new AiReviewRequestError('AI response is too large', { code: 'response_too_large' });
  const reader = response.body?.getReader();
  if (!reader) return '';
  const chunks = [];
  let size = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > maximumBytes) {
      await reader.cancel();
      throw new AiReviewRequestError('AI response is too large', { code: 'response_too_large' });
    }
    chunks.push(value);
  }
  const combined = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { combined.set(chunk, offset); offset += chunk.byteLength; }
  return new TextDecoder().decode(combined);
}

function parseJson(text) {
  try { return JSON.parse(text); } catch { throw new AiReviewRequestError('AI response is not JSON', { code: 'invalid_service_json' }); }
}

function safeProviderString(value, name, { nullable = false } = {}) {
  if (nullable && (value === null || value === undefined || value === '')) return null;
  requireCondition(typeof value === 'string' && value.length > 0 && value.length <= 256 && /^[A-Za-z0-9][A-Za-z0-9._:/-]*$/.test(value), `AI ${name} is invalid`);
  return value;
}

export class AiReviewModelClient {
  constructor({ baseUrl, apiKey, timeoutMs = 300_000, maximumResponseBytes = MAXIMUM_RESPONSE_BYTES, retryableStatuses = [408, 429, 500, 502, 503, 504], fetchImpl = globalThis.fetch }) {
    const parsed = new URL(baseUrl);
    if (parsed.protocol !== 'https:' || parsed.username || parsed.password || parsed.search || parsed.hash) throw new AiReviewRequestError('AI base URL is invalid', { code: 'invalid_base_url' });
    if (typeof apiKey !== 'string' || apiKey.length === 0) throw new AiReviewRequestError('AI API key is missing', { code: 'missing_api_key' });
    requireCondition(Number.isSafeInteger(timeoutMs) && timeoutMs >= 1_000 && timeoutMs <= 600_000, 'AI timeout is invalid', 'invalid_timeout');
    this.baseUrl = parsed.toString().replace(/\/$/, '');
    this.apiKey = apiKey;
    this.timeoutMs = timeoutMs;
    this.maximumResponseBytes = maximumResponseBytes;
    this.retryableStatuses = new Set(retryableStatuses);
    this.fetchImpl = fetchImpl;
  }

  async request(candidate, messages, profile) {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(new Error('timeout')), this.timeoutMs);
    try {
      let response;
      try {
        response = await this.fetchImpl(`${this.baseUrl}${profile.request.endpoint}`, {
          method: 'POST',
          redirect: 'error',
          signal: controller.signal,
          headers: { authorization: `Bearer ${this.apiKey}`, 'content-type': 'application/json', accept: 'application/json' },
          body: JSON.stringify({
            model: candidate.id,
            messages,
            temperature: profile.request.temperature,
            max_tokens: profile.request.maximum_output_tokens,
            stream: false,
            response_format: { type: 'json_schema', json_schema: profile.response_format },
          }),
        });
      } catch (error) {
        if (controller.signal.aborted || error?.name === 'AbortError') throw new AiReviewRequestError('AI request timed out', { code: 'timeout', retryable: true });
        throw new AiReviewRequestError('AI connection failed', { code: 'connect_error', retryable: true });
      }
      const text = await boundedText(response, this.maximumResponseBytes);
      if (!response.ok) {
        parseJson(text);
        throw new AiReviewRequestError(`AI service returned HTTP ${response.status}`, { code: 'http_error', status: response.status, retryable: this.retryableStatuses.has(response.status) });
      }
      const payload = parseJson(text);
      requireCondition(Array.isArray(payload.choices) && payload.choices.length === 1, 'AI response choices are invalid');
      const choice = payload.choices[0];
      requireCondition(choice.finish_reason === 'stop', choice.finish_reason === 'length' ? 'AI response was truncated' : 'AI finish reason is invalid', choice.finish_reason === 'length' ? 'output_truncated' : 'invalid_model_response');
      requireCondition(choice.message?.refusal === undefined || choice.message.refusal === null || choice.message.refusal === '', 'AI model refused the review', 'model_refusal');
      requireCondition(typeof choice.message?.content === 'string' && choice.message.content.length > 0, 'AI response content is invalid');
      const providerModel = safeProviderString(payload.model, 'reported model');
      requireCondition(providerModel === candidate.id, 'AI provider model does not match the requested model', 'provider_model_mismatch');
      return {
        content: choice.message.content,
        requested_model: { alias: candidate.alias, id: candidate.id },
        provider_model: {
          response_id: safeProviderString(payload.id, 'response ID'),
          model: providerModel,
          system_fingerprint: safeProviderString(payload.system_fingerprint, 'system fingerprint', { nullable: true }),
        },
      };
    } catch (error) {
      if (error instanceof AiReviewRequestError) throw error;
      if (controller.signal.aborted || error?.name === 'AbortError') throw new AiReviewRequestError('AI request timed out', { code: 'timeout', retryable: true });
      throw error;
    } finally {
      clearTimeout(timeout);
    }
  }

  async complete({ candidates, messages, profile }) {
    let lastError;
    for (let index = 0; index < candidates.length; index += 1) {
      try { return await this.request(candidates[index], messages, profile); } catch (error) {
        lastError = error;
        if (!(error instanceof AiReviewRequestError) || !error.retryable || index === candidates.length - 1) throw error;
      }
    }
    throw lastError;
  }
}
