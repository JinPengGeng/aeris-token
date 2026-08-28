import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

import { validateExecutorIdentity } from './ai-executor-contract.mjs';

export const ARTIFACT_SCHEMA_VERSION = 2;
export const MAX_ARTIFACT_BYTES = 1024 * 1024;

const SHA256 = /^[0-9a-f]{64}$/;
const COMMIT_SHA = /^[0-9a-f]{40}$/;
const SAFE_IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9._:/-]*$/;
const LEASE_TOKEN = /^(?:[0-9a-f]{32,128}|[A-Za-z0-9_-]{43,171})$/;
const PHASES = Object.freeze(['preflight', 'reserve', 'analyze', 'publish']);
const INPUT_ARTIFACT_TYPE = Object.freeze({
  reserve: 'preflight',
  analyze: 'reservation',
  publish: 'analysis',
});
const OUTPUT_FILE = Object.freeze({
  preflight: 'preflight.json',
  reserve: 'reservation.json',
  analyze: 'analysis.json',
  publish: 'publish-result.json',
});

function reject(message) {
  throw new Error(message);
}

function requireCondition(condition, message) {
  if (!condition) reject(message);
}

function isObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function exactKeys(value, keys, name) {
  requireCondition(isObject(value), `${name} must be an object`);
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  requireCondition(
    actual.length === expected.length && actual.every((key, index) => key === expected[index]),
    `${name} has unexpected keys`,
  );
}

function boundedString(value, name, maximumLength, { nullable = false, pattern = null } = {}) {
  if (nullable && value === null) return null;
  requireCondition(typeof value === 'string', `${name} must be a string`);
  requireCondition(value.length > 0 && value.length <= maximumLength, `${name} length is invalid`);
  requireCondition(!/[\u0000-\u001f\u007f]/.test(value), `${name} contains control characters`);
  if (pattern) requireCondition(pattern.test(value), `${name} format is invalid`);
  return value;
}

function nullableInteger(value, name, { minimum = 0 } = {}) {
  if (value === null) return null;
  requireCondition(Number.isSafeInteger(value) && value >= minimum, `${name} must be a safe integer`);
  return value;
}

function integer(value, name, { minimum = 0 } = {}) {
  requireCondition(Number.isSafeInteger(value) && value >= minimum, `${name} must be a safe integer`);
  return value;
}

function timestamp(value, name, { nullable = false } = {}) {
  if (nullable && value === null) return null;
  boundedString(value, name, 64);
  requireCondition(
    /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?Z$/.test(value),
    `${name} must be a UTC ISO timestamp`,
  );
  const milliseconds = Date.parse(value);
  requireCondition(Number.isFinite(milliseconds), `${name} must be an ISO timestamp`);
  return value;
}

function nullableReason(value, name) {
  return boundedString(value, name, 160, { nullable: true, pattern: SAFE_IDENTIFIER });
}

function jsonByteLength(value) {
  let encoded;
  try {
    encoded = JSON.stringify(value);
  } catch {
    reject('artifact must be JSON serializable');
  }
  requireCondition(encoded !== undefined, 'artifact must be JSON serializable');
  return Buffer.byteLength(encoded, 'utf8');
}

function boundedJsonValue(value, name, maximumBytes) {
  requireCondition(
    value === null || typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean' ||
      Array.isArray(value) || isObject(value),
    `${name} must be a JSON value`,
  );
  requireCondition(jsonByteLength(value) <= maximumBytes, `${name} exceeds maximum size`);
  const encoded = JSON.stringify(value);
  const decoded = JSON.parse(encoded);
  requireCondition(jsonByteLength(decoded) <= maximumBytes, `${name} exceeds maximum size`);
  return decoded;
}

function validateEnvelope(value, expectedType, keys) {
  exactKeys(value, ['schema_version', 'artifact_type', ...keys], `${expectedType} artifact`);
  requireCondition(
    value.schema_version === ARTIFACT_SCHEMA_VERSION,
    `${expectedType} artifact schema_version must be ${ARTIFACT_SCHEMA_VERSION}`,
  );
  requireCondition(value.artifact_type === expectedType, `artifact_type must be ${expectedType}`);
  requireCondition(jsonByteLength(value) <= MAX_ARTIFACT_BYTES, 'artifact exceeds maximum size');
}

