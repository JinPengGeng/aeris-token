import crypto from 'node:crypto';

export const SYNC_CONFLICT_SCHEMA_VERSION = 1;
export const SYNC_CONFLICT_PROFILE = 'aeris-sync-conflict-v1';
export const MAX_CONFLICT_FILES = 4;
export const MAX_CONFLICT_FILE_BYTES = 16_384;
export const MAX_CONFLICT_INPUT_BYTES = 65_536;

const SHA = /^[0-9a-f]{40}$/;
const HASH = /^[0-9a-f]{64}$/;
const REPOSITORY = /^[A-Za-z0-9][A-Za-z0-9._-]*\/[A-Za-z0-9][A-Za-z0-9._-]*$/;
const REF = /^[A-Za-z0-9](?:[A-Za-z0-9._/-]{0,253}[A-Za-z0-9])?$/;
const MODEL = /^[A-Za-z0-9][A-Za-z0-9._:/+-]{0,255}$/;
const SAFE_PATH = /^(?!\/)(?!.*(?:^|\/)\.\.(?:\/|$))(?!.*\\)[^\u0000-\u001f\u007f]+$/u;
const CONFLICT_MARKER = /^(?:<{7}|={7}|>{7})(?: |$)/mu;

function reject(message) {
  throw new Error(message);
}

function object(value, name) {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) reject(`${name} must be an object`);
  return value;
}

function exactKeys(value, keys, name) {
  object(value, name);
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index])) {
    reject(`${name} fields are invalid`);
  }
}

function string(value, name, { pattern = null, maximum = 1024, allowEmpty = false } = {}) {
  if (typeof value !== 'string' || (!allowEmpty && value.length === 0) || value.length > maximum) {
    reject(`${name} is invalid`);
  }
  if (pattern && !pattern.test(value)) reject(`${name} has an invalid format`);
  return value;
}

function positive(value, name) {
  if (!Number.isSafeInteger(value) || value <= 0) reject(`${name} must be a positive integer`);
  return value;
}

function sha(value, name) {
  return string(value, name, { pattern: SHA, maximum: 40 });
}

function hash(value, name) {
  return string(value, name, { pattern: HASH, maximum: 64 });
}

function pathValue(value, name) {
  const result = string(value, name, { pattern: SAFE_PATH, maximum: 1024 });
  if (result === '.' || result.endsWith('/') || result.includes('//')) reject(`${name} is unsafe`);
  return result;
}

function content(value, name, { allowMarkers = false } = {}) {
  if (typeof value !== 'string' || Buffer.byteLength(value, 'utf8') > MAX_CONFLICT_FILE_BYTES) {
    reject(`${name} exceeds the text limit`);
  }
  if (value.includes('\u0000')) reject(`${name} contains a null byte`);
  if (!allowMarkers && CONFLICT_MARKER.test(value)) reject(`${name} contains unresolved conflict markers`);
  return value;
}

function canonicalize(value) {
  if (Array.isArray(value)) return value.map(canonicalize);
  if (value !== null && typeof value === 'object') {
    const result = {};
    for (const key of Object.keys(value).sort()) result[key] = canonicalize(value[key]);
    return result;
  }
  return value;
}

export function canonicalJson(value) {
  return JSON.stringify(canonicalize(value));
}

export function sha256(value) {
  return crypto.createHash('sha256').update(value).digest('hex');
}

export function artifactSha(value) {
  return sha256(canonicalJson(value));
}

function validateModel(value, name) {
  exactKeys(value, ['alias', 'id'], name);
  return Object.freeze({
    alias: string(value.alias, `${name} alias`, { pattern: MODEL, maximum: 128 }),
    id: string(value.id, `${name} id`, { pattern: MODEL, maximum: 256 }),
  });
}

