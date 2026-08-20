import { createHash } from 'node:crypto';

export const CANDIDATE_SCHEMA_VERSION = 1;
export const MAX_CANDIDATE_PATCH_BYTES = 1024 * 1024;
export const MAX_CANDIDATE_FILES = 100;
export const MAX_CANDIDATE_FILE_TEXT_BYTES = 256 * 1024;

const MANIFEST_KEYS = Object.freeze([
  'schema_version',
  'repository',
  'repository_id',
  'task_id',
  'issue_number',
  'base_ref',
  'base_sha',
  'trigger_run_id',
  'trigger_run_attempt',
  'patch_sha256',
  'patch_bytes',
  'created_at',
]);
const EXPECTED_KEYS = Object.freeze([
  'repository',
  'repository_id',
  'task_id',
  'issue_number',
  'base_ref',
  'base_sha',
  'trigger_run_id',
  'trigger_run_attempt',
]);
const SHA256 = /^[0-9a-f]{64}$/;
const COMMIT_SHA = /^[0-9a-f]{40}$/;
const RUN_ID = /^(?:0|[1-9][0-9]*)$/;
const UTC_TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?Z$/;

export class CandidateValidationError extends Error {
  constructor(message) {
    super(message);
    this.name = 'CandidateValidationError';
  }
}