function validateDecision(value) {
  exactKeys(value, value.agent === undefined ? ['action', 'reason'] : ['action', 'reason', 'agent'], 'preflight decision');
  requireCondition(
    ['analyze', 'status', 'cancel', 'skip', 'disabled'].includes(value.action),
    'preflight decision action is invalid',
  );
  return {
    action: value.action,
    reason: boundedString(value.reason, 'preflight decision reason', 160, { pattern: SAFE_IDENTIFIER }),
    ...(value.agent === undefined ? {} : {
      agent: boundedString(value.agent, 'preflight decision agent', 160, { pattern: SAFE_IDENTIFIER }),
    }),
  };
}

function validateContext(value) {
  exactKeys(
    value,
    [
      'kind',
      'event_name',
      'number',
      'agent',
      'source_key',
      'object_id',
      'object_generation',
      'input_sha',
      'policy_sha',
      'run_id',
    ],
    'preflight context',
  );
  requireCondition(['issue', 'pull'].includes(value.kind), 'preflight context kind is invalid');
  boundedString(value.event_name, 'preflight context event_name', 80, { pattern: SAFE_IDENTIFIER });
  requireCondition(Number.isSafeInteger(value.number) && value.number > 0, 'preflight context number is invalid');
  const agent = boundedString(value.agent, 'preflight context agent', 80, {
    nullable: true,
    pattern: SAFE_IDENTIFIER,
  });
  const sourceKey = boundedString(value.source_key, 'preflight context source_key', 512, {
    pattern: SAFE_IDENTIFIER,
  });
  const objectId = boundedString(value.object_id, 'preflight context object_id', 160, {
    pattern: SAFE_IDENTIFIER,
  });
  const objectGeneration = boundedString(
    value.object_generation,
    'preflight context object_generation',
    160,
    { pattern: SAFE_IDENTIFIER },
  );
  const inputSha = boundedString(value.input_sha, 'preflight context input_sha', 64, {
    nullable: true,
    pattern: SHA256,
  });
  const policySha = boundedString(value.policy_sha, 'preflight context policy_sha', 40, {
    pattern: COMMIT_SHA,
  });
  const runId = boundedString(value.run_id, 'preflight context run_id', 128, {
    nullable: true,
    pattern: SAFE_IDENTIFIER,
  });
  return {
    kind: value.kind,
    event_name: value.event_name,
    number: value.number,
    agent,
    source_key: sourceKey,
    object_id: objectId,
    object_generation: objectGeneration,
    input_sha: inputSha,
    policy_sha: policySha,
    run_id: runId,
  };
}

function validatePhaseResult(value, name) {
  exactKeys(value, ['state', 'reason', 'comment_id'], name);
  const state = boundedString(value.state, `${name} state`, 80, { pattern: SAFE_IDENTIFIER });
  return {
    state,
    reason: nullableReason(value.reason, `${name} reason`),
    comment_id: nullableInteger(value.comment_id, `${name} comment_id`, { minimum: 1 }),
  };
}

export function validatePreflightArtifact(value) {
  validateEnvelope(value, 'preflight', ['state', 'decision', 'context', 'input']);
  requireCondition(['ready', 'terminal'].includes(value.state), 'preflight state is invalid');
  const decision = validateDecision(value.decision);
  const context = validateContext(value.context);
  const ready = value.state === 'ready';
  requireCondition(ready === (decision.action === 'analyze'), 'preflight state and decision disagree');
  if (ready) {
    requireCondition(context.agent !== null, 'analysis-ready preflight requires an agent');
    requireCondition(context.input_sha !== null, 'analysis-ready preflight requires input_sha');
    requireCondition(value.input === null, 'analysis-ready preflight must not contain model input');
  } else {
    requireCondition(value.input === null, 'terminal preflight must not contain model input');
  }
  return {
    schema_version: ARTIFACT_SCHEMA_VERSION,
    artifact_type: 'preflight',
    state: value.state,
    decision,
    context,
    input: null,
  };
}

function validateReservation(value) {
  if (value === null) return null;
  exactKeys(
    value,
    ['comment_id', 'comment_updated_at', 'lease_expires_at', 'lease_token', 'cancel_epoch'],
    'reservation details',
  );
  return {
    comment_id: nullableInteger(value.comment_id, 'reservation comment_id', { minimum: 1 }),
    comment_updated_at: timestamp(value.comment_updated_at, 'reservation comment_updated_at', {
      nullable: true,
    }),
    lease_expires_at: timestamp(value.lease_expires_at, 'reservation lease_expires_at'),
    lease_token: boundedString(value.lease_token, 'reservation lease_token', 171, {
      pattern: LEASE_TOKEN,
    }),
    cancel_epoch: integer(value.cancel_epoch, 'reservation cancel_epoch'),
  };
}