function validateModelList(value, name) {
  if (!Array.isArray(value) || value.length < 1 || value.length > 2) reject(`${name} must contain one or two models`);
  const models = value.map((model, index) => validateModel(model, `${name} model ${index}`));
  const aliases = new Set(models.map((model) => model.alias));
  const ids = new Set(models.map((model) => model.id));
  if (aliases.size !== models.length || ids.size !== models.length) reject(`${name} contains duplicate models`);
  return Object.freeze(models);
}

export function validateModelCandidates(value) {
  exactKeys(value, ['resolver', 'reviewer'], 'conflict model candidates');
  const result = Object.freeze({
    resolver: validateModelList(value.resolver, 'resolver candidates'),
    reviewer: validateModelList(value.reviewer, 'reviewer candidates'),
  });
  const resolverIds = new Set(result.resolver.map((model) => model.id));
  if (result.reviewer.some((model) => resolverIds.has(model.id))) {
    reject('resolver and reviewer candidate model IDs must be disjoint');
  }
  return result;
}

function validateConflict(value, index) {
  const name = `conflict ${index}`;
  exactKeys(value, [
    'path', 'mode', 'base_blob_sha', 'ours_blob_sha', 'theirs_blob_sha',
    'base_content', 'ours_content', 'theirs_content', 'marker_content',
  ], name);
  if (value.mode !== '100644') reject(`${name} must keep regular non-executable mode 100644`);
  const result = Object.freeze({
    path: pathValue(value.path, `${name} path`),
    mode: '100644',
    base_blob_sha: sha(value.base_blob_sha, `${name} base blob SHA`),
    ours_blob_sha: sha(value.ours_blob_sha, `${name} ours blob SHA`),
    theirs_blob_sha: sha(value.theirs_blob_sha, `${name} theirs blob SHA`),
    base_content: content(value.base_content, `${name} base content`, { allowMarkers: true }),
    ours_content: content(value.ours_content, `${name} ours content`, { allowMarkers: true }),
    theirs_content: content(value.theirs_content, `${name} theirs content`, { allowMarkers: true }),
    marker_content: content(value.marker_content, `${name} marker content`, { allowMarkers: true }),
  });
  if (!CONFLICT_MARKER.test(result.marker_content)) reject(`${name} marker content does not contain a content conflict`);
  return result;
}

function validateConflicts(value) {
  if (!Array.isArray(value) || value.length < 1 || value.length > MAX_CONFLICT_FILES) {
    reject('conflict manifest file count is invalid');
  }
  const conflicts = value.map(validateConflict);
  const sorted = [...conflicts].sort((left, right) => left.path.localeCompare(right.path, 'en-US'));
  if (conflicts.some((entry, index) => entry.path !== sorted[index].path)) reject('conflict manifest is not sorted');
  const paths = new Set();
  const folded = new Set();
  for (const conflict of conflicts) {
    const key = conflict.path.toLocaleLowerCase('en-US');
    if (paths.has(conflict.path) || folded.has(key)) reject('conflict manifest contains duplicate or case-ambiguous paths');
    paths.add(conflict.path);
    folded.add(key);
  }
  if (Buffer.byteLength(canonicalJson(conflicts), 'utf8') > MAX_CONFLICT_INPUT_BYTES) {
    reject('conflict manifest exceeds the total input limit');
  }
  return Object.freeze(conflicts);
}

export function conflictManifestSha(conflicts) {
  return artifactSha(validateConflicts(conflicts));
}

export function conflictGeneration(bundle) {
  return Object.freeze({
    schema_version: SYNC_CONFLICT_SCHEMA_VERSION,
    profile: SYNC_CONFLICT_PROFILE,
    repository_id: bundle.repository_id,
    base_sha: bundle.base_sha,
    checkpoint_sha: bundle.checkpoint_sha,
    upstream_sha: bundle.upstream.sha,
    workflow_sha: bundle.policy.workflow_sha,
    policy_path: bundle.policy.policy_path,
    policy_blob_sha: bundle.policy.policy_blob_sha,
    state_path: bundle.policy.state_path,
    state_blob_sha: bundle.policy.state_blob_sha,
    synthetic_commit_sha: bundle.merge.synthetic_commit_sha,
    conflict_tree_sha: bundle.merge.conflict_tree_sha,
    manifest_sha: bundle.merge.manifest_sha,
    model_candidates_sha: bundle.model_candidates_sha,
    resolver_prompt_sha: bundle.prompts.resolver_sha,
    reviewer_prompt_sha: bundle.prompts.reviewer_sha,
  });
}

