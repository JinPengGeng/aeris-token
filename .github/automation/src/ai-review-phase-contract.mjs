import {
  REVIEW_ATTESTATION_ROLES,
  canonicalJson,
  sha256,
  validateReviewGeneration,
} from './review-attestation-contract.mjs';

export const AI_REVIEW_ARTIFACT_SCHEMA_VERSION = 1;
export const MAX_AI_REVIEW_ARTIFACT_BYTES = 16 * 1024 * 1024;

const HASH = /^[0-9a-f]{64}$/;
const SAFE = /^[A-Za-z0-9][A-Za-z0-9._:/-]*$/;
const TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?Z$/;

function requireCondition(condition, message) { if (!condition) throw new Error(message); }
function object(value, name) { requireCondition(value && typeof value === 'object' && !Array.isArray(value), `${name} must be an object`); return value; }
function exactKeys(value, keys, name) {
  object(value, name);
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  requireCondition(actual.length === expected.length && actual.every((key, index) => key === expected[index]), `${name} has unexpected keys`);
}
function string(value, name, maximum, pattern = null) {
  requireCondition(typeof value === 'string' && value.length > 0 && value.length <= maximum && !/[\u0000-\u001f\u007f]/.test(value), `${name} is invalid`);
  if (pattern) requireCondition(pattern.test(value), `${name} format is invalid`);
  return value;
}
function role(value) { requireCondition(REVIEW_ATTESTATION_ROLES.includes(value), 'AI review role is invalid'); return value; }

export function validateModelCandidates(value) {
  exactKeys(value, REVIEW_ATTESTATION_ROLES, 'AI review model candidates');
  const result = {};
  for (const currentRole of REVIEW_ATTESTATION_ROLES) {
    requireCondition(Array.isArray(value[currentRole]) && value[currentRole].length >= 1 && value[currentRole].length <= 2, `${currentRole} model candidates are invalid`);
    result[currentRole] = value[currentRole].map((candidate, index) => {
      exactKeys(candidate, ['alias', 'id'], `${currentRole} model candidate ${index}`);
      return {
        alias: string(candidate.alias, `${currentRole} model alias`, 128, SAFE),
        id: string(candidate.id, `${currentRole} model ID`, 256, SAFE),
      };
    });
    requireCondition(new Set(result[currentRole].map((candidate) => candidate.id)).size === result[currentRole].length, `${currentRole} model IDs are duplicated`);
  }
  return result;
}

function validateCoverage(value, { requireComplete = false } = {}) {
  exactKeys(value, ['complete', 'file_count', 'patch_bytes', 'manifest_sha', 'raw_diff_sha'], 'AI review coverage');
  requireCondition(typeof value.complete === 'boolean', 'AI review coverage flag is invalid');
  requireCondition(Number.isSafeInteger(value.file_count) && value.file_count >= 0 && value.file_count <= 300, 'AI review file count is invalid');
  requireCondition(Number.isSafeInteger(value.patch_bytes) && value.patch_bytes >= 0 && value.patch_bytes <= MAX_AI_REVIEW_ARTIFACT_BYTES, 'AI review patch bytes are invalid');
  if (requireComplete) requireCondition(value.complete === true && value.file_count > 0 && value.patch_bytes > 0, 'AI review coverage is incomplete');
  return { complete: value.complete, file_count: value.file_count, patch_bytes: value.patch_bytes, manifest_sha: string(value.manifest_sha, 'AI review manifest SHA', 64, HASH), raw_diff_sha: string(value.raw_diff_sha, 'AI review raw diff SHA', 64, HASH) };
}

function validateProfile(value, currentRole, modelCandidates) {
  exactKeys(value, ['schema_version', 'role', 'prompt_version', 'prompt_template_sha', 'output_schema_sha', 'request', 'requested_models'], `${currentRole} review profile`);
  requireCondition(value.schema_version === 1 && value.role === role(currentRole), `${currentRole} review profile identity is invalid`);
  string(value.prompt_version, `${currentRole} prompt version`, 80, SAFE);
  string(value.prompt_template_sha, `${currentRole} prompt template SHA`, 64, HASH);
  string(value.output_schema_sha, `${currentRole} output schema SHA`, 64, HASH);
  exactKeys(value.request, ['endpoint', 'temperature', 'maximum_output_tokens', 'response_format', 'stream'], `${currentRole} request profile`);
  requireCondition(value.request.endpoint === '/chat/completions' && value.request.temperature === 0.1 && Number.isSafeInteger(value.request.maximum_output_tokens) && value.request.maximum_output_tokens > 0 && value.request.maximum_output_tokens <= 16_384 && value.request.response_format === 'json_schema' && value.request.stream === false, `${currentRole} request profile is invalid`);
  requireCondition(canonicalJson(value.requested_models) === canonicalJson(modelCandidates), `${currentRole} requested models do not match candidates`);
  return structuredClone(value);
}

