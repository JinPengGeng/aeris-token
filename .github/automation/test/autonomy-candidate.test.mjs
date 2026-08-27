import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import test from 'node:test';

import {
  CANDIDATE_SCHEMA_VERSION,
  CandidateValidationError,
  MAX_CANDIDATE_FILE_TEXT_BYTES,
  MAX_CANDIDATE_PATCH_BYTES,
  parseUnifiedDiff,
  validateCandidateArtifact,
  validateCandidateManifest,
} from '../src/autonomy-candidate.mjs';

const candidateExecutor = Object.freeze({
  id: 'codex-action-v1',
  protocol: 'aeris-workspace-candidate-v1',
  kind: 'workspace_candidate',
  action_sha: '52fe01ec70a42f454c9d2ebd47598f9fd6893d56',
  tool_version: '0.148.0',
});

const expected = Object.freeze({
  repository: 'JinPengGeng/aeris-token',
  repository_id: 1310462380,
  task_id: 'issue:123',
  issue_number: 123,
  base_ref: 'refs/heads/main',
  base_sha: 'a'.repeat(40),
  trigger_run_id: '456',
  trigger_run_attempt: 1,
  executor: candidateExecutor,
});

function patch(path = 'docs/automation-canary/example.md', body = '+new content') {
  return `diff --git a/${path} b/${path}\nindex 1111111..2222222 100644\n--- a/${path}\n+++ b/${path}\n@@ -0,0 +1 @@\n${body}\n`;
}

function manifest(sourcePatch, overrides = {}) {
  const bytes = Buffer.byteLength(sourcePatch, 'utf8');
  return {
    schema_version: CANDIDATE_SCHEMA_VERSION,
    ...expected,
    patch_sha256: createHash('sha256').update(sourcePatch).digest('hex'),
    patch_bytes: bytes,
    created_at: '2026-08-20T12:34:56.123Z',
    ...overrides,
  };
}

function assertRejected(action, message) {
  assert.throws(action, (error) => error instanceof CandidateValidationError && new RegExp(message).test(error.message));
}

test('candidate artifact validates a bounded ordinary patch and returns frozen normalized data', () => {
  const sourcePatch = patch();
  const candidate = validateCandidateArtifact({ manifest: manifest(sourcePatch), patch: sourcePatch, expected });
  assert.deepEqual(candidate.paths, ['docs/automation-canary/example.md']);
  assert.ok(Object.isFrozen(candidate));
  assert.ok(Object.isFrozen(candidate.manifest));
  assert.ok(Object.isFrozen(candidate.paths));
  assert.notEqual(candidate.manifest, manifest(sourcePatch));
});

test('manifest requires exact schema keys, valid fields, and the trusted execution snapshot', () => {
  const sourcePatch = patch();
  assertRejected(() => validateCandidateManifest({ ...manifest(sourcePatch), unexpected: true }, expected), 'unexpected keys');
  assertRejected(() => validateCandidateManifest(manifest(sourcePatch, { schema_version: 1 }), expected), 'schema_version');
  assertRejected(() => validateCandidateManifest(manifest(sourcePatch, { task_id: 'issue:124' }), expected), 'task_id');
  assertRejected(() => validateCandidateManifest(manifest(sourcePatch, { base_sha: 'A'.repeat(40) }), expected), 'base_sha');
  assertRejected(() => validateCandidateManifest(manifest(sourcePatch, { created_at: '2026-08-20T12:34:56+08:00' }), expected), 'created_at');
  assertRejected(() => validateCandidateManifest(manifest(sourcePatch), { ...expected, trigger_run_attempt: 2 }), 'trigger_run_attempt');
  assertRejected(() => validateCandidateManifest(manifest(sourcePatch), {
    ...expected,
    executor: { ...candidateExecutor, tool_version: '0.148.1' },
  }), 'executor');
  assertRejected(() => validateCandidateManifest(manifest(sourcePatch, {
    executor: { ...candidateExecutor, kind: 'completion' },
  }), expected), 'executor');
});

test('artifact verifies patch byte length and SHA-256 digest', () => {
  const sourcePatch = patch();
  assertRejected(() => validateCandidateArtifact(manifest(sourcePatch, { patch_bytes: 1 }), sourcePatch, expected), 'patch_bytes');
  assertRejected(() => validateCandidateArtifact(manifest(sourcePatch, { patch_sha256: '0'.repeat(64) }), sourcePatch, expected), 'patch_sha256');
  assertRejected(() => validateCandidateArtifact(manifest(sourcePatch), Buffer.alloc(MAX_CANDIDATE_PATCH_BYTES + 1), expected), 'maximum size');
});

test('unified diff rejects unsafe, governed, duplicate, and case-fold-conflicting paths', () => {
  for (const unsafePath of ['/absolute.md', '../escape.md', 'docs\\escape.md', 'docs/\0escape.md', '.github/workflows/write.yml', 'CODEOWNERS', '.gitmodules']) {
    assertRejected(() => parseUnifiedDiff(patch(unsafePath)), 'path|governed');
  }
  const duplicate = `${patch('docs/a.md')}${patch('docs/a.md')}`;
  assertRejected(() => parseUnifiedDiff(duplicate), 'duplicate paths');
  const conflict = `${patch('docs/A.md')}${patch('docs/a.md')}`;
  assertRejected(() => parseUnifiedDiff(conflict), 'case-fold conflict');
});

test('unified diff rejects binary, quoted, malformed, unsafe-mode, and oversized file patches', () => {
  assertRejected(() => parseUnifiedDiff('diff --git "a/docs/a b.md" "b/docs/a b.md"\n'), 'invalid or quoted');
  assertRejected(() => parseUnifiedDiff('diff --git a/docs/a.md b/docs/a.md\nGIT binary patch\n'), 'binary');
  assertRejected(() => parseUnifiedDiff('not a diff\n'), 'unified diff header');
  assertRejected(() => parseUnifiedDiff(patch('docs/a.md').replace('+++ b/docs/a.md', '+++ b/docs/other.md')), 'marker does not match');
  for (const mode of ['100755', '120000', '160000']) {
    const sourcePatch = patch('docs/a.md').replace('100644', mode);
    assertRejected(() => parseUnifiedDiff(sourcePatch), 'non-regular or executable');
  }
  const sourcePatch = patch('docs/large.md', `+${'a'.repeat(MAX_CANDIDATE_FILE_TEXT_BYTES)}`);
  assertRejected(() => parseUnifiedDiff(sourcePatch), 'file text exceeds');
});

test('unified diff rejects more than one hundred changed files and accepts rename paths', () => {
  const tooMany = Array.from({ length: 101 }, (_, index) => patch(`docs/${index}.md`)).join('');
  assertRejected(() => parseUnifiedDiff(tooMany), 'file count');
  const rename = [
    'diff --git a/docs/old.md b/docs/new.md',
    'similarity index 100%',
    'rename from docs/old.md',
    'rename to docs/new.md',
    '',
  ].join('\n');
  assert.deepEqual(parseUnifiedDiff(rename), ['docs/old.md', 'docs/new.md']);
});
