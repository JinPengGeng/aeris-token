import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import { validateCandidateArtifact } from './autonomy-candidate.mjs';

export class CandidateExtractionError extends Error {
  constructor(message) {
    super(message);
    this.name = 'CandidateExtractionError';
  }
}

function reject(message) {
  throw new CandidateExtractionError(message);
}

function requiredString(value, name, pattern = null) {
  if (typeof value !== 'string' || value.length === 0 || /[\u0000-\u001f\u007f]/.test(value)) {
    reject(`${name} is invalid`);
  }
  if (pattern && !pattern.test(value)) reject(`${name} format is invalid`);
  return value;
}

function positiveInteger(value, name) {
  const parsed = typeof value === 'string' && /^[1-9][0-9]*$/.test(value) ? Number(value) : value;
  if (!Number.isSafeInteger(parsed) || parsed <= 0) reject(`${name} must be a positive integer`);
  return parsed;
}

function git(repositoryRoot, args, { encoding = 'utf8' } = {}) {
  try {
    return execFileSync('git', args, {
      cwd: repositoryRoot,
      encoding,
      env: {
        ...process.env,
        GIT_CONFIG_NOSYSTEM: '1',
        GIT_TERMINAL_PROMPT: '0',
      },
      maxBuffer: 2 * 1024 * 1024,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
  } catch (error) {
    const stderr = typeof error?.stderr === 'string' ? error.stderr.trim() : '';
    reject(`git ${args[0]} failed${stderr ? `: ${stderr}` : ''}`);
  }
}

function normalizeMetadata(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) reject('candidate metadata is invalid');
  const repository = requiredString(value.repository, 'repository', /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/);
  const repositoryId = positiveInteger(value.repository_id, 'repository_id');
  const issueNumber = positiveInteger(value.issue_number, 'issue_number');
  const baseRef = requiredString(value.base_ref, 'base_ref', /^refs\/heads\/[A-Za-z0-9._/-]+$/);
  const baseSha = requiredString(value.base_sha, 'base_sha', /^[0-9a-f]{40}$/);
  const triggerRunId = requiredString(value.trigger_run_id, 'trigger_run_id', /^(?:0|[1-9][0-9]*)$/);
  const triggerRunAttempt = positiveInteger(value.trigger_run_attempt, 'trigger_run_attempt');
  return Object.freeze({
    repository,
    repository_id: repositoryId,
    task_id: `issue:${issueNumber}`,
    issue_number: issueNumber,
    base_ref: baseRef,
    base_sha: baseSha,
    trigger_run_id: triggerRunId,
    trigger_run_attempt: triggerRunAttempt,
  });
}

export function buildCandidateArtifact({ repositoryRoot, outputDirectory, metadata, now = new Date() }) {
  const normalized = normalizeMetadata(metadata);
  const root = path.resolve(requiredString(repositoryRoot, 'repositoryRoot'));
  const output = path.resolve(requiredString(outputDirectory, 'outputDirectory'));
  const actualRoot = fs.realpathSync.native(path.resolve(git(root, ['rev-parse', '--show-toplevel']).trim()));
  const expectedRoot = fs.realpathSync.native(root);
  if (actualRoot.toLocaleLowerCase('en-US') !== expectedRoot.toLocaleLowerCase('en-US')) {
    reject('repositoryRoot is not the Git worktree root');
  }
  const head = git(root, ['rev-parse', 'HEAD']).trim();
  if (head !== normalized.base_sha) reject('repository HEAD changed during Agent execution');

  // Intent-to-add makes untracked text files visible to git diff without
  // staging their contents or executing repository code.
  git(root, ['add', '--intent-to-add', '--', '.']);
  const patch = git(
    root,
    [
      'diff',
      '--binary',
      '--full-index',
      '--no-ext-diff',
      '--src-prefix=a/',
      '--dst-prefix=b/',
      'HEAD',
      '--',
    ],
    { encoding: 'buffer' },
  );
  if (!Buffer.isBuffer(patch) || patch.length === 0) reject('Agent produced no candidate changes');

  const createdAt = now.toISOString();
  const manifest = {
    schema_version: 1,
    ...normalized,
    patch_sha256: createHash('sha256').update(patch).digest('hex'),
    patch_bytes: patch.length,
    created_at: createdAt,
  };
  const verified = validateCandidateArtifact({ manifest, patch, expected: normalized });

  fs.mkdirSync(output, { recursive: true, mode: 0o700 });
  const patchPath = path.join(output, 'candidate.patch');
  const manifestPath = path.join(output, 'candidate-manifest.json');
  fs.writeFileSync(patchPath, patch, { mode: 0o600 });
  fs.writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, { mode: 0o600 });
  return Object.freeze({ patchPath, manifestPath, ...verified });
}

function environmentMetadata(environment) {
  return {
    repository: environment.GITHUB_REPOSITORY,
    repository_id: environment.GITHUB_REPOSITORY_ID,
    issue_number: environment.AERIS_ISSUE_NUMBER,
    base_ref: environment.AERIS_BASE_REF,
    base_sha: environment.AERIS_BASE_SHA,
    trigger_run_id: environment.GITHUB_RUN_ID,
    trigger_run_attempt: environment.GITHUB_RUN_ATTEMPT,
  };
}

export function runCandidateExtraction(environment = process.env) {
  const result = buildCandidateArtifact({
    repositoryRoot: environment.GITHUB_WORKSPACE,
    outputDirectory: environment.AERIS_CANDIDATE_OUTPUT,
    metadata: environmentMetadata(environment),
  });
  if (environment.GITHUB_OUTPUT) {
    fs.appendFileSync(
      environment.GITHUB_OUTPUT,
      `patch_sha256=${result.manifest.patch_sha256}\npatch_bytes=${result.manifest.patch_bytes}\npath_count=${result.paths.length}\n`,
    );
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  runCandidateExtraction();
}