export function validateReservationArtifact(value) {
  validateEnvelope(value, 'reservation', ['state', 'preflight', 'reservation', 'result']);
  requireCondition(['reserved', 'terminal'].includes(value.state), 'reservation state is invalid');
  const preflight = validatePreflightArtifact(value.preflight);
  const reservation = validateReservation(value.reservation);
  const result = value.result === null ? null : validatePhaseResult(value.result, 'reservation result');
  if (value.state === 'reserved') {
    requireCondition(preflight.state === 'ready', 'reserved artifact requires ready preflight');
    requireCondition(reservation !== null, 'reserved artifact requires reservation details');
    requireCondition(result === null, 'reserved artifact must not contain terminal result');
  } else {
    requireCondition(reservation === null, 'terminal reservation must not contain reservation details');
    requireCondition(result !== null, 'terminal reservation requires a result');
  }
  return {
    schema_version: ARTIFACT_SCHEMA_VERSION,
    artifact_type: 'reservation',
    state: value.state,
    preflight,
    reservation,
    result,
  };
}

function validateModel(value) {
  if (value === null) return null;
  exactKeys(value, ['alias', 'id', 'executor', 'duration_ms', 'usage'], 'analysis model');
  const usage = value.usage === null ? null : boundedJsonValue(value.usage, 'analysis model usage', 4096);
  requireCondition(usage === null || isObject(usage), 'analysis model usage must be an object or null');
  return {
    alias: boundedString(value.alias, 'analysis model alias', 128, { pattern: SAFE_IDENTIFIER }),
    id: boundedString(value.id, 'analysis model id', 256, { pattern: SAFE_IDENTIFIER }),
    executor: validateExecutorIdentity(value.executor, 'analysis executor'),
    duration_ms: nullableInteger(value.duration_ms, 'analysis model duration_ms'),
    usage,
  };
}

function validateFailure(value) {
  if (value === null) return null;
  exactKeys(value, ['code'], 'analysis failure');
  return {
    code: boundedString(value.code, 'analysis failure code', 160, { pattern: SAFE_IDENTIFIER }),
  };
}

export function validateAnalysisArtifact(value) {
  validateEnvelope(value, 'analysis', ['state', 'reservation', 'output', 'model', 'failure']);
  requireCondition(['completed', 'failed', 'terminal'].includes(value.state), 'analysis state is invalid');
  const reservation = validateReservationArtifact(value.reservation);
  const output = value.output === null ? null : boundedJsonValue(value.output, 'analysis output', 512 * 1024);
  const model = validateModel(value.model);
  const failure = validateFailure(value.failure);
  if (value.state === 'completed') {
    requireCondition(reservation.state === 'reserved', 'completed analysis requires a reservation');
    requireCondition(isObject(output), 'completed analysis requires output');
    requireCondition(model !== null, 'completed analysis requires model metadata');
    requireCondition(failure === null, 'completed analysis must not contain failure');
  } else if (value.state === 'failed') {
    requireCondition(reservation.state === 'reserved', 'failed analysis requires a reservation');
    requireCondition(output === null && model === null, 'failed analysis must not contain model output');
    requireCondition(failure !== null, 'failed analysis requires failure details');
  } else {
    requireCondition(reservation.state === 'terminal', 'terminal analysis requires terminal reservation');
    requireCondition(output === null && model === null && failure === null, 'terminal analysis must be empty');
  }
  return {
    schema_version: ARTIFACT_SCHEMA_VERSION,
    artifact_type: 'analysis',
    state: value.state,
    reservation,
    output,
    model,
    failure,
  };
}

export function validatePublicationArtifact(value) {
  validateEnvelope(value, 'publication', ['state', 'analysis', 'result']);
  const analysis = validateAnalysisArtifact(value.analysis);
  const result = validatePhaseResult(value.result, 'publication result');
  requireCondition(
    ['published', 'stale', 'cancelled', 'noop', 'in_progress', 'rate_limited', 'disabled', 'skipped'].includes(
      value.state,
    ),
    'publication state is invalid',
  );
  requireCondition(value.state === result.state, 'publication state and result disagree');
  return {
    schema_version: ARTIFACT_SCHEMA_VERSION,
    artifact_type: 'publication',
    state: result.state,
    analysis,
    result,
  };
}

