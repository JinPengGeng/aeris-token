import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  readArtifact,
  readJsonFile,
  resolvePhasePaths,
  validatePhaseArtifact,
  writeArtifactAtomic,
} from './phase-contract.mjs';

const sourceDirectory = path.dirname(fileURLToPath(import.meta.url));
const defaultRepoRoot = path.resolve(sourceDirectory, '..', '..', '..');

function usage() {
  return 'usage: node .github/automation/src/run-phase.mjs <preflight|reserve|analyze|publish>';
}

function expectedInputType(phase) {
  return { reserve: 'preflight', analyze: 'reservation', publish: 'analysis' }[phase] ?? null;
}

function expectedOutputType(phase) {
  return {
    preflight: 'preflight',
    reserve: 'reservation',
    analyze: 'analysis',
    publish: 'publication',
  }[phase];
}

async function loadEngine() {
  return import('./engine.mjs');
}

function phaseRunner(engine, phase) {
  const name = {
    preflight: 'runPreflightPhase',
    reserve: 'runReservationPhase',
    analyze: 'runAnalysisPhase',
    publish: 'runPublishPhase',
  }[phase];
  if (typeof engine[name] !== 'function') throw new Error(`engine does not export ${name}`);
  return engine[name];
}

function readEvent(environment) {
  const eventPath = environment.GITHUB_EVENT_PATH;
  if (!eventPath) throw new Error('GITHUB_EVENT_PATH is required for preflight');
  return readJsonFile(eventPath);
}

function kindFromEnvironment(environment) {
  const value = environment.AERIS_OBJECT_KIND;
  if (value === 'issue') return 'issue';
  if (value === 'pull_request' || value === 'pull') return 'pull';
  throw new Error('AERIS_OBJECT_KIND must be issue or pull_request');
}

function appendGitHubOutput(environment, artifact) {
  if (!environment.GITHUB_OUTPUT) return;
  const action = artifact.artifact_type === 'preflight' ? artifact.decision.action : '';
  fs.appendFileSync(
    environment.GITHUB_OUTPUT,
    `artifact_type=${artifact.artifact_type}\nstate=${artifact.state}\naction=${action}\n`,
    'utf8',
  );
}

export async function runPhaseCli({
  argv = process.argv.slice(2),
  environment = process.env,
  repoRoot = environment.GITHUB_WORKSPACE ?? defaultRepoRoot,
  engine = null,
} = {}) {
  requireExactlyOnePhase(argv);
  const phase = argv[0];
  const paths = resolvePhasePaths(phase, environment);
  const engineModule = engine ?? (await loadEngine());
  const runner = phaseRunner(engineModule, phase);
  let artifact;
  if (phase === 'preflight') {
    artifact = await runner({
      kind: kindFromEnvironment(environment),
      eventName: environment.GITHUB_EVENT_NAME,
      event: readEvent(environment),
      environment,
      repoRoot,
    });
  } else {
    const input = readArtifact(paths.inputPath, expectedInputType(phase));
    artifact = await runner({ artifact: input, environment, repoRoot });
  }
  const output = writeArtifactAtomic(paths.outputPath, artifact, expectedOutputType(phase));
  appendGitHubOutput(environment, output);
  return { phase, path: paths.outputPath, artifact: validatePhaseArtifact(output) };
}

function requireExactlyOnePhase(argv) {
  if (argv.length !== 1 || !['preflight', 'reserve', 'analyze', 'publish'].includes(argv[0])) {
    throw new Error(usage());
  }
}

const invokedDirectly = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invokedDirectly) {
  try {
    await runPhaseCli();
  } catch (error) {
    console.error(`aeris phase failed: ${error.message}`);
    process.exitCode = 1;
  }
}
