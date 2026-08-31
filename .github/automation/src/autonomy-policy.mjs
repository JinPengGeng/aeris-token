const SHA = /^[0-9a-f]{40}$/;
const CONTROL_CHARACTERS = /[\u0000-\u001f\u007f]/;
const ALLOWED_FILE_STATUSES = new Set(['added', 'modified']);
const GOVERNED_FILENAMES = new Set(['codeowners', '.gitmodules']);
const MAXIMUM_LABELS = 100;
const REQUIRED_CONFIG_KEYS = Object.freeze([
  'repository',
  'base_ref',
  'writer_login',
  'branch_prefix',
  'maximum_files',
  'maximum_changes',
]);

export class AutonomyPolicyError extends Error {
  constructor(message) {
    super(message);
    this.name = 'AutonomyPolicyError';
  }
}

function fail(message) {
  throw new AutonomyPolicyError(message);
}

function isObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function exactKeys(value, keys, name) {
  if (!isObject(value)) fail(`${name} must be an object`);
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index])) {
    fail(`${name} has unexpected keys`);
  }
}

function nonEmptyString(value, name, maximumLength = 512) {
  if (typeof value !== 'string' || value.length === 0 || value.length > maximumLength || CONTROL_CHARACTERS.test(value)) {
    fail(`${name} is invalid`);
  }
  return value;
}

function safeInteger(value, name) {
  if (!Number.isSafeInteger(value) || value < 0) fail(`${name} must be a non-negative safe integer`);
  return value;
}

function validRelativePath(value, name) {
  const path = nonEmptyString(value, name, 1024);
  if (path.includes('\\') || path.startsWith('/') || /^[A-Za-z]:/.test(path)) fail(`${name} must be a relative slash-separated path`);
  const segments = path.split('/');
  if (segments.some((segment) => segment.length === 0 || segment === '.' || segment === '..')) fail(`${name} escapes the repository`);
  return path;
}

function validBranch(value, name) {
  const branch = nonEmptyString(value, name, 255);
  if (branch.startsWith('refs/') || branch.startsWith('/') || branch.endsWith('/') || branch.includes('..') || branch.includes('~') || branch.includes('^') || branch.includes(':') || branch.includes('?') || branch.includes('*') || branch.includes('[') || branch.includes('\\') || branch.endsWith('.lock')) {
    fail(`${name} is invalid`);
  }
  return branch;
}

function validateRef(value, name) {
  const ref = validBranch(value, name);
  if (ref !== 'main' && !ref.startsWith('release/')) fail(`${name} is not an allowed base ref`);
  return ref;
}

function validateSha(value, name) {
  const sha = nonEmptyString(value, name, 40);
  if (!SHA.test(sha)) fail(`${name} format is invalid`);
  return sha;
}

function validateConfig(config) {
  exactKeys(config, REQUIRED_CONFIG_KEYS, 'policy configuration');
  const branchPrefix = nonEmptyString(config.branch_prefix, 'configuration branch_prefix', 255);
  if (!branchPrefix.endsWith('-')) fail('configuration branch_prefix must end with a hyphen');
  validBranch(`${branchPrefix}1`, 'configuration branch_prefix');
  const maximumFiles = safeInteger(config.maximum_files, 'configuration maximum_files');
  const maximumChanges = safeInteger(config.maximum_changes, 'configuration maximum_changes');
  if (maximumFiles === 0 || maximumFiles > 20 || maximumChanges === 0 || maximumChanges > 2000) {
    fail('configuration eligibility limits exceed the policy maximum');
  }
  return Object.freeze({
    repository: nonEmptyString(config.repository, 'configuration repository', 256),
    base_ref: validateRef(config.base_ref, 'configuration base_ref'),
    writer_login: (() => {
      const login = nonEmptyString(config.writer_login, 'configuration writer_login', 256);
      if (!/^[A-Za-z0-9][A-Za-z0-9-]{0,99}\[bot\]$/.test(login)) fail('configuration writer_login is invalid');
      return login;
    })(),
    branch_prefix: branchPrefix,
    maximum_files: maximumFiles,
    maximum_changes: maximumChanges,
  });
}

