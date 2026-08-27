import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  executorDescriptorForRoute,
  executorForRoute,
  validateExecutorRegistry,
  validateWorkspaceCandidateExecutor,
} from '../src/ai-executor-contract.mjs';
import { createAiExecutor, loadExecutorRegistry } from '../src/ai-executor-factory.mjs';

const registry = {
  schema_version: 1,
  executors: [
    { id: 'openai-chat-v1', kind: 'completion', protocol: 'openai-chat-completions-v1' },
    { id: 'openai-responses-v1', kind: 'completion', protocol: 'openai-responses-v1' },
    {
      id: 'codex-action-v1', kind: 'workspace_candidate', protocol: 'aeris-workspace-candidate-v1',
      action_sha: '52fe01ec70a42f454c9d2ebd47598f9fd6893d56', tool_version: '0.148.0',
    },
  ],
  routes: {
    agent_analysis: 'openai-chat-v1',
    sync_conflict_resolver: 'openai-chat-v1',
    sync_conflict_reviewer: 'openai-chat-v1',
    candidate: 'codex-action-v1',
  },
};

test('registry is a static exact allowlist and production routes remain chat', () => {
  const normalized = validateExecutorRegistry(registry);
  assert.deepEqual(executorForRoute(normalized, 'agent_analysis'), { id: 'openai-chat-v1', protocol: 'openai-chat-completions-v1' });
  assert.deepEqual(executorDescriptorForRoute(normalized, 'candidate'), registry.executors[2]);
  assert.deepEqual(validateWorkspaceCandidateExecutor(registry.executors[2]), registry.executors[2]);
  assert.throws(() => validateExecutorRegistry({ ...registry, routes: { ...registry.routes, arbitrary: 'openai-chat-v1' } }), /invalid fields/);
  assert.throws(() => validateExecutorRegistry({ ...registry, routes: { ...registry.routes, agent_analysis: 'external' } }), /trusted executor/);
  assert.throws(() => validateExecutorRegistry({ ...registry, routes: { ...registry.routes, candidate: 'openai-chat-v1' } }), /wrong executor kind/);
  assert.throws(() => validateWorkspaceCandidateExecutor(registry.executors[0]), /kind is invalid/);
});

test('factory returns a trusted chat adapter and does not select from environment', async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-executor-registry-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.mkdirSync(path.join(root, '.github'), { recursive: true });
  fs.writeFileSync(path.join(root, '.github', 'ai-executors.json'), JSON.stringify(registry));
  assert.deepEqual(loadExecutorRegistry(root), validateExecutorRegistry(registry));
  const calls = [];
  const executor = createAiExecutor({
    repoRoot: root, route: 'agent_analysis', baseUrl: 'https://ai.example.test/v1', apiKey: 'key',
    retryableStatuses: [429], fetchImpl: async (url, init) => {
      calls.push({ url, init });
      if (url.endsWith('/models')) return new Response(JSON.stringify({ data: [{ id: 'model' }] }), { status: 200 });
      return new Response(JSON.stringify({ choices: [{ finish_reason: 'stop', message: { content: '{}' } }] }), { status: 200 });
    },
  });
  const result = await executor.complete({ candidates: [{ alias: 'role', id: 'model' }], messages: [] });
  assert.deepEqual(result.executor, { id: 'openai-chat-v1', protocol: 'openai-chat-completions-v1' });
  assert.equal(calls[1].url.endsWith('/chat/completions'), true);
});