function reject(message) {
  throw new CandidateValidationError(message);
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

function string(value, name, maximumLength, pattern = null) {
  requireCondition(typeof value === 'string' && value.length > 0 && value.length <= maximumLength, `${name} is invalid`);
  requireCondition(!/[\u0000-\u001f\u007f]/.test(value), `${name} contains control characters`);
  if (pattern) requireCondition(pattern.test(value), `${name} format is invalid`);
  return value;
}

function positiveInteger(value, name) {
  requireCondition(Number.isSafeInteger(value) && value > 0, `${name} must be a positive safe integer`);
  return value;
}

function deepFreeze(value) {
  if (value && typeof value === 'object' && !Object.isFrozen(value)) {
    for (const child of Object.values(value)) deepFreeze(child);
    Object.freeze(value);
  }
  return value;
}

function toPatchBuffer(patch) {
  requireCondition(Buffer.isBuffer(patch) || typeof patch === 'string', 'patch must be a string or Buffer');
  const buffer = Buffer.isBuffer(patch) ? Buffer.from(patch) : Buffer.from(patch, 'utf8');
  requireCondition(buffer.length > 0 && buffer.length <= MAX_CANDIDATE_PATCH_BYTES, 'patch exceeds maximum size');
  const text = buffer.toString('utf8');
  requireCondition(Buffer.from(text, 'utf8').equals(buffer), 'patch must be valid UTF-8');
  return { buffer, text };
}

function validatePath(value) {
  requireCondition(typeof value === 'string' && value.length > 0, 'patch path is invalid');
  requireCondition(!value.includes('\0'), 'patch path contains a null byte');
  requireCondition(!value.includes('\\'), 'patch path contains a backslash');
  requireCondition(!value.startsWith('/') && !/^[A-Za-z]:/.test(value), 'patch path must be relative');
  const segments = value.split('/');
  requireCondition(segments.every((segment) => segment.length > 0 && segment !== '.' && segment !== '..'), 'patch path escapes repository');
  const folded = value.toLocaleLowerCase('en-US');
  requireCondition(!folded.startsWith('.github/'), 'patch path is governed');
  requireCondition(!['codeowners', '.gitmodules'].includes(folded), 'patch path is governed');
  return value;
}

function diffPath(token, prefix) {
  requireCondition(token.startsWith(prefix), 'diff header path prefix is invalid');
  return validatePath(token.slice(prefix.length));
}

function parseDiffHeader(line) {
  const match = /^diff --git (a\/[^\s]+) (b\/[^\s]+)$/.exec(line);
  requireCondition(match !== null, 'diff header is invalid or quoted');
  return [diffPath(match[1], 'a/'), diffPath(match[2], 'b/')];
}

function markerPath(line, marker) {
  const value = line.slice(marker.length).split('\t', 1)[0];
  if (value === '/dev/null') return null;
  return diffPath(value, marker === '--- ' ? 'a/' : 'b/');
}

export function parseUnifiedDiff(patch) {
  const { text } = toPatchBuffer(patch);
  requireCondition(!text.includes('GIT binary patch') && !text.includes('Binary files '), 'binary patch is forbidden');
  const lines = text.split('\n').map((line) => line.endsWith('\r') ? line.slice(0, -1) : line);
  requireCondition(lines[0]?.startsWith('diff --git '), 'patch must begin with a unified diff header');
  const sections = [];
  let current = null;
  for (const line of lines) {
    if (line.startsWith('diff --git ')) {
      const [before, after] = parseDiffHeader(line);
      current = { before, after, lines: [line] };
      sections.push(current);
    } else {
      requireCondition(current !== null, 'patch contains content before its first diff header');
      current.lines.push(line);
    }
  }
  requireCondition(sections.length > 0 && sections.length <= MAX_CANDIDATE_FILES, 'patch file count is invalid');

  const paths = [];
  const seen = new Set();
  const folded = new Set();
  for (const section of sections) {
    const sectionText = section.lines.join('\n');
    requireCondition(Buffer.byteLength(sectionText, 'utf8') <= MAX_CANDIDATE_FILE_TEXT_BYTES, 'patch file text exceeds maximum size');
    requireCondition(/^(?:--- |rename from |new file mode |deleted file mode )/m.test(sectionText), 'diff section lacks file metadata');
    for (const mode of sectionText.matchAll(/^(?:(?:old mode|new mode|new file mode|deleted file mode) ([0-9]{6})|index [0-9a-f]+(?:\.\.[0-9a-f]+)? ([0-9]{6}))$/gm)) {
      requireCondition((mode[1] ?? mode[2]) === '100644', 'patch changes a non-regular or executable file mode');
    }
    if (/^@@ /m.test(sectionText)) {
      const oldMarker = section.lines.find((line) => line.startsWith('--- '));
      const newMarker = section.lines.find((line) => line.startsWith('+++ '));
      requireCondition(oldMarker !== undefined && newMarker !== undefined, 'diff hunk lacks file markers');
      const oldPath = markerPath(oldMarker, '--- ');
      const newPath = markerPath(newMarker, '+++ ');
      requireCondition(oldPath === null || oldPath === section.before, 'old diff marker does not match header');
      requireCondition(newPath === null || newPath === section.after, 'new diff marker does not match header');
      requireCondition(oldPath !== null || newPath !== null, 'diff hunk has no repository path');
    }
    const sectionPaths = section.before === section.after ? [section.before] : [section.before, section.after];
    for (const candidate of sectionPaths) {
      requireCondition(!seen.has(candidate), 'patch contains duplicate paths');
      const fold = candidate.toLocaleLowerCase('en-US');
      requireCondition(!folded.has(fold), 'patch paths have a case-fold conflict');
      seen.add(candidate);
      folded.add(fold);
      paths.push(candidate);
    }
  }
  return deepFreeze([...paths]);
}

export function validateCandidateManifest(manifest, expected) {
  exactKeys(manifest, MANIFEST_KEYS, 'candidate manifest');
  exactKeys(expected, EXPECTED_KEYS, 'candidate expectation');
  requireCondition(manifest.schema_version === CANDIDATE_SCHEMA_VERSION, 'candidate manifest schema_version is invalid');
  const normalized = {
    schema_version: CANDIDATE_SCHEMA_VERSION,
    repository: string(manifest.repository, 'repository', 256),
    repository_id: positiveInteger(manifest.repository_id, 'repository_id'),
    task_id: string(manifest.task_id, 'task_id', 256),
    issue_number: positiveInteger(manifest.issue_number, 'issue_number'),
    base_ref: string(manifest.base_ref, 'base_ref', 256),
    base_sha: string(manifest.base_sha, 'base_sha', 40, COMMIT_SHA),
    trigger_run_id: string(manifest.trigger_run_id, 'trigger_run_id', 32, RUN_ID),
    trigger_run_attempt: positiveInteger(manifest.trigger_run_attempt, 'trigger_run_attempt'),
    patch_sha256: string(manifest.patch_sha256, 'patch_sha256', 64, SHA256),
    patch_bytes: positiveInteger(manifest.patch_bytes, 'patch_bytes'),
    created_at: string(manifest.created_at, 'created_at', 64, UTC_TIMESTAMP),
  };
  requireCondition(Number.isFinite(Date.parse(normalized.created_at)), 'created_at is invalid');
  requireCondition(normalized.task_id === `issue:${normalized.issue_number}`, 'task_id and issue_number disagree');
  requireCondition(normalized.base_ref.startsWith('refs/heads/'), 'base_ref is invalid');
  for (const key of EXPECTED_KEYS) requireCondition(normalized[key] === expected[key], `candidate ${key} does not match expected`);
  return deepFreeze(normalized);
}

export function validateCandidateArtifact(manifestOrArtifact, patchArgument, expectedArgument) {
  const artifactInput = isObject(manifestOrArtifact) && Object.hasOwn(manifestOrArtifact, 'manifest') && Object.hasOwn(manifestOrArtifact, 'patch');
  const manifest = artifactInput ? manifestOrArtifact.manifest : manifestOrArtifact;
  const patch = artifactInput ? manifestOrArtifact.patch : patchArgument;
  const expected = artifactInput ? manifestOrArtifact.expected : expectedArgument;
  const normalizedManifest = validateCandidateManifest(manifest, expected);
  const { buffer } = toPatchBuffer(patch);
  requireCondition(normalizedManifest.patch_bytes === buffer.length, 'patch_bytes does not match patch');
  const digest = createHash('sha256').update(buffer).digest('hex');
  requireCondition(normalizedManifest.patch_sha256 === digest, 'patch_sha256 does not match patch');
  const paths = parseUnifiedDiff(buffer);
  return deepFreeze({ manifest: normalizedManifest, paths });
}
