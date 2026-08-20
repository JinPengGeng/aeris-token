import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import { buildCandidateArtifact } from './autonomy-extract.mjs';
import { createSafeGitContext, SafeGitError } from './autonomy-safe-git.mjs';

export class AgentCandidateRunnerError extends Error {
  constructor(message) {
    super(message);
    this.name = 'AgentCandidateRunnerError';
  }
}

function reject(message) {
  throw new AgentCandidateRunnerError(message);
}

function required(value, name) {
  if (typeof value !== 'string' || value.length === 0 || /[\u0000-\u001f\u007f]/.test(value)) {
    reject(`${name} is invalid`);
  }
  return value;
}

// Validate against HEAD in an isolated index so the Agent's unstaged changes
// are never executed, staged, or otherwise treated as trusted repository code.
export function validatePatchApplies({ repositoryRoot, baseSha, patchPath, temporaryDirectory = os.tmpdir() }) {
  const root = path.resolve(required(repositoryRoot, 'repositoryRoot'));
  const sha = required(baseSha, 'baseSha');
  const patch = path.resolve(required(patchPath, 'patchPath'));
  let context;
  try {
    context = createSafeGitContext({
      repositoryRoot: root,
      baseSha: sha,
      temporaryDirectory,
    });
    context.run(['apply', '--check', '--cached', '--whitespace=error-all', '--', patch]);
  } catch (error) {
    if (error instanceof SafeGitError) reject(error.message);
    throw error;
  } finally {
    context?.dispose();
  }
}

export function runAgentCandidateRunner(environment = process.env) {
  const result = buildCandidateArtifact({
    repositoryRoot: required(environment.GITHUB_WORKSPACE, 'GITHUB_WORKSPACE'),
    outputDirectory: required(environment.AERIS_CANDIDATE_OUTPUT, 'AERIS_CANDIDATE_OUTPUT'),
    metadata: {
      repository: environment.GITHUB_REPOSITORY,
      repository_id: environment.GITHUB_REPOSITORY_ID,
      issue_number: environment.AERIS_ISSUE_NUMBER,
      base_ref: environment.AERIS_BASE_REF,
      base_sha: environment.AERIS_BASE_SHA,
      trigger_run_id: environment.GITHUB_RUN_ID,
      trigger_run_attempt: environment.GITHUB_RUN_ATTEMPT,
    },
    temporaryDirectory: environment.RUNNER_TEMP || os.tmpdir(),
  });
  validatePatchApplies({
    repositoryRoot: environment.GITHUB_WORKSPACE,
    baseSha: result.manifest.base_sha,
    patchPath: result.patchPath,
    temporaryDirectory: environment.RUNNER_TEMP || os.tmpdir(),
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
  runAgentCandidateRunner();
}
