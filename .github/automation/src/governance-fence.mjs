const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const LOGIN = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$/;
const APP_SLUG = /^[a-z0-9](?:[a-z0-9-]{0,98}[a-z0-9])?$/;

export const GOVERNANCE_FENCE_RULESET_NAME = 'agent-head-fence-v1';
export const GOVERNANCE_FENCE_RULE_TYPES = Object.freeze([
  'creation',
  'update',
  'deletion',
  'non_fast_forward',
]);
export const WRITER_SECRET_LANE_PROFILE = 'writer-secret-lane-v1';

const ACTIVE_RULESET_KEYS = Object.freeze([
  'id',
  'name',
  'target',
  'enforcement',
  'conditions',
  'rules',
  'bypass_actors',
]);
const RULESET_SUMMARY_KEYS = Object.freeze(['id', 'name', 'target', 'enforcement']);

export class GovernanceFenceValidationError extends Error {
  constructor(message) {
    super(message);
    this.name = 'GovernanceFenceValidationError';
  }
}

export class WriterSecretLaneValidationError extends Error {
  constructor(message) {
    super(message);
    this.name = 'WriterSecretLaneValidationError';
  }
}

function reject(message) {
  throw new GovernanceFenceValidationError(message);
}

function rejectSecretLane(message) {
  throw new WriterSecretLaneValidationError(message);
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

function requiredString(value, name, maximumLength, pattern = null) {
  requireCondition(
    typeof value === 'string' && value.length > 0 && value.length <= maximumLength,
    `${name} is invalid`,
  );
  requireCondition(!/[\u0000-\u001f\u007f]/.test(value), `${name} contains control characters`);
  if (pattern) requireCondition(pattern.test(value), `${name} format is invalid`);
  return value;
}

function positiveInteger(value, name) {
  requireCondition(Number.isSafeInteger(value) && value > 0, `${name} must be a positive safe integer`);
  return value;
}

function completeCollection(value, name, scopeKeys = []) {
  exactKeys(value, ['items', 'truncated', ...scopeKeys], name);
  requireCondition(Array.isArray(value.items), `${name} items are invalid`);
  requireCondition(value.truncated === false, `${name} pagination is incomplete`);
  return value.items;
}

function normalizedExpected(value) {
  exactKeys(value, [
    'repository',
    'repository_id',
    'trusted_owner_login',
    'trusted_owner_database_id',
    'app_id',
    'app_slug',
  ], 'governance fence expected identity');
  return Object.freeze({
    repository: requiredString(value.repository, 'expected repository', 256, REPOSITORY),
    repository_id: positiveInteger(value.repository_id, 'expected repository id'),
    trusted_owner_login: requiredString(value.trusted_owner_login, 'trusted owner login', 39, LOGIN),
    trusted_owner_database_id: positiveInteger(
      value.trusted_owner_database_id,
      'trusted owner database id',
    ),
    app_id: positiveInteger(value.app_id, 'Writer App id'),
    app_slug: requiredString(value.app_slug, 'Writer App slug', 100, APP_SLUG),
  });
}

function validateRepository(value, expected) {
  exactKeys(value, ['id', 'full_name'], 'governance repository');
  const id = positiveInteger(value.id, 'governance repository id');
  const fullName = requiredString(value.full_name, 'governance repository full_name', 256, REPOSITORY);
  requireCondition(
    id === expected.repository_id && fullName === expected.repository,
    'governance repository identity does not match the trusted context',
  );
}

function validateDirectCollaborators(connection, expected) {
  const collaborators = completeCollection(
    connection,
    'repository direct collaborators',
    ['affiliation'],
  );
  requireCondition(
    connection.affiliation === 'direct',
    'repository collaborator inventory is not scoped to direct access',
  );
  requireCondition(
    collaborators.length === 1,
    'repository direct collaborator inventory is not uniquely owner-controlled',
  );
  const owner = collaborators[0];
  exactKeys(
    owner,
    ['login', 'database_id', 'type', 'permission'],
    'repository direct collaborator',
  );
  const login = requiredString(owner.login, 'repository direct collaborator login', 39, LOGIN);
  const databaseId = positiveInteger(
    owner.database_id,
    'repository direct collaborator database id',
  );
  requireCondition(owner.type === 'User', 'repository direct collaborator type is not trusted');
  requireCondition(owner.permission === 'ADMIN', 'repository owner permission is not ADMIN');
  requireCondition(
    login === expected.trusted_owner_login && databaseId === expected.trusted_owner_database_id,
    'repository owner identity does not match the trusted owner',
  );
}

function validateFenceConditions(value) {
  exactKeys(value, ['ref_name'], 'governance fence conditions');
  exactKeys(value.ref_name, ['include', 'exclude'], 'governance fence ref_name condition');
  requireCondition(
    Array.isArray(value.ref_name.include) &&
      value.ref_name.include.length === 1 &&
      value.ref_name.include[0] === 'refs/heads/agent/**',
    'governance fence include condition is not exact',
  );
  requireCondition(
    Array.isArray(value.ref_name.exclude) && value.ref_name.exclude.length === 0,
    'governance fence exclude condition is not empty',
  );
}

function validateFenceRules(value) {
  requireCondition(Array.isArray(value), 'governance fence rules are invalid');
  requireCondition(
    value.length === GOVERNANCE_FENCE_RULE_TYPES.length,
    'governance fence rules are not exact',
  );
  const actual = new Set();
  for (const rule of value) {
    exactKeys(rule, ['type'], 'governance fence rule');
    const type = requiredString(rule.type, 'governance fence rule type', 64);
    requireCondition(GOVERNANCE_FENCE_RULE_TYPES.includes(type), `governance fence rule is invalid: ${type}`);
    requireCondition(!actual.has(type), `governance fence rule is duplicated: ${type}`);
    actual.add(type);
  }
  requireCondition(
    GOVERNANCE_FENCE_RULE_TYPES.every((type) => actual.has(type)),
    'governance fence rules are not exact',
  );
}

function validateFenceBypass(value, expected) {
  requireCondition(Array.isArray(value), 'governance fence bypass actors are invalid');
  requireCondition(value.length === 1, 'governance fence App bypass is not unique');
  const bypass = value[0];
  exactKeys(bypass, ['actor_id', 'actor_type', 'bypass_mode'], 'governance fence bypass actor');
  requireCondition(
    positiveInteger(bypass.actor_id, 'governance fence bypass actor id') === expected.app_id,
    'governance fence bypass actor is not the Writer App',
  );
  requireCondition(bypass.actor_type === 'Integration', 'governance fence bypass actor type is invalid');
  requireCondition(bypass.bypass_mode === 'always', 'governance fence bypass mode is invalid');
}

function validateActiveFence(value, expected) {
  exactKeys(value, ACTIVE_RULESET_KEYS, 'active governance fence ruleset');
  requireCondition(value.target === 'branch', 'governance fence target is not branch');
  validateFenceConditions(value.conditions);
  validateFenceRules(value.rules);
  validateFenceBypass(value.bypass_actors, expected);
}

function validateRulesets(connection, expected) {
  const rulesets = completeCollection(connection, 'repository rulesets', ['includes_parents']);
  requireCondition(
    connection.includes_parents === true,
    'repository ruleset inventory does not include parent rulesets',
  );
  const ids = new Set();
  let activeFence = null;
  for (const ruleset of rulesets) {
    requireCondition(isObject(ruleset), 'repository ruleset must be an object');
    const enforcement = requiredString(ruleset.enforcement, 'repository ruleset enforcement', 16);
    const name = requiredString(ruleset.name, 'repository ruleset name', 100);
    const id = positiveInteger(ruleset.id, 'repository ruleset id');
    requireCondition(
      ['branch', 'tag', 'push'].includes(ruleset.target),
      'repository ruleset target is unknown',
    );
    requireCondition(!ids.has(id), 'repository ruleset id is duplicated');
    ids.add(id);
    requireCondition(
      ['active', 'disabled', 'evaluate'].includes(enforcement),
      'repository ruleset enforcement is unknown',
    );
    if (enforcement === 'active' && name !== GOVERNANCE_FENCE_RULESET_NAME) {
      exactKeys(ruleset, RULESET_SUMMARY_KEYS, 'unexpected active repository ruleset');
      reject(`unexpected active repository ruleset: ${name}`);
    }
    if (enforcement !== 'active') {
      exactKeys(ruleset, RULESET_SUMMARY_KEYS, 'inactive repository ruleset');
      continue;
    }
    requireCondition(activeFence === null, 'active governance fence ruleset is not unique');
    validateActiveFence(ruleset, expected);
    activeFence = ruleset;
  }
  requireCondition(activeFence !== null, 'active governance fence ruleset is missing');
  return activeFence;
}

// classicMainProtectionVerified is asserted only after the caller has completed
// its independent, exact classic-main branch-protection validation.
export function validateGovernanceFence(proof, expectedIdentity) {
  const expected = normalizedExpected(expectedIdentity);
  exactKeys(
    proof,
    ['repository', 'direct_collaborators', 'rulesets', 'classicMainProtectionVerified'],
    'governance fence proof',
  );
  requireCondition(
    proof.classicMainProtectionVerified === true,
    'classic main branch protection has not been verified',
  );
  validateRepository(proof.repository, expected);
  validateDirectCollaborators(proof.direct_collaborators, expected);
  const fence = validateRulesets(proof.rulesets, expected);
  return Object.freeze({
    profile: GOVERNANCE_FENCE_RULESET_NAME,
    repository: expected.repository,
    repository_id: expected.repository_id,
    trusted_owner_login: expected.trusted_owner_login,
    trusted_owner_database_id: expected.trusted_owner_database_id,
    app_id: expected.app_id,
    app_slug: expected.app_slug,
    ruleset_id: fence.id,
  });
}

function secretLaneCondition(condition, message) {
  if (!condition) rejectSecretLane(message);
}

function secretLaneExactKeys(value, keys, name) {
  secretLaneCondition(isObject(value), `${name} must be an object`);
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  secretLaneCondition(
    actual.length === expected.length && actual.every((key, index) => key === expected[index]),
    `${name} has unexpected keys`,
  );
}

function defaultBranch(value) {
  secretLaneCondition(
    typeof value === 'string' && value.length > 0 && value.length <= 255,
    'Writer secret lane default branch is invalid',
  );
  secretLaneCondition(
    !/[\u0000-\u0020\u007f~^:?*[\\]/.test(value) &&
      !value.includes('..') &&
      !value.includes('@{') &&
      !value.includes('//') &&
      !value.startsWith('/') &&
      !value.startsWith('-') &&
      !value.startsWith('refs/') &&
      !value.endsWith('/') &&
      !value.endsWith('.') &&
      value.split('/').every((component) => !component.startsWith('.') && !component.endsWith('.lock')) &&
      value !== '@',
    'Writer secret lane default branch format is invalid',
  );
  return value;
}

function validateActionsPermissions(value) {
  secretLaneExactKeys(
    value,
    ['enabled', 'allowed_actions', 'sha_pinning_required'],
    'Writer Actions permissions',
  );
  secretLaneCondition(value.enabled === true, 'Writer Actions must be enabled');
  secretLaneCondition(
    value.allowed_actions === 'selected',
    'Writer Actions allowed_actions must be selected',
  );
  secretLaneCondition(
    value.sha_pinning_required === true,
    'Writer Actions SHA pinning must be required',
  );
}

function validateWorkflowPermissions(value) {
  secretLaneExactKeys(
    value,
    ['default_workflow_permissions', 'can_approve_pull_request_reviews'],
    'Writer workflow permissions',
  );
  secretLaneCondition(
    value.default_workflow_permissions === 'read',
    'Writer default workflow permissions must be read',
  );
  secretLaneCondition(
    value.can_approve_pull_request_reviews === false,
    'Writer workflows must not approve pull requests',
  );
}

function validateWriterEnvironment(value) {
  secretLaneExactKeys(
    value,
    [
      'name',
      'custom_branch_policies',
      'protected_branches',
      'can_admins_bypass_secrets_and_variables',
    ],
    'Writer Environment',
  );
  secretLaneCondition(value.name === 'writer', 'Writer Environment name is invalid');
  secretLaneCondition(
    value.custom_branch_policies === true,
    'Writer Environment custom branch policies must be enabled',
  );
  secretLaneCondition(
    value.protected_branches === false,
    'Writer Environment protected_branches must be disabled',
  );
  secretLaneCondition(
    value.can_admins_bypass_secrets_and_variables === false,
    'Writer Environment admin secret bypass must be disabled',
  );
}

function validateDeploymentBranchPolicies(value, expectedDefaultBranch) {
  secretLaneExactKeys(
    value,
    ['environment_name', 'items', 'truncated'],
    'Writer deployment branch policies',
  );
  secretLaneCondition(
    value.environment_name === 'writer',
    'deployment branch policy inventory is not bound to the Writer Environment',
  );
  secretLaneCondition(Array.isArray(value.items), 'Writer deployment branch policy items are invalid');
  secretLaneCondition(
    value.truncated === false,
    'Writer deployment branch policy pagination is incomplete',
  );
  secretLaneCondition(
    value.items.length === 1,
    'Writer deployment branch policy inventory is not exact',
  );
  const policy = value.items[0];
  secretLaneExactKeys(policy, ['name', 'type'], 'Writer deployment branch policy');
  secretLaneCondition(
    policy.name === expectedDefaultBranch && policy.type === 'branch',
    'Writer deployment branch policy does not bind the exact default branch',
  );
}

export function validateWriterSecretLane(proof, expected) {
  secretLaneExactKeys(expected, ['default_branch'], 'Writer secret lane expected identity');
  const expectedDefaultBranch = defaultBranch(expected.default_branch);
  secretLaneExactKeys(
    proof,
    ['actions_permissions', 'workflow_permissions', 'environment', 'deployment_branch_policies'],
    'Writer secret lane proof',
  );
  validateActionsPermissions(proof.actions_permissions);
  validateWorkflowPermissions(proof.workflow_permissions);
  validateWriterEnvironment(proof.environment);
  validateDeploymentBranchPolicies(proof.deployment_branch_policies, expectedDefaultBranch);
  return Object.freeze({
    profile: WRITER_SECRET_LANE_PROFILE,
    environment: 'writer',
    default_branch: expectedDefaultBranch,
  });
}