export function validateCandidateArtifact(value) {
  exactKeys(value, ['schema_version', 'artifact_type', 'generation', 'input', 'input_sha', 'coverage', 'model_candidates', 'profiles', 'profile_shas', 'prompt_shas'], 'AI review candidate artifact');
  requireCondition(value.schema_version === 1 && value.artifact_type === 'ai_review_candidate', 'AI review candidate schema is invalid');
  const generation = validateReviewGeneration(value.generation);
  const models = validateModelCandidates(value.model_candidates);
  object(value.input, 'AI review input');
  requireCondition(value.input_sha === sha256(canonicalJson(value.input)), 'AI review input SHA is invalid');
  requireCondition(
    value.input.repository === generation.repository && value.input.pull_number === generation.pull_number &&
      value.input.head?.sha === generation.head_sha && value.input.base?.sha === generation.base_sha,
    'AI review input does not match its exact generation',
  );
  const coverage = validateCoverage(value.coverage);
  exactKeys(value.profiles, REVIEW_ATTESTATION_ROLES, 'AI review profiles');
  exactKeys(value.profile_shas, REVIEW_ATTESTATION_ROLES, 'AI review profile SHAs');
  exactKeys(value.prompt_shas, REVIEW_ATTESTATION_ROLES, 'AI review prompt SHAs');
  for (const currentRole of REVIEW_ATTESTATION_ROLES) {
    const profile = validateProfile(value.profiles[currentRole], currentRole, models[currentRole]);
    requireCondition(value.profile_shas[currentRole] === sha256(canonicalJson(profile)), `${currentRole} profile SHA is invalid`);
    string(value.prompt_shas[currentRole], `${currentRole} prompt SHA`, 64, HASH);
  }
  const normalized = {
    schema_version: 1,
    artifact_type: 'ai_review_candidate',
    generation,
    input: structuredClone(value.input),
    input_sha: value.input_sha,
    coverage,
    model_candidates: models,
    profiles: structuredClone(value.profiles),
    profile_shas: { ...value.profile_shas },
    prompt_shas: { ...value.prompt_shas },
  };
  requireCondition(Buffer.byteLength(canonicalJson(normalized), 'utf8') <= MAX_AI_REVIEW_ARTIFACT_BYTES, 'AI review candidate exceeds maximum size');
  return normalized;
}

export function candidateSha(value) { return sha256(canonicalJson(validateCandidateArtifact(value))); }

export function validateReviewOutput(currentRole, value) {
  exactKeys(value, ['schema_version', 'role', 'verdict', 'summary', 'findings'], `${currentRole} review output`);
  requireCondition(value.schema_version === 1 && value.role === role(currentRole), `${currentRole} output identity is invalid`);
  requireCondition(['pass', 'fail'].includes(value.verdict), `${currentRole} verdict is invalid`);
  string(value.summary, `${currentRole} summary`, 4000);
  requireCondition(Array.isArray(value.findings) && value.findings.length <= 100, `${currentRole} findings are invalid`);
  const findings = value.findings.map((finding, index) => {
    exactKeys(finding, ['severity', 'title', 'details', 'path', 'line'], `${currentRole} finding ${index}`);
    requireCondition(['critical', 'high', 'medium', 'low'].includes(finding.severity), `${currentRole} finding severity is invalid`);
    return {
      severity: finding.severity,
      title: string(finding.title, `${currentRole} finding title`, 200),
      details: string(finding.details, `${currentRole} finding details`, 2000),
      path: finding.path === null ? null : string(finding.path, `${currentRole} finding path`, 1024),
      line: finding.line === null ? null : (() => { requireCondition(Number.isSafeInteger(finding.line) && finding.line > 0, `${currentRole} finding line is invalid`); return finding.line; })(),
    };
  });
  requireCondition((value.verdict === 'pass') === (findings.length === 0), `${currentRole} verdict and findings disagree`);
  return { schema_version: 1, role: currentRole, verdict: value.verdict, summary: value.summary, findings };
}