export function validateConflictBundle(value) {
  exactKeys(value, [
    'schema_version', 'artifact_type', 'profile', 'repository', 'repository_id',
    'base_ref', 'sync_ref', 'base_sha', 'checkpoint_sha', 'upstream', 'policy',
    'merge', 'prompts', 'model_candidates', 'model_candidates_sha', 'conflicts',
    'generation_sha',
  ], 'conflict bundle');
  if (value.schema_version !== 1 || value.artifact_type !== 'sync_conflict_bundle' || value.profile !== SYNC_CONFLICT_PROFILE) {
    reject('conflict bundle version, type, or profile is invalid');
  }
  exactKeys(value.upstream, ['repository', 'ref', 'sha'], 'conflict bundle upstream');
  exactKeys(value.policy, ['workflow_sha', 'policy_path', 'policy_blob_sha', 'state_path', 'state_blob_sha'], 'conflict bundle policy');
  exactKeys(value.merge, ['synthetic_commit_sha', 'conflict_tree_sha', 'manifest_sha'], 'conflict bundle merge');
  exactKeys(value.prompts, ['resolver_sha', 'reviewer_sha'], 'conflict bundle prompts');
  const models = validateModelCandidates(value.model_candidates);
  const conflicts = validateConflicts(value.conflicts);
  const result = {
    schema_version: 1,
    artifact_type: 'sync_conflict_bundle',
    profile: SYNC_CONFLICT_PROFILE,
    repository: string(value.repository, 'conflict repository', { pattern: REPOSITORY, maximum: 256 }),
    repository_id: positive(value.repository_id, 'conflict repository ID'),
    base_ref: string(value.base_ref, 'conflict base ref', { pattern: REF, maximum: 255 }),
    sync_ref: string(value.sync_ref, 'conflict sync ref', { pattern: REF, maximum: 255 }),
    base_sha: sha(value.base_sha, 'conflict base SHA'),
    checkpoint_sha: sha(value.checkpoint_sha, 'conflict checkpoint SHA'),
    upstream: Object.freeze({
      repository: string(value.upstream.repository, 'upstream repository', { pattern: REPOSITORY, maximum: 256 }),
      ref: string(value.upstream.ref, 'upstream ref', { pattern: REF, maximum: 255 }),
      sha: sha(value.upstream.sha, 'upstream SHA'),
    }),
    policy: Object.freeze({
      workflow_sha: sha(value.policy.workflow_sha, 'trusted workflow SHA'),
      policy_path: pathValue(value.policy.policy_path, 'sync policy path'),
      policy_blob_sha: sha(value.policy.policy_blob_sha, 'sync policy blob SHA'),
      state_path: pathValue(value.policy.state_path, 'sync state path'),
      state_blob_sha: sha(value.policy.state_blob_sha, 'sync state blob SHA'),
    }),
    merge: Object.freeze({
      synthetic_commit_sha: sha(value.merge.synthetic_commit_sha, 'synthetic commit SHA'),
      conflict_tree_sha: sha(value.merge.conflict_tree_sha, 'conflict tree SHA'),
      manifest_sha: hash(value.merge.manifest_sha, 'conflict manifest hash'),
    }),
    prompts: Object.freeze({
      resolver_sha: hash(value.prompts.resolver_sha, 'resolver prompt hash'),
      reviewer_sha: hash(value.prompts.reviewer_sha, 'reviewer prompt hash'),
    }),
    model_candidates: models,
    model_candidates_sha: hash(value.model_candidates_sha, 'model candidates hash'),
    conflicts,
    generation_sha: hash(value.generation_sha, 'conflict generation hash'),
  };
  if (result.policy.workflow_sha !== result.base_sha) reject('trusted workflow SHA must equal the fork base SHA');
  if (result.merge.manifest_sha !== artifactSha(conflicts)) reject('conflict manifest hash is invalid');
  if (result.model_candidates_sha !== artifactSha(models)) reject('model candidates hash is invalid');
  if (result.generation_sha !== artifactSha(conflictGeneration(result))) reject('conflict generation hash is invalid');
  return Object.freeze(result);
}

