import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

import {
  AgentCandidateRunnerError,
  sealedCandidateExecutor,
  validatePatchApplies,
} from '../src/autonomy-agent-candidate-runner.mjs';

function git(root, args) {
  return execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim();
}

function repository() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-agent-candidate-'));
  git(root, ['init', '--initial-branch=main']);
  git(root, ['config', 'user.name', 'Test']);
  git(root, ['config', 'user.email', 'test@example.invalid']);
  fs.writeFileSync(path.join(root, 'README.md'), 'base\n');
  git(root, ['add', 'README.md']);
  git(root, ['commit', '-m', 'base']);
  return root;
}

test('sealed runtime resolves the workspace executor descriptor from the trusted registry', () => {
  assert.deepEqual(sealedCandidateExecutor(), {
    id: 'codex-action-v1',
    protocol: 'aeris-workspace-candidate-v1',
    kind: 'workspace_candidate',
    action_sha: '52fe01ec70a42f454c9d2ebd47598f9fd6893d56',
    tool_version: '0.148.0',
  });
});

test('sealed runtime rejects a missing flat registry instead of falling back outside the artifact', async (t) => {
  const runtime = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-sealed-runtime-'));
  t.after(() => fs.rmSync(runtime, { recursive: true, force: true }));
  const sourceDirectory = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'src');
  for (const file of [
    'autonomy-agent-candidate-runner.mjs',
    'autonomy-candidate.mjs',
    'autonomy-extract.mjs',
    'autonomy-safe-git.mjs',
    'ai-executor-contract.mjs',
  ]) {
    fs.copyFileSync(path.join(sourceDirectory, file), path.join(runtime, file));
  }
  const module = await import(`${pathToFileURL(path.join(runtime, 'autonomy-agent-candidate-runner.mjs')).href}?test=missing-registry`);
  assert.throws(
    () => module.sealedCandidateExecutor(),
    (error) => error instanceof module.AgentCandidateRunnerError && /registry is unavailable/.test(error.message),
  );
});

test('validates a patch against HEAD with an isolated index', () => {
  const root = repository();
  const baseSha = git(root, ['rev-parse', 'HEAD']);
  const patchPath = path.join(root, 'candidate.patch');
  fs.writeFileSync(
    patchPath,
    'diff --git a/README.md b/README.md\nindex df967b9..3bd1f0e 100644\n--- a/README.md\n+++ b/README.md\n@@ -1 +1,2 @@\n base\n+candidate\n',
  );
  validatePatchApplies({ repositoryRoot: root, baseSha, patchPath });
  assert.equal(git(root, ['status', '--porcelain']), '?? candidate.patch');
});

test('rejects a patch that does not apply to the bound base', () => {
  const root = repository();
  const baseSha = git(root, ['rev-parse', 'HEAD']);
  const patchPath = path.join(root, 'candidate.patch');
  fs.writeFileSync(
    patchPath,
    'diff --git a/README.md b/README.md\nindex df967b9..3bd1f0e 100644\n--- a/README.md\n+++ b/README.md\n@@ -1 +1,2 @@\n wrong-base\n+candidate\n',
  );
  assert.throws(
    () => validatePatchApplies({ repositoryRoot: root, baseSha, patchPath }),
    (error) => error instanceof AgentCandidateRunnerError && /git apply failed/.test(error.message),
  );
});

test('isolated validation never executes repository hooks or local Git configuration', () => {
  const root = repository();
  const baseSha = git(root, ['rev-parse', 'HEAD']);
  const patchPath = path.join(root, 'candidate.patch');
  fs.writeFileSync(
    patchPath,
    'diff --git a/README.md b/README.md\nindex df967b9..3bd1f0e 100644\n--- a/README.md\n+++ b/README.md\n@@ -1 +1,2 @@\n base\n+candidate\n',
  );

  const marker = path.join(root, '.git', 'hook-executed').replaceAll('\\', '/');
  const hook = path.join(root, '.git', 'hooks', 'post-index-change');
  fs.writeFileSync(hook, `#!/bin/sh\nprintf executed >'${marker}'\n`, { mode: 0o755 });
  fs.chmodSync(hook, 0o755);
  git(root, ['config', 'core.hooksPath', path.join(root, '.git', 'hooks')]);

  const inheritedIndex = process.env.GIT_INDEX_FILE;
  process.env.GIT_INDEX_FILE = path.join(root, '.git', 'attacker-index');
  try {
    validatePatchApplies({ repositoryRoot: root, baseSha, patchPath });
  } finally {
    if (inheritedIndex === undefined) delete process.env.GIT_INDEX_FILE;
    else process.env.GIT_INDEX_FILE = inheritedIndex;
  }
  assert.equal(fs.existsSync(marker), false);
  assert.equal(fs.existsSync(path.join(root, '.git', 'attacker-index')), false);
});
