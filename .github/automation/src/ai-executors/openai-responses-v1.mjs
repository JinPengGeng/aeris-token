import { AiRequestError, OpenAICompatibleClient } from '../openai-client.mjs';

function canUseFallback(error, retryableStatuses) {
  return error instanceof AiRequestError && error.retryable &&
    (error.code === 'connect_error' || error.code === 'timeout' ||
      (error.code === 'http_error' && retryableStatuses.has(error.status)));
}

function extractOutput(payload) {
  if (payload?.status !== 'completed') {
    throw new AiRequestError('AI Responses request did not complete', {
      code: payload?.incomplete_details ? 'output_truncated' : 'invalid_responses_response',
    });
  }
  if (!Array.isArray(payload.output)) {
    throw new AiRequestError('AI Responses output has an invalid shape', { code: 'invalid_responses_response' });
  }
  let content = '';
  for (const item of payload.output) {
    if (item?.type === 'refusal') {
      throw new AiRequestError('AI Responses request was refused by the model', { code: 'model_refusal' });
    }
    if (item?.type !== 'message') continue;
    if (!Array.isArray(item.content)) {
      throw new AiRequestError('AI Responses message content has an invalid shape', { code: 'invalid_responses_response' });
    }
    for (const part of item.content) {
      if (part?.type === 'refusal') {
        throw new AiRequestError('AI Responses request was refused by the model', { code: 'model_refusal' });
      }
      if (part?.type === 'output_text') {
        if (typeof part.text !== 'string') {
          throw new AiRequestError('AI Responses output text is invalid', { code: 'invalid_responses_response' });
        }
        content += part.text;
      }
    }
  }
  if (content.length === 0) {
    throw new AiRequestError('AI Responses output does not contain text', { code: 'invalid_responses_response' });
  }
  return content;
}

function responseFormat(responseFormat) {
  if (responseFormat === undefined) return {};
  if (responseFormat?.type !== 'json_schema' || !responseFormat.json_schema || typeof responseFormat.json_schema !== 'object') {
    throw new AiRequestError('AI response format is not supported by Responses', { code: 'invalid_response_format' });
  }
  const { name, strict, schema } = responseFormat.json_schema;
  return { text: { format: { type: 'json_schema', name, strict, schema } } };
}

export function createOpenAiResponsesExecutor(options, identity) {
  const client = new OpenAICompatibleClient({ ...options, endpoint: '/responses' });
  const retryableStatuses = new Set(options.retryableStatuses);
  const deadline = client.deadlineAtMs;
  const ensureDeadline = () => {
    if (deadline !== null && Date.now() >= deadline) {
      throw new AiRequestError('AI completion deadline exceeded', { code: 'timeout' });
    }
  };
  return Object.freeze({
    identity,
    async complete({ candidates, messages, maxTokens = 1800, responseFormat: format = undefined }) {
      const available = await client.listModelIds({ timeoutMs: client.remainingRequestTimeoutMs(deadline) });
      for (const candidate of candidates) {
        if (!available.has(candidate.id)) {
          throw new AiRequestError(`configured model is not present in /models: ${candidate.alias}`, { code: 'model_not_allowed' });
        }
      }
      let lastError;
      for (let index = 0; index < candidates.length; index += 1) {
        const candidate = candidates[index];
        const startedAt = Date.now();
        try {
          const payload = await client.request('/responses', {
            method: 'POST',
            body: JSON.stringify({
              model: candidate.id,
              input: messages.map(({ role, content }) => ({ role, content })),
              temperature: 0.1,
              max_output_tokens: maxTokens,
              ...responseFormat(format),
            }),
          }, client.remainingRequestTimeoutMs(deadline));
          ensureDeadline();
          return { content: extractOutput(payload), model: candidate, durationMs: Date.now() - startedAt, usage: payload.usage ?? null, executor: identity };
        } catch (error) {
          lastError = client.normalizeTransportError(error, null);
          ensureDeadline();
          if (!canUseFallback(lastError, retryableStatuses) || index === candidates.length - 1) throw lastError;
        }
      }
      throw lastError;
    },
  });
}
