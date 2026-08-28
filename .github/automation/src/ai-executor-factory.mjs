import fs from 'node:fs';
import path from 'node:path';

import { executorForRoute, validateExecutorRegistry } from './ai-executor-contract.mjs';
import { createOpenAiChatExecutor } from './ai-executors/openai-chat-v1.mjs';
import { createOpenAiResponsesExecutor } from './ai-executors/openai-responses-v1.mjs';

const EXECUTOR_PATH = '.github/ai-executors.json';

export function loadExecutorRegistry(repoRoot) {
  const filePath = path.resolve(repoRoot, EXECUTOR_PATH);
  const relative = path.relative(path.resolve(repoRoot), filePath);
  if (relative === '..' || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    throw new Error('AI executor registry path escapes the trusted checkout');
  }
  const stat = fs.lstatSync(filePath);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 65_536) throw new Error('AI executor registry is not a bounded regular file');
  let parsed;
  try { parsed = JSON.parse(fs.readFileSync(filePath, 'utf8')); } catch { throw new Error('AI executor registry is invalid JSON'); }
  return validateExecutorRegistry(parsed);
}

export function createAiExecutorFromIdentity({ identity, ...options }) {
  if (identity.id === 'openai-chat-v1' && identity.protocol === 'openai-chat-completions-v1') {
    return createOpenAiChatExecutor(options, identity);
  }
  if (identity.id === 'openai-responses-v1' && identity.protocol === 'openai-responses-v1') {
    return createOpenAiResponsesExecutor(options, identity);
  }
  throw new Error(`AI executor ${identity.id} has no trusted factory implementation`);
}

export function createAiExecutor({ repoRoot, route, ...options }) {
  return createAiExecutorFromIdentity({
    identity: executorForRoute(loadExecutorRegistry(repoRoot), route),
    ...options,
  });
}

export function trustedExecutorForRoute({ repoRoot, route }) {
  return executorForRoute(loadExecutorRegistry(repoRoot), route);
}
