import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { executorDescriptorForRoute, validateExecutorRegistry } from './ai-executor-contract.mjs';
import { buildCandidateArtifact } from './autonomy-extract.mjs';
import { createSafeGitContext, SafeGitError } from './autonomy-safe-git.mjs';

const MAXIMUM_EXECUTOR_REGISTRY_BYTES = 65_536;

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

function sealedRegistryPath() {
  const moduleDirectory = path.dirname(fileURLToPath(import.meta.url));
  const direct = path.join(moduleDirectory, 'ai-executors.json');
  try {
    const stat = fs.lstatSync(direct);
    if (!stat.isFile() || stat.isSymbolicLink() || stat.size > MAXIMUM_EXECUTOR_REGISTRY_BYTES) {
      reject('trusted candidate executor registry is invalid');
    }
    return direct;
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error;
  }
  const isSourceTreeRuntime = path.basename(moduleDirectory) === 'src' && path.basename(path.dirname(moduleDirectory)) === 'automation';
  if (!isSourceTreeRuntime) reject('trusted candidate executor registry is unavailable');
  const sourceRegistry = path.resolve(moduleDirectory, '..', '..', 'ai-executors.json');
  try {
    const stat = fs.lstatSync(sourceRegistry);
    if (!stat.isFile() || stat.isSymbolicLink() || stat.size > MAXIMUM_EXECUTOR_REGISTRY_BYTES) {
      reject('trusted candidate executor registry is invalid');
    }
    return sourceRegistry;
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error;
  }
  reject('trusted candidate executor registry is unavailable');
}

export function sealedCandidateExecutor() {
  let registry;
  try {
    registry = JSON.parse(fs.readFileSync(sealedRegistryPath(), 'utf8'));
  } catch (error) {
    if (error instanceof AgentCandidateRunnerError) throw error;
    reject('trusted candidate executor registry is invalid');
  }
  try {
    return executorDescriptorForRoute(validateExecutorRegistry(registry), 'candidate');
  } catch {
    reject('trusted candidate executor registry is invalid');
  }
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
      executor: sealedCandidateExecutor(),
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
