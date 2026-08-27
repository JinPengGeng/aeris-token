import path from 'node:path';
import { pathToFileURL } from 'node:url';

import { validateExecutorIdentity } from './ai-executor-contract.mjs';
import { createAiExecutorFromIdentity, loadExecutorRegistry } from './ai-executor-factory.mjs';

function fail(message) { throw new Error(`AI executor canary: ${message}`); }

export async function runLiveExecutorCanary({
  executorId = null,
  environment = process.env,
  repoRoot = environment.GITHUB_WORKSPACE ?? process.cwd(),
  executorFactory = createAiExecutorFromIdentity,
}) {
  const registry = loadExecutorRegistry(repoRoot);
  const selectedId = executorId ?? environment.AERIS_AI_EXECUTOR_ID;
  if (typeof selectedId !== 'string' || selectedId !== selectedId.trim()) fail('executor ID is invalid');
  const identity = registry.executors.find((entry) => entry.id === selectedId);
  if (!identity) fail('executor is not in the trusted registry');
  const model = environment.AERIS_AI_MODEL?.trim();
  if (!/^[A-Za-z0-9][A-Za-z0-9._:/+-]{0,255}$/.test(model ?? '')) fail('model is invalid');
  const executor = executorFactory({
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
  let completionIdentity;
  try {
    completionIdentity = validateExecutorIdentity(result?.executor, 'canary completion executor');
  } catch {
    fail('completion executor identity is invalid');
  }
  if (completionIdentity.id !== identity.id || completionIdentity.protocol !== identity.protocol) {
    fail('completion executor identity does not match the trusted registry');
  }
  if (result.content.trim() !== '{}') fail('model response did not match the canary contract');
  return { executor: identity, model };
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  if (process.argv.length !== 2) fail('command-line arguments are not accepted');
  const result = await runLiveExecutorCanary({});
  process.stdout.write(`${JSON.stringify(result)}\n`);
}
