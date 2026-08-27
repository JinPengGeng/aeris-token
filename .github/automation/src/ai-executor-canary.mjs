import path from 'node:path';
import { pathToFileURL } from 'node:url';

import { createAiExecutorFromIdentity, loadExecutorRegistry } from './ai-executor-factory.mjs';

function fail(message) { throw new Error(`AI executor canary: ${message}`); }

export async function runLiveExecutorCanary({ executorId, environment = process.env, repoRoot = environment.GITHUB_WORKSPACE ?? process.cwd() }) {
  const registry = loadExecutorRegistry(repoRoot);
  const identity = registry.executors.find((entry) => entry.id === executorId);
  if (!identity) fail('executor is not in the trusted registry');
  const model = environment.AERIS_AI_MODEL?.trim();
  if (!/^[A-Za-z0-9][A-Za-z0-9._:/+-]{0,255}$/.test(model ?? '')) fail('model is invalid');
  const executor = createAiExecutorFromIdentity({
    identity,
    baseUrl: environment.AERIS_AI_BASE_URL,
    apiKey: environment.AERIS_AI_API_KEY,
    retryableStatuses: [408, 429, 500, 502, 503, 504],
    timeoutMs: 60_000,
    connectTimeoutMs: 30_000,
    maximumResponseBytes: 65_536,
  });
  const result = await executor.complete({
    candidates: [{ alias: 'canary', id: model }],
    messages: [{ role: 'user', content: 'Return exactly {}.' }],
    maxTokens: 16,
  });
  if (result.content.trim() !== '{}') fail('model response did not match the canary contract');
  return { executor: result.executor, model: result.model.id, usage: result.usage ?? null };
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  const result = await runLiveExecutorCanary({ executorId: process.argv[2] });
  process.stdout.write(`${JSON.stringify(result)}\n`);
}