function validateFile(file, index) {
  const name = `files[${index}]`;
  const hasPreviousFilename = Object.hasOwn(file ?? {}, 'previous_filename');
  exactKeys(file, hasPreviousFilename
    ? ['filename', 'status', 'additions', 'deletions', 'changes', 'previous_filename', 'mode', 'binary']
    : ['filename', 'status', 'additions', 'deletions', 'changes', 'mode', 'binary'], name);
  const previousFilename = hasPreviousFilename ? validRelativePath(file.previous_filename, `${name}.previous_filename`) : null;
  if (typeof file.mode !== 'string' || !/^[0-9]{6}$/.test(file.mode)) fail(`${name}.mode is invalid`);
  if (typeof file.binary !== 'boolean') fail(`${name}.binary must be boolean`);
  return Object.freeze({
    filename: validRelativePath(file.filename, `${name}.filename`),
    status: nonEmptyString(file.status, `${name}.status`, 32),
    additions: safeInteger(file.additions, `${name}.additions`),
    deletions: safeInteger(file.deletions, `${name}.deletions`),
    changes: safeInteger(file.changes, `${name}.changes`),
    previous_filename: previousFilename,
    mode: file.mode,
    binary: file.binary,
  });
}

function validateLabels(labels) {
  if (!Array.isArray(labels)) fail('snapshot labels must be an array');
  if (labels.length > MAXIMUM_LABELS) fail('snapshot labels exceed the policy maximum');
  const ids = new Set();
  const names = new Set();
  return Object.freeze(labels.map((label, index) => {
    const item = `labels[${index}]`;
    exactKeys(label, ['id', 'name'], item);
    const id = safeInteger(label.id, `${item}.id`);
    if (id === 0) fail(`${item}.id must be positive`);
    const name = nonEmptyString(label.name, `${item}.name`, 50);
    const foldedName = name.toLocaleLowerCase('en-US');
    if (ids.has(id)) fail('snapshot labels contain a duplicate id');
    if (names.has(foldedName)) fail('snapshot labels contain a duplicate name');
    ids.add(id);
    names.add(foldedName);
    return Object.freeze({ id, name });
  }));
}

function validateSnapshot(snapshot) {
  exactKeys(snapshot, ['repository', 'base', 'head', 'source', 'files', 'truncated', 'labels', 'labels_truncated'], 'PR snapshot');
  exactKeys(snapshot.base, ['ref', 'sha'], 'snapshot base');
  exactKeys(snapshot.head, ['ref', 'sha'], 'snapshot head');
  exactKeys(snapshot.source, ['author', 'branch', 'repository'], 'snapshot source');
  if (!Array.isArray(snapshot.files)) fail('snapshot files must be an array');
  if (typeof snapshot.truncated !== 'boolean') fail('snapshot truncated must be boolean');
  if (typeof snapshot.labels_truncated !== 'boolean') fail('snapshot labels_truncated must be boolean');
  return Object.freeze({
    repository: nonEmptyString(snapshot.repository, 'snapshot repository', 256),
    base: Object.freeze({ ref: validateRef(snapshot.base.ref, 'snapshot base.ref'), sha: validateSha(snapshot.base.sha, 'snapshot base.sha') }),
    head: Object.freeze({ ref: validBranch(snapshot.head.ref, 'snapshot head.ref'), sha: validateSha(snapshot.head.sha, 'snapshot head.sha') }),
    source: Object.freeze({
      author: nonEmptyString(snapshot.source.author, 'snapshot source.author', 256),
      branch: validBranch(snapshot.source.branch, 'snapshot source.branch'),
      repository: nonEmptyString(snapshot.source.repository, 'snapshot source.repository', 256),
    }),
    labels: validateLabels(snapshot.labels),
    labels_truncated: snapshot.labels_truncated,
    files: Object.freeze(snapshot.files.map(validateFile)),
    truncated: snapshot.truncated,
  });
}

