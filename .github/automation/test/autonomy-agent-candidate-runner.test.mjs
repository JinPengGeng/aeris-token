import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import { AgentCandidateRunnerError, validatePatchApplies } from '../src/autonomy-agent-candidate-runner.mjs';

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

test('validates a patch against HEAD with an isolated index', () => {
  const root = repository();
  const patchPath = path.join(root, 'candidate.patch');
  fs.writeFileSync(
    patchPath,
    'diff --git a/README.md b/README.md\nindex df967b9..3bd1f0e 100644\n--- a/README.md\n+++ b/README.md\n@@ -1 +1,2 @@\n base\n+candidate\n',
  );
  validatePatchApplies({ repositoryRoot: root, patchPath });
  assert.equal(git(root, ['status', '--porcelain']), '?? candidate.patch');
});

test('rejects a patch that does not apply to the bound base', () => {
  const root = repository();
  const patchPath = path.join(root, 'candidate.patch');
  fs.writeFileSync(
    patchPath,
    'diff --git a/README.md b/README.md\nindex df967b9..3bd1f0e 100644\n--- a/README.md\n+++ b/README.md\n@@ -1 +1,2 @@\n wrong-base\n+candidate\n',
  );
  assert.throws(
    () => validatePatchApplies({ repositoryRoot: root, patchPath }),
    (error) => error instanceof AgentCandidateRunnerError && /git apply failed/.test(error.message),
  );
});