export function validatePhaseArtifact(value, expectedType = null) {
  requireCondition(isObject(value), 'artifact must be an object');
  const type = expectedType ?? value.artifact_type;
  const validators = {
    preflight: validatePreflightArtifact,
    reservation: validateReservationArtifact,
    analysis: validateAnalysisArtifact,
    publication: validatePublicationArtifact,
  };
  requireCondition(Object.hasOwn(validators, type), `unsupported artifact type: ${String(type)}`);
  if (expectedType !== null) {
    requireCondition(value.artifact_type === expectedType, `expected ${expectedType} artifact`);
  }
  return validators[type](value);
}

function allowedRoot(environment) {
  const configured = environment.AERIS_ARTIFACT_ROOT || environment.RUNNER_TEMP || os.tmpdir();
  requireCondition(typeof configured === 'string' && configured.length > 0, 'artifact root is invalid');
  return path.resolve(configured);
}

function resolveInsideRoot(candidate, root, name) {
  requireCondition(typeof candidate === 'string' && candidate.length > 0, `${name} is invalid`);
  requireCondition(!candidate.includes('\0'), `${name} contains a null byte`);
  const resolved = path.resolve(candidate);
  const relative = path.relative(root, resolved);
  requireCondition(
    relative === '' || (!relative.startsWith(`..${path.sep}`) && relative !== '..' && !path.isAbsolute(relative)),
    `${name} must stay within artifact root`,
  );
  return resolved;
}

export function resolvePhasePaths(phase, environment = process.env) {
  requireCondition(PHASES.includes(phase), `unsupported phase: ${phase}`);
  const root = allowedRoot(environment);
  const defaultDirectory = path.join(root, 'aeris-automation');
  const outputCandidate = environment.AERIS_OUTPUT_PATH || path.join(defaultDirectory, OUTPUT_FILE[phase]);
  const outputPath = resolveInsideRoot(outputCandidate, root, 'AERIS_OUTPUT_PATH');
  let inputPath = null;
  if (phase !== 'preflight') {
    const inputType = INPUT_ARTIFACT_TYPE[phase];
    const inputCandidate = environment.AERIS_INPUT_PATH || path.join(defaultDirectory, `${inputType}.json`);
    inputPath = resolveInsideRoot(inputCandidate, root, 'AERIS_INPUT_PATH');
  }
  requireCondition(inputPath === null || inputPath !== outputPath, 'input and output paths must differ');
  return { root, inputPath, outputPath };
}

export function readJsonFile(filePath, { maximumBytes = MAX_ARTIFACT_BYTES } = {}) {
  requireCondition(Number.isSafeInteger(maximumBytes) && maximumBytes > 0, 'maximumBytes is invalid');
  let stat;
  try {
    stat = fs.lstatSync(filePath);
  } catch (error) {
    throw new Error(`cannot inspect JSON file: ${error.code ?? 'unknown_error'}`);
  }
  requireCondition(stat.isFile() && !stat.isSymbolicLink(), 'JSON input must be a regular file');
  requireCondition(stat.size <= maximumBytes, 'JSON input exceeds maximum size');
  const buffer = fs.readFileSync(filePath);
  requireCondition(buffer.length <= maximumBytes, 'JSON input exceeds maximum size');
  let value;
  try {
    value = JSON.parse(buffer.toString('utf8'));
  } catch {
    reject('JSON input is invalid');
  }
  return value;
}

export function readArtifact(filePath, expectedType) {
  return validatePhaseArtifact(readJsonFile(filePath), expectedType);
}

export function writeArtifactAtomic(filePath, value, expectedType = null) {
  const artifact = validatePhaseArtifact(value, expectedType);
  const serialized = `${JSON.stringify(artifact)}\n`;
  requireCondition(Buffer.byteLength(serialized, 'utf8') <= MAX_ARTIFACT_BYTES, 'artifact exceeds maximum size');
  const directory = path.dirname(filePath);
  fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
  const temporary = path.join(
    directory,
    `.${path.basename(filePath)}.${process.pid}.${Date.now()}.${Math.random().toString(16).slice(2)}.tmp`,
  );
  try {
    fs.writeFileSync(temporary, serialized, { encoding: 'utf8', flag: 'wx', mode: 0o600 });
    fs.renameSync(temporary, filePath);
  } finally {
    try {
      fs.unlinkSync(temporary);
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
  }
  return artifact;
}