function validateResolution(value, index) {
  exactKeys(value, ['path', 'content'], `resolution ${index}`);
  return Object.freeze({
    path: pathValue(value.path, `resolution ${index} path`),
    content: content(value.content, `resolution ${index} content`),
  });
}

function validateRun(value, name) {
  exactKeys(value, ['id', 'attempt'], name);
  return Object.freeze({ id: positive(value.id, `${name} id`), attempt: positive(value.attempt, `${name} attempt`) });
}

export function validateResolverOutput(value, bundle) {
  exactKeys(value, ['schema_version', 'verdict', 'summary', 'resolutions'], 'resolver output');
  if (value.schema_version !== 1 || value.verdict !== 'resolved') reject('resolver did not return a resolved verdict');
  const summary = string(value.summary, 'resolver summary', { maximum: 2000 });
  if (!Array.isArray(value.resolutions) || value.resolutions.length !== bundle.conflicts.length) {
    reject('resolver output does not cover every conflict exactly once');
  }
  const resolutions = value.resolutions.map(validateResolution);
  const expectedPaths = bundle.conflicts.map((entry) => entry.path);
  if (resolutions.some((entry, index) => entry.path !== expectedPaths[index])) {
    reject('resolver paths do not exactly match the sorted conflict manifest');
  }
  return Object.freeze({ schema_version: 1, verdict: 'resolved', summary, resolutions: Object.freeze(resolutions) });
}

export function validateConflictCandidate(value, bundleValue) {
  const bundle = validateConflictBundle(bundleValue);
  exactKeys(value, [
    'schema_version', 'artifact_type', 'profile', 'bundle_sha', 'generation_sha',
    'model', 'run', 'output', 'resolution_sha',
  ], 'conflict candidate');
  if (value.schema_version !== 1 || value.artifact_type !== 'sync_conflict_candidate' || value.profile !== SYNC_CONFLICT_PROFILE) {
    reject('conflict candidate version, type, or profile is invalid');
  }
  const model = validateModel(value.model, 'resolver model');
  if (!bundle.model_candidates.resolver.some((candidate) => candidate.alias === model.alias && candidate.id === model.id)) {
    reject('resolver model is not allowed by the exact conflict generation');
  }
  const output = validateResolverOutput(value.output, bundle);
  const result = Object.freeze({
    schema_version: 1,
    artifact_type: 'sync_conflict_candidate',
    profile: SYNC_CONFLICT_PROFILE,
    bundle_sha: hash(value.bundle_sha, 'candidate bundle hash'),
    generation_sha: hash(value.generation_sha, 'candidate generation hash'),
    model,
    run: validateRun(value.run, 'resolver run'),
    output,
    resolution_sha: hash(value.resolution_sha, 'resolution hash'),
  });
  if (result.bundle_sha !== artifactSha(bundle)) reject('candidate bundle hash does not match');
  if (result.generation_sha !== bundle.generation_sha) reject('candidate generation does not match');
  if (result.resolution_sha !== artifactSha(output.resolutions)) reject('candidate resolution hash is invalid');
  return result;
}

export function reviewGeneration(input) {
  return Object.freeze({
    schema_version: 1,
    profile: SYNC_CONFLICT_PROFILE,
    repository_id: input.repository_id,
    pull_number: input.pull_number,
    head_sha: input.head_sha,
    head_tree_sha: input.head_tree_sha,
    base_sha: input.base_sha,
    bundle_sha: input.bundle_sha,
    candidate_sha: input.candidate_sha,
    conflict_generation_sha: input.conflict_generation_sha,
    resolution_sha: input.resolution_sha,
    resolved_merge_tree_sha: input.resolved_merge_tree_sha,
    reviewer_candidates_sha: artifactSha(input.reviewer_candidates),
    reviewer_prompt_sha: input.reviewer_prompt_sha,
  });
}

