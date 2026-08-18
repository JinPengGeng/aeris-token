const SHA = /^[0-9a-f]{40}$/;
const SAFE_REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const APP_SLUG = /^[a-z0-9](?:[a-z0-9-]{0,98}[a-z0-9])?$/;
const ALLOWED_MODES = new Set(['shadow', 'human']);
const ALLOWED_FILE_STATUSES = new Set(['added', 'modified', 'removed', 'renamed']);

function fail(message) {
  throw new Error(message);
}

function requireCondition(condition, message) {
  if (!condition) fail(message);
}

function uniqueStrings(values, name, maximumItems = 100) {
  requireCondition(Array.isArray(values) && values.length <= maximumItems, `${name} is invalid`);
  requireCondition(values.every((value) => typeof value === 'string' && value.length > 0), `${name} is invalid`);
  requireCondition(new Set(values).size === values.length, `${name} must not contain duplicates`);
  return values;
}

function normalizedPath(value, name) {
  requireCondition(typeof value === 'string' && value.length > 0 && value.length <= 1024, `${name} is invalid`);
  requireCondition(!value.includes('\\') && !value.startsWith('/') && !value.endsWith('/'), `${name} is invalid`);
  requireCondition(!/[\u0000-\u001f\u007f]/.test(value), `${name} is invalid`);
  const segments = value.split('/');
  requireCondition(segments.every((segment) => segment.length > 0 && segment !== '.' && segment !== '..'), `${name} is invalid`);
  return value;
}

function normalizedPattern(value) {
  requireCondition(typeof value === 'string' && value.length > 0 && value.length <= 256, 'path pattern is invalid');
  requireCondition(!value.includes('\\') && !value.startsWith('/') && !/[\u0000-\u001f\u007f]/.test(value), 'path pattern is invalid');
  const segments = value.split('/');
  requireCondition(segments.every((segment) => segment.length > 0 && segment !== '.' && segment !== '..'), 'path pattern is invalid');
  requireCondition(!/\*{3,}/.test(value), 'path pattern is invalid');
  return value;
}

function escapeRegex(character) {
  return /[\\^$.*+?()[\]{}|]/.test(character) ? `\\${character}` : character;
}

export function pathMatchesPattern(path, pattern) {
  const candidate = normalizedPath(path, 'changed path');
  const source = normalizedPattern(pattern);
  let expression = '^';
  for (let index = 0; index < source.length; index += 1) {
    const character = source[index];
    if (character === '*' && source[index + 1] === '*') {
      index += 1;
      if (source[index + 1] === '/') {
        index += 1;
        expression += '(?:[^/]+/)*';
      } else {
        expression += '.*';
      }
    } else if (character === '*') {
      expression += '[^/]*';
    } else if (character === '?') {
      expression += '[^/]';
    } else {
      expression += escapeRegex(character);
    }
  }
  expression += '$';
  return new RegExp(expression, 'u').test(candidate);
}

function changedPaths(files) {
  requireCondition(files && typeof files === 'object' && !Array.isArray(files), 'pull files response is invalid');
  requireCondition(files.truncated === false, 'pull files response is truncated');
  requireCondition(Array.isArray(files.files) && files.files.length > 0 && files.files.length <= 300, 'pull files are invalid');
  const paths = [];
  const deletedPaths = [];
  for (const [index, file] of files.files.entries()) {
    requireCondition(file && typeof file === 'object' && !Array.isArray(file), `pull file ${index} is invalid`);
    requireCondition(
      Object.keys(file).length === 3 && ['filename', 'status', 'previous_filename'].every((key) => Object.hasOwn(file, key)),
      `pull file ${index} has unexpected keys`,
    );
    requireCondition(ALLOWED_FILE_STATUSES.has(file.status), `pull file ${index} status is invalid`);
    const filename = normalizedPath(file.filename, `pull file ${index} filename`);
    paths.push(filename);
    if (file.status === 'removed') deletedPaths.push(filename);
    if (file.status === 'renamed') {
      const previous = normalizedPath(file.previous_filename, `pull file ${index} previous filename`);
      requireCondition(previous !== filename, `pull file ${index} rename endpoints are identical`);
      paths.push(previous);
    } else {
      requireCondition(file.previous_filename === null, `pull file ${index} previous filename is invalid`);
    }
  }
  return {
    paths: [...new Set(paths)].sort(),
    deletedPaths: [...new Set(deletedPaths)].sort(),
    fileCount: files.files.length,
  };
}

function labelsFromPull(pull) {
  requireCondition(Array.isArray(pull.labels) && pull.labels.length <= 100, 'pull labels are invalid');
  const labels = pull.labels.map((label) => (typeof label === 'string' ? label : label?.name));
  requireCondition(labels.every((label) => typeof label === 'string' && label.length > 0 && label.length <= 100), 'pull labels are invalid');
  return new Set(labels);
}