function validateRequestedModel(value, currentRole) {
  exactKeys(value, ['alias', 'id'], `${currentRole} requested model`);
  return { alias: string(value.alias, `${currentRole} requested model alias`, 128, SAFE), id: string(value.id, `${currentRole} requested model ID`, 256, SAFE) };
}

function validateProviderModel(value, currentRole) {
  exactKeys(value, ['response_id', 'model', 'system_fingerprint'], `${currentRole} provider model`);
  return {
    response_id: string(value.response_id, `${currentRole} provider response ID`, 256, SAFE),
    model: string(value.model, `${currentRole} provider model`, 256, SAFE),
    system_fingerprint: value.system_fingerprint === null ? null : string(value.system_fingerprint, `${currentRole} provider fingerprint`, 256, SAFE),
  };
}

export function validateAnalysisArtifact(value) {
  exactKeys(value, ['schema_version', 'artifact_type', 'candidate_sha', 'results'], 'AI review analysis artifact');
  requireCondition(value.schema_version === 1 && value.artifact_type === 'ai_review_analysis', 'AI review analysis schema is invalid');
  string(value.candidate_sha, 'AI review analysis candidate SHA', 64, HASH);
  exactKeys(value.results, REVIEW_ATTESTATION_ROLES, 'AI review analysis results');
  const results = {};
  for (const currentRole of REVIEW_ATTESTATION_ROLES) {
    const result = value.results[currentRole];
    exactKeys(result, ['state', 'requested_model', 'provider_model', 'output', 'result_sha', 'prompt_sha', 'profile_sha', 'failure'], `${currentRole} analysis result`);
    requireCondition(['completed', 'failed'].includes(result.state), `${currentRole} analysis state is invalid`);
    string(result.prompt_sha, `${currentRole} analysis prompt SHA`, 64, HASH);
    string(result.profile_sha, `${currentRole} analysis profile SHA`, 64, HASH);
    if (result.state === 'completed') {
      const output = validateReviewOutput(currentRole, result.output);
      requireCondition(result.result_sha === sha256(canonicalJson(output)), `${currentRole} result SHA is invalid`);
      requireCondition(result.failure === null, `${currentRole} completed result contains a failure`);
      results[currentRole] = {
        state: 'completed',
        requested_model: validateRequestedModel(result.requested_model, currentRole),
        provider_model: validateProviderModel(result.provider_model, currentRole),
        output,
        result_sha: result.result_sha,
        prompt_sha: result.prompt_sha,
        profile_sha: result.profile_sha,
        failure: null,
      };
    } else {
      requireCondition(result.requested_model === null && result.provider_model === null && result.output === null && result.result_sha === null, `${currentRole} failed result contains completion evidence`);
      exactKeys(result.failure, ['code'], `${currentRole} analysis failure`);
      results[currentRole] = {
        state: 'failed', requested_model: null, provider_model: null, output: null, result_sha: null,
        prompt_sha: result.prompt_sha, profile_sha: result.profile_sha,
        failure: { code: string(result.failure.code, `${currentRole} failure code`, 80, /^[a-z][a-z0-9_]{0,79}$/) },
      };
    }
  }
  return { schema_version: 1, artifact_type: 'ai_review_analysis', candidate_sha: value.candidate_sha, results };
}

export function assertAnalysisMatchesCandidate(analysisValue, candidateValue) {
  const analysis = validateAnalysisArtifact(analysisValue);
  const candidate = validateCandidateArtifact(candidateValue);
  requireCondition(analysis.candidate_sha === candidateSha(candidate), 'AI review analysis belongs to another candidate');
  for (const currentRole of REVIEW_ATTESTATION_ROLES) {
    requireCondition(analysis.results[currentRole].prompt_sha === candidate.prompt_shas[currentRole], `${currentRole} analysis prompt is stale`);
    requireCondition(analysis.results[currentRole].profile_sha === candidate.profile_shas[currentRole], `${currentRole} analysis profile is stale`);
    if (analysis.results[currentRole].state === 'completed') {
      requireCondition(candidate.model_candidates[currentRole].some((model) => canonicalJson(model) === canonicalJson(analysis.results[currentRole].requested_model)), `${currentRole} requested model is not allowed by the candidate`);
    }
  }
  return { analysis, candidate };
}

export { canonicalJson, sha256 } from './review-attestation-contract.mjs';
