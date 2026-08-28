import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import { buildCandidateArtifact, CandidateExtractionError } from '../src/autonomy-extract.mjs';

const candidateExecutor = Object.freeze({
  id: 'codex-action-v1',
  protocol: 'aeris-workspace-candidate-v1',
  kind: 'workspace_candidate',
  action_sha: '52fe01ec70a42f454c9d2ebd47598f9fd6893d56',
  tool_version: '0.148.0',
});

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
    executor: candidateExecutor,
    ...overrides,
  };
}

function executableCommand(helper, marker) {
  const quote = (value) => `"${value.replaceAll('\\', '/').replaceAll('"', '\\"')}"`;
  return `${quote(process.execPath)} ${quote(helper)} ${quote(marker)}`;
}

function restoreEnvironment(snapshot) {
  for (const [name, value] of Object.entries(snapshot)) {
    if (value === undefined) delete process.env[name];
    else process.env[name] = value;
  }
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
  assert.equal(result.manifest.schema_version, 2);
  assert.deepEqual(result.manifest.executor, candidateExecutor);
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

test('rejects an executor descriptor that is not a trusted workspace candidate', () => {
  const root = repository();
  fs.appendFileSync(path.join(root, 'README.md'), 'candidate\n');
  assert.throws(
    () => buildCandidateArtifact({
      repositoryRoot: root,
      outputDirectory: path.join(root, '.candidate-output'),
      metadata: metadata(root, { executor: { id: 'openai-chat-v1', protocol: 'openai-chat-completions-v1', kind: 'completion' } }),
    }),
    (error) => error instanceof CandidateExtractionError && /candidate executor/.test(error.message),
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

test('resolves HEAD from packed refs without trusting unrelated refs', () => {
  const root = repository();
  const baseSha = command(root, ['rev-parse', 'HEAD']);
  command(root, ['update-ref', 'refs/remotes/origin/main', baseSha]);
  command(root, ['pack-refs', '--all']);
  fs.appendFileSync(path.join(root, 'README.md'), 'candidate\n');
  const result = buildCandidateArtifact({
    repositoryRoot: root,
    outputDirectory: fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-packed-output-')),
    metadata: metadata(root, { base_sha: baseSha }),
  });
  assert.deepEqual(result.paths, ['README.md']);
});

test('ignores Agent-controlled Git commands and leaves the real index untouched', () => {
  const root = repository();
  fs.writeFileSync(path.join(root, '.gitattributes'), 'README.md diff=hostile filter=hostile\n');
  command(root, ['add', '.gitattributes']);
  command(root, ['commit', '-m', 'attributes']);
  const expectedMetadata = metadata(root);
  fs.appendFileSync(path.join(root, 'README.md'), 'candidate\n');

  const helper = path.join(root, '.git', 'hostile-git-command.cjs');
  fs.writeFileSync(
    helper,
    "const fs = require('node:fs');\nfs.writeFileSync(process.argv[2], 'executed');\nprocess.stdin.pipe(process.stdout);\n",
  );
  const markers = {
    textconv: path.join(root, '.git', 'textconv-executed'),
    filter: path.join(root, '.git', 'filter-executed'),
    fsmonitor: path.join(root, '.git', 'fsmonitor-executed'),
    environment: path.join(root, '.git', 'environment-executed'),
  };
  command(root, ['config', 'diff.hostile.textconv', executableCommand(helper, markers.textconv)]);
  command(root, ['config', 'filter.hostile.clean', executableCommand(helper, markers.filter)]);
  command(root, ['config', 'filter.hostile.smudge', executableCommand(helper, markers.filter)]);
  command(root, ['config', 'core.fsmonitor', executableCommand(helper, markers.fsmonitor)]);

  const environmentNames = [
    'GIT_CONFIG_COUNT',
    'GIT_CONFIG_KEY_0',
    'GIT_CONFIG_VALUE_0',
    'GIT_EXTERNAL_DIFF',
    'GIT_INDEX_FILE',
  ];
  const environmentSnapshot = Object.fromEntries(environmentNames.map((name) => [name, process.env[name]]));
  process.env.GIT_CONFIG_COUNT = '1';
  process.env.GIT_CONFIG_KEY_0 = 'diff.inherited.textconv';
  process.env.GIT_CONFIG_VALUE_0 = executableCommand(helper, markers.environment);
  process.env.GIT_EXTERNAL_DIFF = executableCommand(helper, markers.environment);
  process.env.GIT_INDEX_FILE = path.join(root, '.git', 'hostile-index');

  const hostileOutput = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-hostile-output-'));
  const cleanOutput = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-clean-output-'));
  let hostilePatch;
  try {
    const result = buildCandidateArtifact({
      repositoryRoot: root,
      outputDirectory: hostileOutput,
      metadata: expectedMetadata,
    });
    hostilePatch = fs.readFileSync(result.patchPath);
  } finally {
    restoreEnvironment(environmentSnapshot);
  }

  for (const marker of Object.values(markers)) assert.equal(fs.existsSync(marker), false);
  assert.equal(command(root, ['diff', '--cached', '--name-only']), '');
  assert.match(command(root, ['status', '--porcelain']), /^M README\.md$/m);

  command(root, ['config', '--unset-all', 'diff.hostile.textconv']);
  command(root, ['config', '--unset-all', 'filter.hostile.clean']);
  command(root, ['config', '--unset-all', 'filter.hostile.smudge']);
  command(root, ['config', '--unset-all', 'core.fsmonitor']);
  const cleanResult = buildCandidateArtifact({
    repositoryRoot: root,
    outputDirectory: cleanOutput,
    metadata: expectedMetadata,
  });
  assert.deepEqual(hostilePatch, fs.readFileSync(cleanResult.patchPath));
});