function policyConfiguration(policy) {
  const gate = policy?.policy_gate;
  requireCondition(gate && typeof gate === 'object' && !Array.isArray(gate), 'policy gate configuration is missing');
  requireCondition(ALLOWED_MODES.has(gate.mode), 'policy gate mode is invalid');
  requireCondition(typeof gate.check_name === 'string' && gate.check_name.length > 0, 'policy check name is invalid');
  requireCondition(gate.require_exact_head_sha === true, 'policy gate must require an exact head SHA');
  requireCondition(gate.require_base_up_to_date === true, 'policy gate must require an up-to-date base');
  requireCondition(gate.require_conversation_resolution === true, 'policy gate must require resolved conversations');
  const requiredChecks = uniqueStrings(gate.required_checks, 'required checks', 20);
  requireCondition(
    Array.isArray(gate.required_check_sources) && gate.required_check_sources.length === requiredChecks.length,
    'required check sources are invalid',
  );
  const requiredCheckSources = gate.required_check_sources.map((source, index) => {
    requireCondition(source && typeof source === 'object' && !Array.isArray(source), 'required check source is invalid');
    requireCondition(
      Object.keys(source).length === 3 && ['context', 'app_id', 'app_slug'].every((key) => Object.hasOwn(source, key)),
      'required check source has unexpected keys',
    );
    requireCondition(source.context === requiredChecks[index], 'required check source order does not match required checks');
    requireCondition(Number.isSafeInteger(source.app_id) && source.app_id > 0, 'required check source App ID is invalid');
    requireCondition(typeof source.app_slug === 'string' && APP_SLUG.test(source.app_slug), 'required check source App slug is invalid');
    return source;
  });
  const humanPatterns = uniqueStrings(gate.always_require_human_review, 'human-review patterns', 100)
    .map(normalizedPattern);
  const allowlistPatterns = uniqueStrings(gate.allowlist_paths, 'allowlist patterns', 100)
    .map(normalizedPattern);
  requireCondition(
    typeof gate.human_enable_label === 'string' && gate.human_enable_label.length > 0 && gate.human_enable_label.length <= 100,
    'policy approval label is invalid',
  );
  const baseRef = policy?.trusted_source?.ref;
  requireCondition(typeof baseRef === 'string' && /^refs\/heads\/[A-Za-z0-9._/-]+$/.test(baseRef), 'trusted policy ref is invalid');
  return {
    gate,
    requiredChecks,
    requiredCheckSources,
    humanPatterns,
    allowlistPatterns,
    baseBranch: baseRef.slice('refs/heads/'.length),
  };
}

function evaluateTrustedRequiredChecks(requiredChecks, sources, headSha, checkRuns) {
  requireCondition(Array.isArray(checkRuns) && checkRuns.length <= 1000, 'check runs are invalid');
  const statuses = new Set(['queued', 'in_progress', 'completed', 'waiting', 'requested', 'pending']);
  const conclusions = new Set(['action_required', 'cancelled', 'failure', 'neutral', 'success', 'skipped', 'stale', 'startup_failure', 'timed_out']);
  const unsuccessful = [];
  for (let index = 0; index < requiredChecks.length; index += 1) {
    const context = requiredChecks[index];
    const source = sources[index];
    const trusted = checkRuns.filter((check) => (
      check?.name === context &&
      check?.head_sha === headSha &&
      check?.app?.id === source.app_id &&
      check?.app?.slug === source.app_slug
    ));
    for (const check of trusted) {
      requireCondition(check && typeof check === 'object' && !Array.isArray(check), 'trusted check run is invalid');
      requireCondition(Number.isSafeInteger(check.id) && check.id > 0, 'trusted check run ID is invalid');
      requireCondition(check.name === context, 'trusted check run name is invalid');
      requireCondition(check.head_sha === headSha, 'trusted check run head SHA is invalid');
      requireCondition(check.app && check.app.id === source.app_id && check.app.slug === source.app_slug, 'trusted check run App identity is invalid');
      requireCondition(typeof check.status === 'string' && statuses.has(check.status), 'trusted check run status is invalid');
      requireCondition(check.conclusion === null || (typeof check.conclusion === 'string' && conclusions.has(check.conclusion)), 'trusted check run conclusion is invalid');
      requireCondition(check.status === 'completed' ? check.conclusion !== null : check.conclusion === null, 'trusted check run status and conclusion are inconsistent');
    }
    trusted.sort((left, right) => right.id - left.id);
    const latest = trusted[0];
    if (!latest || latest.status !== 'completed' || latest.conclusion !== 'success') unsuccessful.push(context);
  }
  return { ready: unsuccessful.length === 0, unsuccessful };
}

