import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import { buildCandidateArtifact } from './autonomy-extract.mjs';

const NULL_DEVICE = process.platform === 'win32' ? 'NUL' : '/dev/null';

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

function git(repositoryRoot, args, environment = {}) {
  try {
    return execFileSync('git', args, {
      cwd: repositoryRoot,
      encoding: 'utf8',
      env: {
        ...process.env,
        ...environment,
        GIT_CONFIG_NOSYSTEM: '1',
        GIT_CONFIG_GLOBAL: NULL_DEVICE,
        GIT_CONFIG_SYSTEM: NULL_DEVICE,
        GIT_TERMINAL_PROMPT: '0',
      },
      timeout: 30_000,
      maxBuffer: 2 * 1024 * 1024,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
  } catch (error) {
    const stderr = typeof error?.stderr === 'string' ? error.stderr.trim() : '';
    reject(`git ${args[0]} failed${stderr ? `: ${stderr}` : ''}`);
  }
}

// Validate against HEAD in an isolated index so the Agent's unstaged changes
// are never executed, staged, or otherwise treated as trusted repository code.
export function validatePatchApplies({ repositoryRoot, patchPath, temporaryDirectory = os.tmpdir() }) {
  const root = path.resolve(required(repositoryRoot, 'repositoryRoot'));
  const patch = path.resolve(required(patchPath, 'patchPath'));
  const scratch = fs.mkdtempSync(path.join(path.resolve(temporaryDirectory), 'aeris-candidate-index-'));
  const index = path.join(scratch, 'index');
  try {
    git(root, ['read-tree', 'HEAD'], { GIT_INDEX_FILE: index });
    git(root, ['apply', '--check', '--cached', '--whitespace=error-all', '--', patch], { GIT_INDEX_FILE: index });
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
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
  });
  validatePatchApplies({
    repositoryRoot: environment.GITHUB_WORKSPACE,
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