function governed(path) {
  const folded = path.toLocaleLowerCase('en-US');
  return folded.startsWith('.github/') || GOVERNED_FILENAMES.has(folded);
}

function canaryDocument(path) {
  return /^docs\/automation-canary\/.+\.md$/.test(path);
}

function decisionFor(reasons) {
  const sorted = [...new Set(reasons)].sort();
  const deny = sorted.some((reason) => reason.startsWith('deny_'));
  return Object.freeze({ classification: deny ? 'deny' : sorted.length > 0 ? 'manual' : 'eligible', reasons: Object.freeze(sorted) });
}

/**
 * Classify a fully fetched PR snapshot without reading the network, filesystem, or clock.
 * Invalid contracts throw; complete snapshots that prove an unsafe or incomplete PR return deny.
 */
export function classifyAutonomyPolicy(snapshot, config) {
  const normalizedConfig = validateConfig(config);
  const normalized = validateSnapshot(snapshot);
  const reasons = [];
  const managedBranch = normalized.source.branch.startsWith(normalizedConfig.branch_prefix) &&
    /^[1-9][0-9]*$/.test(normalized.source.branch.slice(normalizedConfig.branch_prefix.length));
  const managed = managedBranch && normalized.source.repository === normalized.repository;

  if (normalized.repository !== normalizedConfig.repository) reasons.push('deny_repository_mismatch');
  if (normalized.base.ref !== normalizedConfig.base_ref) reasons.push('deny_base_ref_mismatch');
  if (normalized.head.ref !== normalized.source.branch) reasons.push('deny_head_branch_mismatch');
  if (normalized.truncated) reasons.push('deny_snapshot_truncated');
  if (normalized.labels_truncated) reasons.push('deny_labels_snapshot_truncated');
  if (!managedBranch) reasons.push('manual_unmanaged_branch');
  if (managedBranch && normalized.source.repository !== normalized.repository) reasons.push('manual_external_head_repository');
  if (managed && normalized.source.author !== normalizedConfig.writer_login) reasons.push('deny_untrusted_author');
  if (managed && normalized.files.length === 0) reasons.push('deny_empty_change_set');

  const labels = new Set(normalized.labels.map((label) => label.name.toLocaleLowerCase('en-US')));
  if (labels.has('do-not-merge')) reasons.push('deny_do_not_merge_label');
  if (managed && labels.has('autonomy-manual')) reasons.push('deny_autonomy_manual_label');

  const paths = new Set();
  const foldedPaths = new Set();
  let totalChanges = 0;
  for (const file of normalized.files) {
    totalChanges += file.changes;
    if (!managed) continue;
    const folded = file.filename.toLocaleLowerCase('en-US');
    if (paths.has(file.filename)) reasons.push('deny_duplicate_path');
    if (foldedPaths.has(folded) && !paths.has(file.filename)) reasons.push('deny_case_fold_conflict');
    paths.add(file.filename);
    foldedPaths.add(folded);
    if (governed(file.filename) || (file.previous_filename && governed(file.previous_filename))) reasons.push('deny_governed_path');
    if (!ALLOWED_FILE_STATUSES.has(file.status) || file.previous_filename !== null) reasons.push('deny_unsafe_file_status');
    if (file.mode !== '100644') reasons.push('deny_non_regular_mode');
    if (file.binary) reasons.push('deny_binary_file');
    if (!canaryDocument(file.filename)) reasons.push('manual_path_outside_allowlist');
  }
  if (managed && normalized.files.length > normalizedConfig.maximum_files) reasons.push('manual_file_limit_exceeded');
  if (managed && totalChanges > normalizedConfig.maximum_changes) reasons.push('manual_change_limit_exceeded');
  return decisionFor(reasons);
}
