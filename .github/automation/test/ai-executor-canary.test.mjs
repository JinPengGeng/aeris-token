import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import { runLiveExecutorCanary } from '../src/ai-executor-canary.mjs';

const chatIdentity = { id: 'openai-chat-v1', protocol: 'openai-chat-completions-v1' };

function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-canary-test-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.mkdirSync(path.join(root, '.github'), { recursive: true });
  fs.writeFileSync(path.join(root, '.github', 'ai-executors.json'), JSON.stringify({
    schema_version: 1,
    executors: [
      chatIdentity,
      { id: 'openai-responses-v1', protocol: 'openai-responses-v1' },
    ],
    routes: {
      agent_analysis: 'openai-chat-v1',
      sync_conflict_resolver: 'openai-chat-v1',
      sync_conflict_reviewer: 'openai-chat-v1',
    },
  }));
  return root;
}

function environment(executorId = 'openai-chat-v1') {
  return {
    AERIS_AI_EXECUTOR_ID: executorId,
    AERIS_AI_MODEL: 'canary-model',
    AERIS_AI_BASE_URL: 'https://ai.example.test/v1',
    AERIS_AI_API_KEY: 'secret',
  };
}

function factoryWith(result) {
  return () => ({ async complete() { return result; } });
}

test('live canary reads the executor from environment and returns only trusted local metadata', async (t) => {
  const providerUsage = { total_tokens: 7, injected: 'provider-controlled' };
  const result = await runLiveExecutorCanary({
    environment: environment(),
    repoRoot: fixture(t),
    executorFactory: factoryWith({
      content: '{}',
      model: { alias: 'provider-alias', id: 'provider-model' },
      executor: chatIdentity,
      usage: providerUsage,
    }),
  });
  assert.deepEqual(result, { executor: chatIdentity, model: 'canary-model' });
  assert.equal(JSON.stringify(result).includes('provider-controlled'), false);
  assert.equal(JSON.stringify(result).includes('provider-model'), false);
});

test('live canary rejects missing, extra-field, and mismatched completion executor identities', async (t) => {
  const root = fixture(t);
  for (const executor of [
    undefined,
    { ...chatIdentity, extra: 'untrusted' },
    { id: 'openai-responses-v1', protocol: 'openai-responses-v1' },
  ]) {
    await assert.rejects(
      () => runLiveExecutorCanary({
        environment: environment(),
        repoRoot: root,
        executorFactory: factoryWith({ content: '{}', executor }),
      }),
      /completion executor identity (?:is invalid|does not match)/,
    );
  }
});

test('live canary rejects untrusted or noncanonical environment executor IDs', async (t) => {
  const root = fixture(t);
  for (const executorId of ['external', ' openai-chat-v1', 'openai-chat-v1\n']) {
    await assert.rejects(
      () => runLiveExecutorCanary({ environment: environment(executorId), repoRoot: root }),
      /executor (?:ID is invalid|is not in the trusted registry)/,
    );
  }
});