export function validateReviewInput(value, bundleValue, candidateValue) {
  const bundle = validateConflictBundle(bundleValue);
  const candidate = validateConflictCandidate(candidateValue, bundle);
  exactKeys(value, [
    'schema_version', 'artifact_type', 'profile', 'repository', 'repository_id',
    'pull_number', 'head_sha', 'head_tree_sha', 'base_sha', 'bundle_sha',
    'candidate_sha', 'conflict_generation_sha', 'resolution_sha',
    'resolved_merge_tree_sha', 'resolver_model', 'reviewer_candidates',
    'reviewer_prompt_sha', 'conflicts', 'resolutions', 'input_sha',
    'review_generation_sha',
  ], 'conflict review input');
  if (value.schema_version !== 1 || value.artifact_type !== 'sync_conflict_review_input' || value.profile !== SYNC_CONFLICT_PROFILE) {
    reject('conflict review input version, type, or profile is invalid');
  }
  const reviewerCandidates = validateModelList(value.reviewer_candidates, 'review input reviewer candidates');
  const conflicts = validateConflicts(value.conflicts);
  const resolutions = value.resolutions.map(validateResolution);
  const result = {
    schema_version: 1,
    artifact_type: 'sync_conflict_review_input',
    profile: SYNC_CONFLICT_PROFILE,
    repository: string(value.repository, 'review repository', { pattern: REPOSITORY, maximum: 256 }),
    repository_id: positive(value.repository_id, 'review repository ID'),
    pull_number: positive(value.pull_number, 'review pull number'),
    head_sha: sha(value.head_sha, 'review head SHA'),
    head_tree_sha: sha(value.head_tree_sha, 'review head tree SHA'),
    base_sha: sha(value.base_sha, 'review base SHA'),
    bundle_sha: hash(value.bundle_sha, 'review bundle hash'),
    candidate_sha: hash(value.candidate_sha, 'review candidate hash'),
    conflict_generation_sha: hash(value.conflict_generation_sha, 'review conflict generation hash'),
    resolution_sha: hash(value.resolution_sha, 'review resolution hash'),
    resolved_merge_tree_sha: sha(value.resolved_merge_tree_sha, 'review resolved merge tree SHA'),
    resolver_model: validateModel(value.resolver_model, 'review resolver model'),
    reviewer_candidates: reviewerCandidates,
    reviewer_prompt_sha: hash(value.reviewer_prompt_sha, 'reviewer prompt hash'),
    conflicts,
    resolutions: Object.freeze(resolutions),
    input_sha: hash(value.input_sha, 'review input hash'),
    review_generation_sha: hash(value.review_generation_sha, 'review generation hash'),
  };
  if (result.repository !== bundle.repository || result.repository_id !== bundle.repository_id || result.base_sha !== bundle.base_sha) {
    reject('review repository or base does not match the conflict bundle');
  }
  if (result.bundle_sha !== artifactSha(bundle) || result.candidate_sha !== artifactSha(candidate)) reject('review artifact binding is invalid');
  if (result.conflict_generation_sha !== bundle.generation_sha || result.resolution_sha !== candidate.resolution_sha) {
    reject('review conflict generation or resolution binding is invalid');
  }
  if (canonicalJson(result.resolver_model) !== canonicalJson(candidate.model)) reject('review resolver model binding is invalid');
  if (canonicalJson(result.reviewer_candidates) !== canonicalJson(bundle.model_candidates.reviewer)) reject('reviewer candidates drifted');
  if (result.reviewer_candidates.some((model) => model.id === result.resolver_model.id)) reject('reviewer model must be independent from the resolver model');
  if (canonicalJson(conflicts) !== canonicalJson(bundle.conflicts) || canonicalJson(resolutions) !== canonicalJson(candidate.output.resolutions)) {
    reject('review input coverage is not exact');
  }
  const inputPayload = { conflicts: result.conflicts, resolutions: result.resolutions, resolved_merge_tree_sha: result.resolved_merge_tree_sha };
  if (result.input_sha !== artifactSha(inputPayload)) reject('review input hash is invalid');
  if (result.review_generation_sha !== artifactSha(reviewGeneration(result))) reject('review generation hash is invalid');
  return Object.freeze(result);
}

