import { OpenAICompatibleClient } from '../openai-client.mjs';

export function createOpenAiChatExecutor(options, identity) {
  const client = new OpenAICompatibleClient({ ...options, endpoint: '/chat/completions' });
  return Object.freeze({
    identity,
    async complete(request) {
      return { ...(await client.complete(request)), executor: identity };
    },
  });
}
