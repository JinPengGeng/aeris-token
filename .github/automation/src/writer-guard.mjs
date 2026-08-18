import { WRITER_FOUNDATION_LIMITS } from './writer-phase-contract.mjs';

export const WRITER_COMMANDS = Object.freeze(['/agent implement', '/agent retry-write']);
export const WRITER_ACTOR_PERMISSIONS = Object.freeze(['admin', 'maintain', 'write']);
export const MAXIMUM_WRITER_FILES = WRITER_FOUNDATION_LIMITS.maximum_files;
export const MAXIMUM_WRITER_PATCH_BYTES = WRITER_FOUNDATION_LIMITS.maximum_patch_bytes;
export const MAXIMUM_WRITER_FILE_BYTES = WRITER_FOUNDATION_LIMITS.maximum_file_size_bytes;
export const MAXIMUM_WRITER_TOTAL_BYTES = WRITER_FOUNDATION_LIMITS.maximum_total_file_bytes;
export const MAXIMUM_WRITER_FIX_CYCLES = WRITER_FOUNDATION_LIMITS.maximum_fix_cycles;
export const DEFAULT_WRITER_LIMITS = Object.freeze({
  maximumFiles: MAXIMUM_WRITER_FILES,
  maximumPatchBytes: MAXIMUM_WRITER_PATCH_BYTES,
  maximumFileBytes: MAXIMUM_WRITER_FILE_BYTES,
  maximumTotalBytes: MAXIMUM_WRITER_TOTAL_BYTES,
  maximumFixCycles: MAXIMUM_WRITER_FIX_CYCLES,
});

