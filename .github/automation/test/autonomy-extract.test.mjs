import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import { buildCandidateArtifact, CandidateExtractionError } from '../src/autonomy-extract.mjs';

function command(root, args) {
  return execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim();
}

function repository() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-autonomy-extract-'));
  command(root, ['init', '--initial-branch=main']);
  command(root, ['config', 'user.name', 'Test']);
  command(root, ['config', 'user.email', 'test@example.invalid']);
  fs.mkdirSync(path.join(root, 'docs', 'automation-canary'), { recursive: true });
  fs.writeFileSync(path.join(root, 'README.md'), 'base\n');
  command(root, ['add', '.']);
  command(root, ['commit', '-m', 'base']);
  return root;
}

function metadata(root, overrides = {}) {
  return {
    repository: 'JinPengGeng/aeris-token',
    repository_id: 1310462380,
    issue_number: 123,
    base_ref: 'refs/heads/main',
    base_sha: command(root, ['rev-parse', 'HEAD']),
    trigger_run_id: '9001',
    trigger_run_attempt: 1,
    ...overrides,
  };
}

test('extracts tracked and untracked text changes into a verified artifact', () => {
  const root = repository();
  const output = path.join(root, '.candidate-output');
  fs.appendFileSync(path.join(root, 'README.md'), 'changed\n');
  fs.writeFileSync(path.join(root, 'docs', 'automation-canary', 'new.md'), 'new\n');

  const result = buildCandidateArtifact({
    repositoryRoot: root,
    outputDirectory: output,
    metadata: metadata(root),
    now: new Date('2026-08-20T00:00:00Z'),
  });

  assert.deepEqual(result.paths, ['README.md', 'docs/automation-canary/new.md']);
  assert.equal(result.manifest.created_at, '2026-08-20T00:00:00.000Z');
  assert.equal(fs.existsSync(result.patchPath), true);
  assert.equal(fs.existsSync(result.manifestPath), true);
});

test('rejects an unchanged workspace', () => {
  const root = repository();
  assert.throws(
    () => buildCandidateArtifact({
      repositoryRoot: root,
      outputDirectory: path.join(root, '.candidate-output'),
      metadata: metadata(root),
    }),
    (error) => error instanceof CandidateExtractionError && /no candidate changes/.test(error.message),
  );
});

test('rejects a commit made by the Agent', () => {
  const root = repository();
  const expected = metadata(root);
  fs.appendFileSync(path.join(root, 'README.md'), 'committed\n');
  command(root, ['add', '.']);
  command(root, ['commit', '-m', 'agent commit']);
  assert.throws(
    () => buildCandidateArtifact({
      repositoryRoot: root,
      outputDirectory: path.join(root, '.candidate-output'),
      metadata: expected,
    }),
    /HEAD changed/,
  );
});

test('rejects governed changes during extraction', () => {
  const root = repository();
  fs.mkdirSync(path.join(root, '.github'), { recursive: true });
  fs.writeFileSync(path.join(root, '.github', 'unsafe.yml'), 'unsafe: true\n');
  assert.throws(
    () => buildCandidateArtifact({
      repositoryRoot: root,
      outputDirectory: path.join(root, '.candidate-output'),
      metadata: metadata(root),
    }),
    /governed/,
  );
});