export function validateReviewerOutput(value) {
  exactKeys(value, ['schema_version', 'verdict', 'summary', 'findings'], 'reviewer output');
  if (value.schema_version !== 1 || !['pass', 'fail'].includes(value.verdict)) reject('reviewer verdict is invalid');
  const summary = string(value.summary, 'reviewer summary', { maximum: 2000 });
  if (!Array.isArray(value.findings) || value.findings.length > 20) reject('reviewer findings are invalid');
  const findings = value.findings.map((finding, index) => {
    exactKeys(finding, ['severity', 'path', 'details'], `reviewer finding ${index}`);
    if (!['critical', 'high', 'medium', 'low'].includes(finding.severity)) reject(`reviewer finding ${index} severity is invalid`);
    return Object.freeze({
      severity: finding.severity,
      path: finding.path === null ? null : pathValue(finding.path, `reviewer finding ${index} path`),
      details: string(finding.details, `reviewer finding ${index} details`, { maximum: 2000 }),
    });
  });
  if ((value.verdict === 'pass') !== (findings.length === 0)) reject('reviewer verdict and findings disagree');
  return Object.freeze({ schema_version: 1, verdict: value.verdict, summary, findings: Object.freeze(findings) });
}

export function validateReviewReceipt(value, inputValue, bundleValue, candidateValue) {
  const input = validateReviewInput(inputValue, bundleValue, candidateValue);
  exactKeys(value, [
    'schema_version', 'artifact_type', 'profile', 'review_generation_sha',
    'input_sha', 'model', 'run', 'output', 'output_sha', 'coverage',
  ], 'conflict review receipt');
  if (value.schema_version !== 1 || value.artifact_type !== 'sync_conflict_review' || value.profile !== SYNC_CONFLICT_PROFILE) {
    reject('conflict review receipt version, type, or profile is invalid');
  }
  exactKeys(value.coverage, ['complete', 'conflict_count', 'input_bytes'], 'review coverage');
  const model = validateModel(value.model, 'reviewer model');
  if (!input.reviewer_candidates.some((candidate) => candidate.alias === model.alias && candidate.id === model.id)) {
    reject('reviewer model is not allowed by the exact review generation');
  }
  if (model.id === input.resolver_model.id) reject('reviewer used the resolver model');
  const output = validateReviewerOutput(value.output);
  const result = Object.freeze({
    schema_version: 1,
    artifact_type: 'sync_conflict_review',
    profile: SYNC_CONFLICT_PROFILE,
    review_generation_sha: hash(value.review_generation_sha, 'receipt review generation hash'),
    input_sha: hash(value.input_sha, 'receipt input hash'),
    model,
    run: validateRun(value.run, 'reviewer run'),
    output,
    output_sha: hash(value.output_sha, 'review output hash'),
    coverage: Object.freeze({
      complete: value.coverage.complete,
      conflict_count: positive(value.coverage.conflict_count, 'review conflict count'),
      input_bytes: positive(value.coverage.input_bytes, 'review input bytes'),
    }),
  });
  if (result.review_generation_sha !== input.review_generation_sha || result.input_sha !== input.input_sha) reject('review receipt generation is stale');
  if (result.output_sha !== artifactSha(output)) reject('review output hash is invalid');
  if (result.coverage.complete !== true || result.coverage.conflict_count !== input.conflicts.length) reject('review coverage is incomplete');
  if (result.coverage.input_bytes !== Buffer.byteLength(canonicalJson({ conflicts: input.conflicts, resolutions: input.resolutions }), 'utf8')) {
    reject('review coverage byte count is invalid');
  }
  if (output.verdict !== 'pass' || output.findings.length !== 0) reject('independent conflict review did not approve the resolution');
  return result;
}