const CONTROL_CHARACTER = /[\u0000-\u001f\u007f]/;
const WINDOWS_ILLEGAL_CHARACTER = /[<>"|?*]/;
const WINDOWS_RESERVED_DEVICE = /^(?:con|prn|aux|nul|com[1-9¹²³]|lpt[1-9¹²³]|conin\$|conout\$|clock\$)$/i;
const ACTOR_LOGIN = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$/;
const CHANGE_KEYS = new Set(['path', 'previousPath', 'fromPath', 'mode', 'bytes']);
const CONTRACT_LIMIT_KEYS = new Set([
  'maximum_files', 'maximum_patch_bytes', 'maximum_file_size_bytes',
  'maximum_total_file_bytes', 'maximum_fix_cycles',
]);
const RUNTIME_LIMIT_KEYS = new Set([
  'maximumFiles', 'maximumPatchBytes', 'maximumFileBytes',
  'maximumTotalBytes', 'maximumFixCycles',
]);

function hasExactKeys(value, expected) {
  const keys = Object.keys(value);
  return keys.length === expected.size && keys.every((key) => expected.has(key));
}

function denied(reason, extra = {}) {
  return { allowed: false, reason, ...extra };
}

function allowed(extra = {}) {
  return { allowed: true, reason: null, ...extra };
}

function labelsContain(labels, label) {
  return Array.isArray(labels) && labels.some((item) => (typeof item === 'string' ? item : item?.name) === label);
}

export function writerLimitsFromContract(limits) {
  if (!limits || typeof limits !== 'object' || Array.isArray(limits)) return null;
  if (!hasExactKeys(limits, CONTRACT_LIMIT_KEYS)) return null;
  if (
    !Number.isSafeInteger(limits.maximum_patch_bytes) || limits.maximum_patch_bytes <= 0 ||
    limits.maximum_patch_bytes > MAXIMUM_WRITER_PATCH_BYTES
  ) return null;
  const normalized = {
    maximumFiles: limits.maximum_files,
    maximumPatchBytes: limits.maximum_patch_bytes,
    maximumFileBytes: limits.maximum_file_size_bytes,
    maximumTotalBytes: limits.maximum_total_file_bytes,
    maximumFixCycles: limits.maximum_fix_cycles,
  };
  if (
    !Number.isSafeInteger(normalized.maximumFiles) || normalized.maximumFiles <= 0 || normalized.maximumFiles > MAXIMUM_WRITER_FILES ||
    !Number.isSafeInteger(normalized.maximumFileBytes) || normalized.maximumFileBytes <= 0 || normalized.maximumFileBytes > MAXIMUM_WRITER_FILE_BYTES ||
    !Number.isSafeInteger(normalized.maximumTotalBytes) || normalized.maximumTotalBytes <= 0 || normalized.maximumTotalBytes > MAXIMUM_WRITER_TOTAL_BYTES ||
    !Number.isSafeInteger(normalized.maximumFixCycles) || normalized.maximumFixCycles <= 0 || normalized.maximumFixCycles > MAXIMUM_WRITER_FIX_CYCLES ||
    normalized.maximumFileBytes > normalized.maximumTotalBytes
  ) return null;
  return normalized;
}

function normalizedLimits(limits) {
  if (!limits || typeof limits !== 'object' || Array.isArray(limits)) return null;
  const hasContractKey = Object.keys(limits).some((key) => CONTRACT_LIMIT_KEYS.has(key));
  const normalized = hasContractKey
    ? writerLimitsFromContract(limits)
    : hasExactKeys(limits, RUNTIME_LIMIT_KEYS) ? limits : null;
  if (!normalized || ['maximumFiles', 'maximumPatchBytes', 'maximumFileBytes', 'maximumTotalBytes', 'maximumFixCycles'].some(
    (key) => !Number.isSafeInteger(normalized[key]) || normalized[key] <= 0,
  )) return null;
  if (
    normalized.maximumFiles > MAXIMUM_WRITER_FILES ||
    normalized.maximumPatchBytes > MAXIMUM_WRITER_PATCH_BYTES ||
    normalized.maximumFileBytes > MAXIMUM_WRITER_FILE_BYTES ||
    normalized.maximumTotalBytes > MAXIMUM_WRITER_TOTAL_BYTES ||
    normalized.maximumFixCycles > MAXIMUM_WRITER_FIX_CYCLES ||
    normalized.maximumFileBytes > normalized.maximumTotalBytes
  ) return null;
  return normalized;
}

function normalizeMode(mode) {
  if (typeof mode === 'string' && /^[0-7]{6}$/.test(mode)) return mode;
  if (typeof mode === 'number' && Number.isSafeInteger(mode)) {
    const knownModes = new Map([
      [0o100644, '100644'], [0o100755, '100755'], [0o120000, '120000'], [0o160000, '160000'],
      [100644, '100644'], [100755, '100755'], [120000, '120000'], [160000, '160000'],
    ]);
    return knownModes.get(mode) ?? null;
  }
  return null;
}

function pathFailure(candidate) {
  if (typeof candidate !== 'string' || candidate.length === 0) return 'invalid_path';
  if (CONTROL_CHARACTER.test(candidate)) return 'path_control_character';
  if (candidate !== candidate.normalize('NFC')) return 'path_not_nfc';
  if (candidate.includes('\\')) return 'path_backslash';
  if (candidate.startsWith('/') || /^[A-Za-z]:/.test(candidate)) return 'path_absolute';
  const segments = candidate.split('/');
  if (segments.some((segment) => segment === '' || segment === '.' || segment === '..')) return 'path_segment';
  const lowerSegments = segments.map((segment) => segment.toLowerCase());
  if (lowerSegments.includes('.git') || lowerSegments[0] === '.github') return 'forbidden_path';
  if (lowerSegments.includes('codeowners') || lowerSegments.includes('.gitmodules')) return 'forbidden_path';
  if (segments.some((segment) => segment.includes(':'))) return 'path_ads';
  if (segments.some((segment) => WINDOWS_ILLEGAL_CHARACTER.test(segment))) return 'path_windows_illegal_character';
  if (segments.some((segment) => /[. ]$/u.test(segment))) return 'path_windows_normalization';
  if (segments.some((segment) => {
    const baseName = segment.split('.', 1)[0].toLowerCase();
    return WINDOWS_RESERVED_DEVICE.test(baseName);
  })) return 'path_windows_reserved';
  return null;
}

function changePaths(change) {
  if (!change || typeof change !== 'object' || Array.isArray(change)) return null;
  const paths = [];
  if (Object.hasOwn(change, 'path')) paths.push(change.path);
  if (Object.hasOwn(change, 'previousPath')) paths.push(change.previousPath);
  if (Object.hasOwn(change, 'fromPath')) paths.push(change.fromPath);
  if (paths.length === 0) return null;
  return paths;
}

function isPermittedChange(change) {
  return change && typeof change === 'object' && !Array.isArray(change) &&
    Reflect.ownKeys(change).every((key) => typeof key === 'string' && CHANGE_KEYS.has(key));
}

/**
 * Validates a fully materialized git change set. The caller must supply byte sizes
 * measured from the final blobs, not untrusted model estimates.
 */
export function validateWriterChangeSet(changes, limits = DEFAULT_WRITER_LIMITS, patchBytes) {
  const normalized = normalizedLimits(limits);
  if (!normalized) return denied('invalid_limits');
  if (!Array.isArray(changes)) return denied('invalid_change_set');
  if (changes.length === 0) return denied('empty_change_set');
  if (changes.length > normalized.maximumFiles) return denied('maximum_files_exceeded');
  if (!Number.isSafeInteger(patchBytes) || patchBytes <= 0) return denied('invalid_patch_bytes');
  if (patchBytes > normalized.maximumPatchBytes) return denied('maximum_patch_bytes_exceeded');

  const canonicalPaths = new Set();
  let totalBytes = 0;
  for (const change of changes) {
    if (!isPermittedChange(change)) return denied('invalid_change');
    const paths = changePaths(change);
    if (paths === null) return denied('invalid_change');
    const mode = normalizeMode(change.mode);
    if (mode !== '100644' && mode !== '100755') return denied('non_regular_mode');
    if (!Number.isSafeInteger(change.bytes) || change.bytes < 0) return denied('invalid_file_bytes');
    if (change.bytes > normalized.maximumFileBytes) return denied('maximum_file_bytes_exceeded');
    totalBytes += change.bytes;
    if (!Number.isSafeInteger(totalBytes) || totalBytes > normalized.maximumTotalBytes) {
      return denied('maximum_total_bytes_exceeded');
    }
    for (const candidate of paths) {
      const reason = pathFailure(candidate);
      if (reason) return denied(reason, { path: candidate });
      const canonical = candidate.normalize('NFC').toLowerCase();
      if (canonicalPaths.has(canonical)) return denied('path_collision', { path: candidate });
      canonicalPaths.add(canonical);
    }
  }
  return allowed({ fileCount: changes.length, patchBytes, totalBytes });
}

export function branchForIssue(issueNumber) {
  if (!Number.isSafeInteger(issueNumber) || issueNumber <= 0) return null;
  return `agent/issue-${issueNumber}`;
}

/**
 * Fail-closed writer admission decision. All three independent feature switches
 * must be literal true before a code-writing run can start.
 */
export function evaluateWriterRequest({
  command,
  actorLogin,
  actorPermission,
  issue,
  switches,
  changeSet = null,
  patchBytes,
  fixCycle,
  limits = DEFAULT_WRITER_LIMITS,
} = {}) {
  if (!WRITER_COMMANDS.includes(command)) return denied('unsupported_command');
  if (typeof actorLogin !== 'string') return denied('invalid_actor');
  if (/\[bot\]/i.test(actorLogin)) return denied('bot_actor_not_allowed');
  if (!ACTOR_LOGIN.test(actorLogin) || CONTROL_CHARACTER.test(actorLogin)) return denied('invalid_actor');
  if (!WRITER_ACTOR_PERMISSIONS.includes(actorPermission)) return denied('insufficient_permission');
  if (!switches || switches.globalEnabled !== true || switches.writerVariableEnabled !== true || switches.writerContractEnabled !== true) {
    return denied('writer_disabled');
  }
  if (!issue || issue.state !== 'open') return denied('issue_not_open');
  if (issue.isPullRequest !== false) return denied('pull_request_not_allowed');
  if (!Number.isSafeInteger(issue.number) || issue.number <= 0) return denied('invalid_issue_number');
  if (!labelsContain(issue.labels, 'agent-ready')) return denied('missing_agent_ready_label');
  const branch = branchForIssue(issue.number);
  const normalized = normalizedLimits(limits);
  if (!normalized) return denied('invalid_limits');
  if (!Number.isSafeInteger(fixCycle) || fixCycle < 0) return denied('invalid_fix_cycle');
  if (command === '/agent implement' && fixCycle !== 0) return denied('invalid_fix_cycle');
  if (command === '/agent retry-write' && fixCycle === 0) return denied('invalid_fix_cycle');
  if (fixCycle > normalized.maximumFixCycles) return denied('maximum_fix_cycles_exceeded');
  if (changeSet === null) return allowed({ branch });
  const changes = validateWriterChangeSet(changeSet, normalized, patchBytes);
  return changes.allowed ? allowed({ branch, ...changes }) : changes;
}