function matchedPaths(paths, patterns) {
  return paths.filter((path) => patterns.some((pattern) => pathMatchesPattern(path, pattern)));
}

export function evaluatePolicyGate({
  policy,
  repository,
  pull,
  files,
  checkRuns,
  comparison,
  reviewThreads,
  expectedHeadSha = null,
}) {
  requireCondition(typeof repository === 'string' && SAFE_REPOSITORY.test(repository), 'repository is invalid');
  const { gate, requiredChecks, requiredCheckSources, humanPatterns, baseBranch } = policyConfiguration(policy);
  requireCondition(pull && typeof pull === 'object' && !Array.isArray(pull), 'pull request is invalid');
  requireCondition(Number.isSafeInteger(pull.number) && pull.number > 0, 'pull request number is invalid');
  requireCondition(typeof pull.head?.sha === 'string' && SHA.test(pull.head.sha), 'pull head SHA is invalid');
  requireCondition(typeof pull.base?.sha === 'string' && SHA.test(pull.base.sha), 'pull base SHA is invalid');
  requireCondition(Array.isArray(checkRuns), 'check signals are invalid');
  requireCondition(reviewThreads && typeof reviewThreads === 'object' && !Array.isArray(reviewThreads), 'review threads are invalid');
  requireCondition(Number.isSafeInteger(reviewThreads.unresolved) && reviewThreads.unresolved >= 0, 'review thread count is invalid');
  requireCondition(reviewThreads.truncated === false, 'review threads are truncated');

  const changes = changedPaths(files);
  const paths = changes.paths;
  const labels = labelsFromPull(pull);
  const reasons = [];
  const blocking = [];
  const pending = [];
  const human = [];
  const add = (bucket, reason) => {
    if (!reasons.includes(reason)) reasons.push(reason);
    if (!bucket.includes(reason)) bucket.push(reason);
  };

  if (pull.state !== 'open') add(blocking, 'pull_request_not_open');
  if (pull.draft !== false) add(blocking, 'draft_pull_request');
  if (pull.base.ref !== baseBranch) add(blocking, 'base_branch_mismatch');
  if (pull.base.repo?.full_name !== repository) add(blocking, 'base_repository_mismatch');
  if (pull.head.repo?.full_name !== repository) add(blocking, 'head_repository_mismatch');
  if (expectedHeadSha !== null && pull.head.sha !== expectedHeadSha) add(blocking, 'stale_head_sha');

  if (pull.mergeable === null || pull.mergeable === undefined) add(pending, 'mergeability_pending');
  else if (pull.mergeable !== true) add(blocking, 'merge_conflict');

  if (!comparison || typeof comparison !== 'object' || comparison.base_sha !== pull.base.sha || comparison.head_sha !== pull.head.sha) {
    add(blocking, 'comparison_binding_mismatch');
  } else if (comparison.status === 'unknown' || comparison.status === null) {
    add(pending, 'base_comparison_pending');
  } else if (!['ahead', 'identical'].includes(comparison.status)) {
    add(blocking, 'base_not_up_to_date');
  }

  const checks = evaluateTrustedRequiredChecks(requiredChecks, requiredCheckSources, pull.head.sha, checkRuns);
  if (!checks.ready) add(pending, 'required_checks_not_successful');
  if (reviewThreads.unresolved > 0) add(blocking, 'unresolved_review_threads');

  const sensitivePaths = matchedPaths(paths, humanPatterns);
  const humanReviewPaths = [...new Set([...sensitivePaths, ...changes.deletedPaths])].sort();
  if (humanReviewPaths.length > 0) add(human, 'human_review_path_changed');
  if (changes.deletedPaths.length > 0) add(human, 'file_deleted');

  if (gate.mode === 'human') add(human, 'human_mode_requires_manual_merge');
  void labels;

  let verdict = 'pass';
  if (blocking.length > 0) verdict = 'block';
  else if (pending.length > 0) verdict = 'pending';
  else if (human.length > 0) verdict = 'human_required';

  const enforcement = 'advisory';
  return Object.freeze({
    mode: gate.mode,
    verdict,
    enforcement,
    eligible_for_automatic_merge: false,
    reason_codes: Object.freeze(reasons),
    unsuccessful_checks: Object.freeze([...checks.unsuccessful]),
    human_review_paths: Object.freeze(humanReviewPaths),
    changed_file_count: changes.fileCount,
  });
}

export function policyCheckConclusion(result) {
  requireCondition(result && typeof result === 'object', 'policy result is invalid');
  if (result.mode === 'shadow') return 'neutral';
  if (result.verdict === 'pending' || result.verdict === 'block') return 'failure';
  return 'success';
}