export function validateFinalAttestation(value) {
  exactKeys(value, [
    'schema_version', 'artifact_type', 'profile', 'repository', 'repository_id',
    'pull_number', 'head_sha', 'head_tree_sha', 'base_sha', 'checkpoint_sha',
    'upstream_repository', 'upstream_ref', 'upstream_sha', 'policy_sha',
    'policy_blob_sha', 'bundle_sha', 'candidate_sha', 'review_input_sha',
    'review_receipt_sha', 'conflict_generation_sha', 'review_generation_sha',
    'resolution_sha', 'resolved_merge_tree_sha', 'resolver_model', 'reviewer_model',
    'verifier_run', 'decision',
  ], 'conflict final attestation');
  if (value.schema_version !== 1 || value.artifact_type !== 'sync_conflict_attestation' || value.profile !== SYNC_CONFLICT_PROFILE || value.decision !== 'approved') {
    reject('conflict final attestation identity or decision is invalid');
  }
  return Object.freeze({
    schema_version: 1,
    artifact_type: 'sync_conflict_attestation',
    profile: SYNC_CONFLICT_PROFILE,
    repository: string(value.repository, 'attestation repository', { pattern: REPOSITORY, maximum: 256 }),
    repository_id: positive(value.repository_id, 'attestation repository ID'),
    pull_number: positive(value.pull_number, 'attestation pull number'),
    head_sha: sha(value.head_sha, 'attestation head SHA'),
    head_tree_sha: sha(value.head_tree_sha, 'attestation head tree SHA'),
    base_sha: sha(value.base_sha, 'attestation base SHA'),
    checkpoint_sha: sha(value.checkpoint_sha, 'attestation checkpoint SHA'),
    upstream_repository: string(value.upstream_repository, 'attestation upstream repository', { pattern: REPOSITORY, maximum: 256 }),
    upstream_ref: string(value.upstream_ref, 'attestation upstream ref', { pattern: REF, maximum: 255 }),
    upstream_sha: sha(value.upstream_sha, 'attestation upstream SHA'),
    policy_sha: sha(value.policy_sha, 'attestation policy SHA'),
    policy_blob_sha: sha(value.policy_blob_sha, 'attestation policy blob SHA'),
    bundle_sha: hash(value.bundle_sha, 'attestation bundle hash'),
    candidate_sha: hash(value.candidate_sha, 'attestation candidate hash'),
    review_input_sha: hash(value.review_input_sha, 'attestation review input artifact hash'),
    review_receipt_sha: hash(value.review_receipt_sha, 'attestation review receipt artifact hash'),
    conflict_generation_sha: hash(value.conflict_generation_sha, 'attestation conflict generation hash'),
    review_generation_sha: hash(value.review_generation_sha, 'attestation review generation hash'),
    resolution_sha: hash(value.resolution_sha, 'attestation resolution hash'),
    resolved_merge_tree_sha: sha(value.resolved_merge_tree_sha, 'attestation resolved merge tree SHA'),
    resolver_model: validateModel(value.resolver_model, 'attestation resolver model'),
    reviewer_model: validateModel(value.reviewer_model, 'attestation reviewer model'),
    verifier_run: validateRun(value.verifier_run, 'verifier run'),
    decision: 'approved',
  });
}

export function parseCanonicalJson(text, name = 'JSON artifact') {
  if (typeof text !== 'string' || Buffer.byteLength(text, 'utf8') > 2 * 1024 * 1024) reject(`${name} is too large`);
  let value;
  try { value = JSON.parse(text); } catch { reject(`${name} is invalid JSON`); }
  if (canonicalJson(value) !== text.trim()) reject(`${name} is not canonical JSON`);
  return value;
}
