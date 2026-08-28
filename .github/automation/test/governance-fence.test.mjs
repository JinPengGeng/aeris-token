import assert from 'node:assert/strict';
import test from 'node:test';

import {
  GOVERNANCE_FENCE_RULESET_NAME,
  GovernanceFenceValidationError,
  WRITER_SECRET_LANE_PROFILE,
  WriterSecretLaneValidationError,
  validateGovernanceFence,
  validateWriterSecretLane,
} from '../src/governance-fence.mjs';

const EXPECTED = Object.freeze({
  repository: 'trusted-owner/aeris-token',
  repository_id: 42,
  trusted_owner_login: 'trusted-owner',
  trusted_owner_database_id: 7,
  app_id: 9001,
  app_slug: 'aeris-writer',
});

function proof() {
  return {
    repository: { id: 42, full_name: 'trusted-owner/aeris-token' },
    classicMainProtectionVerified: true,
    direct_collaborators: {
      items: [{ login: 'trusted-owner', database_id: 7, type: 'User', permission: 'ADMIN' }],
      truncated: false,
      affiliation: 'direct',
    },
    rulesets: {
      items: [{
        id: 101,
        name: GOVERNANCE_FENCE_RULESET_NAME,
        target: 'branch',
        enforcement: 'active',
        conditions: {
          ref_name: { include: ['refs/heads/agent/**'], exclude: [] },
        },
        rules: [
          { type: 'creation' },
          { type: 'update' },
          { type: 'deletion' },
          { type: 'non_fast_forward' },
        ],
        bypass_actors: [{ actor_id: 9001, actor_type: 'Integration', bypass_mode: 'always' }],
      }],
      truncated: false,
      includes_parents: true,
    },
  };
}

function rejects(mutator, pattern) {
  const value = proof();
  mutator(value);
  assert.throws(
    () => validateGovernanceFence(value, EXPECTED),
    (error) => error instanceof GovernanceFenceValidationError && pattern.test(error.message),
  );
}

test('accepts the exact Writer-only agent head governance fence', () => {
  const result = validateGovernanceFence(proof(), EXPECTED);
  assert.deepEqual(result, {
    profile: GOVERNANCE_FENCE_RULESET_NAME,
    repository: EXPECTED.repository,
    repository_id: EXPECTED.repository_id,
    trusted_owner_login: EXPECTED.trusted_owner_login,
    trusted_owner_database_id: EXPECTED.trusted_owner_database_id,
    app_id: EXPECTED.app_id,
    app_slug: EXPECTED.app_slug,
    ruleset_id: 101,
  });
  assert.ok(Object.isFrozen(result));
});

test('rejects unknown fields in the closed proof and expected schemas', () => {
  rejects((value) => { value.untrusted = true; }, /proof has unexpected keys/);
  assert.throws(
    () => validateGovernanceFence(proof(), { ...EXPECTED, untrusted: true }),
    (error) => error instanceof GovernanceFenceValidationError && /expected identity has unexpected keys/.test(error.message),
  );
});

test('requires the caller to have verified exact classic main protection', () => {
  rejects((value) => { value.classicMainProtectionVerified = false; }, /has not been verified/);
  rejects((value) => { value.classicMainProtectionVerified = 1; }, /has not been verified/);
});

test('binds the exact repository name and numeric id', () => {
  rejects((value) => { value.repository.id = 43; }, /repository identity/);
  rejects((value) => { value.repository.full_name = 'trusted-owner/other'; }, /repository identity/);
});

test('rejects incomplete owner and ruleset pagination', () => {
  rejects((value) => { value.direct_collaborators.truncated = true; }, /collaborators pagination is incomplete/);
  rejects((value) => { value.rulesets.truncated = true; }, /rulesets pagination is incomplete/);
});

test('requires direct collaborator scope and parent-inclusive ruleset scope', () => {
  rejects(
    (value) => { value.direct_collaborators.affiliation = 'outside'; },
    /not scoped to direct access/,
  );
  rejects(
    (value) => { value.rulesets.includes_parents = false; },
    /does not include parent rulesets/,
  );
});

test('requires one exact trusted User owner with ADMIN permission', () => {
  rejects(
    (value) => { value.direct_collaborators.items = []; },
    /direct collaborator inventory is not uniquely owner-controlled/,
  );
  rejects((value) => {
    value.direct_collaborators.items.push({
      login: 'other-owner',
      database_id: 8,
      type: 'User',
      permission: 'ADMIN',
    });
  }, /direct collaborator inventory is not uniquely owner-controlled/);
  rejects((value) => { value.direct_collaborators.items[0].login = 'other-owner'; }, /owner identity/);
  rejects((value) => { value.direct_collaborators.items[0].database_id = 8; }, /owner identity/);
  rejects((value) => { value.direct_collaborators.items[0].type = 'Organization'; }, /collaborator type/);
  rejects((value) => { value.direct_collaborators.items[0].permission = 'WRITE'; }, /permission is not ADMIN/);
});

test('rejects every additional direct collaborator regardless of permission', () => {
  for (const permission of ['MAINTAIN', 'WRITE', 'TRIAGE', 'READ']) {
    rejects((value) => {
      value.direct_collaborators.items.push({
        login: `extra-${permission.toLowerCase()}`,
        database_id: permission.length + 100,
        type: 'User',
        permission,
      });
    }, /direct collaborator inventory is not uniquely owner-controlled/);
  }
});

test('requires one active ruleset with the exact audited name', () => {
  rejects((value) => { value.rulesets.items = []; }, /ruleset is missing/);
  rejects(
    (value) => { value.rulesets.items[0].enforcement = 'disabled'; },
    /inactive repository ruleset has unexpected keys/,
  );
  rejects((value) => { value.rulesets.items[0].name = 'lookalike-fence'; }, /unexpected active/);
});

test('rejects every other active ruleset while allowing closed inactive summaries', () => {
  rejects((value) => {
    value.rulesets.items.push({ id: 102, name: 'other-active', target: 'branch', enforcement: 'active' });
  }, /unexpected active repository ruleset/);

  const value = proof();
  value.rulesets.items.push({ id: 102, name: 'inactive-policy', target: 'branch', enforcement: 'disabled' });
  value.rulesets.items.push({ id: 103, name: 'evaluated-policy', target: 'branch', enforcement: 'evaluate' });
  assert.equal(validateGovernanceFence(value, EXPECTED).ruleset_id, 101);
});

test('rejects duplicated ruleset ids and extra ruleset fields', () => {
  rejects((value) => {
    value.rulesets.items.push({ id: 101, name: 'inactive-policy', target: 'branch', enforcement: 'disabled' });
  }, /ruleset id is duplicated/);
  rejects((value) => { value.rulesets.items[0].source = 'repository'; }, /ruleset has unexpected keys/);
  rejects((value) => { value.rulesets.items[0].target = 'unknown'; }, /ruleset target is unknown/);
});

test('requires the exact branch target and agent head include with no excludes', () => {
  rejects((value) => { value.rulesets.items[0].target = 'tag'; }, /target is not branch/);
  rejects((value) => { value.rulesets.items[0].conditions.ref_name.include = ['refs/heads/agent/*']; }, /include condition/);
  rejects((value) => { value.rulesets.items[0].conditions.ref_name.include.push('refs/heads/main'); }, /include condition/);
  rejects((value) => { value.rulesets.items[0].conditions.ref_name.exclude.push('refs/heads/agent/escape'); }, /exclude condition/);
});

test('requires exactly creation, update, deletion, and non-fast-forward rules', () => {
  for (const type of ['creation', 'update', 'deletion', 'non_fast_forward']) {
    rejects((value) => {
      value.rulesets.items[0].rules = value.rulesets.items[0].rules.filter((rule) => rule.type !== type);
    }, /rules are not exact/);
  }
  rejects((value) => { value.rulesets.items[0].rules[0].type = 'pull_request'; }, /rule is invalid/);
  rejects((value) => { value.rulesets.items[0].rules[1].type = 'creation'; }, /rule is duplicated/);
  rejects((value) => { value.rulesets.items[0].rules[0].parameters = {}; }, /rule has unexpected keys/);
});

test('requires one exact always Writer App Integration bypass with no flags', () => {
  rejects((value) => { value.rulesets.items[0].bypass_actors = []; }, /App bypass is not unique/);
  rejects((value) => {
    value.rulesets.items[0].bypass_actors.push({
      actor_id: 9002,
      actor_type: 'Integration',
      bypass_mode: 'always',
    });
  }, /App bypass is not unique/);
  rejects((value) => { value.rulesets.items[0].bypass_actors[0].actor_id = 9002; }, /not the Writer App/);
  rejects((value) => { value.rulesets.items[0].bypass_actors[0].actor_type = 'Team'; }, /actor type/);
  rejects((value) => { value.rulesets.items[0].bypass_actors[0].bypass_mode = 'pull_request'; }, /bypass mode/);
  rejects((value) => { value.rulesets.items[0].bypass_actors[0].deploy_key_bypass = true; }, /bypass actor has unexpected keys/);
});

test('does not accept unbound Writer App identity metadata', () => {
  assert.throws(
    () => validateGovernanceFence(proof(), { ...EXPECTED, app_id: 0 }),
    (error) => error instanceof GovernanceFenceValidationError && /positive safe integer/.test(error.message),
  );
  assert.throws(
    () => validateGovernanceFence(proof(), { ...EXPECTED, app_slug: 'AERIS Writer' }),
    (error) => error instanceof GovernanceFenceValidationError && /slug format/.test(error.message),
  );
});

function secretLaneProof() {
  return {
    actions_permissions: {
      enabled: true,
      allowed_actions: 'selected',
      sha_pinning_required: true,
    },
    workflow_permissions: {
      default_workflow_permissions: 'read',
      can_approve_pull_request_reviews: false,
    },
    environment: {
      name: 'writer',
      custom_branch_policies: true,
      protected_branches: false,
      can_admins_bypass_secrets_and_variables: false,
    },
    deployment_branch_policies: {
      environment_name: 'writer',
      items: [{ name: 'main', type: 'branch' }],
      truncated: false,
    },
  };
}

function rejectsSecretLane(mutator, pattern, expected = { default_branch: 'main' }) {
  const value = secretLaneProof();
  mutator(value);
  assert.throws(
    () => validateWriterSecretLane(value, expected),
    (error) => error instanceof WriterSecretLaneValidationError && pattern.test(error.message),
  );
}

test('accepts the exact default-branch-only Writer secret lane', () => {
  const result = validateWriterSecretLane(secretLaneProof(), { default_branch: 'main' });
  assert.deepEqual(result, {
    profile: WRITER_SECRET_LANE_PROFILE,
    environment: 'writer',
    default_branch: 'main',
  });
  assert.ok(Object.isFrozen(result));
});

test('Writer secret lane input and expected schemas reject unknown fields', () => {
  rejectsSecretLane((value) => { value.repository = 'unexpected/repository'; }, /proof has unexpected keys/);
  rejectsSecretLane(
    (value) => { value.actions_permissions.selected_actions_url = 'https://example.invalid'; },
    /Actions permissions has unexpected keys/,
  );
  assert.throws(
    () => validateWriterSecretLane(secretLaneProof(), { default_branch: 'main', fallback: 'develop' }),
    (error) => error instanceof WriterSecretLaneValidationError && /expected identity has unexpected keys/.test(error.message),
  );
});

test('Writer Actions permissions must be enabled, selected, and SHA pinned', () => {
  rejectsSecretLane((value) => { value.actions_permissions.enabled = false; }, /must be enabled/);
  rejectsSecretLane((value) => { value.actions_permissions.allowed_actions = 'all'; }, /must be selected/);
  rejectsSecretLane((value) => { value.actions_permissions.sha_pinning_required = false; }, /pinning must be required/);
});

test('Writer workflow defaults remain read-only and cannot approve reviews', () => {
  rejectsSecretLane(
    (value) => { value.workflow_permissions.default_workflow_permissions = 'write'; },
    /must be read/,
  );
  rejectsSecretLane(
    (value) => { value.workflow_permissions.can_approve_pull_request_reviews = true; },
    /must not approve/,
  );
  rejectsSecretLane(
    (value) => { value.workflow_permissions.extra = false; },
    /workflow permissions has unexpected keys/,
  );
});

test('Writer Environment requires exact non-bypassable custom branch policy flags', () => {
  rejectsSecretLane((value) => { value.environment.name = 'production'; }, /name is invalid/);
  rejectsSecretLane(
    (value) => { value.environment.custom_branch_policies = false; },
    /custom branch policies must be enabled/,
  );
  rejectsSecretLane(
    (value) => { value.environment.protected_branches = true; },
    /protected_branches must be disabled/,
  );
  rejectsSecretLane(
    (value) => { value.environment.can_admins_bypass_secrets_and_variables = true; },
    /admin secret bypass must be disabled/,
  );
  rejectsSecretLane(
    (value) => { value.environment.wait_timer = 0; },
    /Environment has unexpected keys/,
  );
});

test('Writer deployment branch policy inventory must be complete and Writer-bound', () => {
  rejectsSecretLane(
    (value) => { value.deployment_branch_policies.truncated = true; },
    /pagination is incomplete/,
  );
  rejectsSecretLane(
    (value) => { value.deployment_branch_policies.environment_name = 'production'; },
    /not bound to the Writer Environment/,
  );
  rejectsSecretLane(
    (value) => { value.deployment_branch_policies.cursor = 'next'; },
    /policies has unexpected keys/,
  );
});

test('Writer deployment policy permits only the exact default branch', () => {
  rejectsSecretLane((value) => { value.deployment_branch_policies.items = []; }, /inventory is not exact/);
  rejectsSecretLane((value) => {
    value.deployment_branch_policies.items.push({ name: 'agent/**', type: 'branch' });
  }, /inventory is not exact/);
  rejectsSecretLane(
    (value) => { value.deployment_branch_policies.items[0].name = 'agent/issue-1'; },
    /does not bind the exact default branch/,
  );
  rejectsSecretLane(
    (value) => { value.deployment_branch_policies.items[0].type = 'tag'; },
    /does not bind the exact default branch/,
  );
  rejectsSecretLane(
    (value) => { value.deployment_branch_policies.items[0].id = 17; },
    /policy has unexpected keys/,
  );
});

test('Writer secret lane rejects malformed expected default branches', () => {
  for (const default_branch of [
    '',
    'refs/heads/main',
    '-main',
    '.main',
    'main.lock',
    'main..next',
    'main lock',
    'main@{tip}',
    '@',
  ]) {
    const value = secretLaneProof();
    value.deployment_branch_policies.items[0].name = default_branch;
    assert.throws(
      () => validateWriterSecretLane(value, { default_branch }),
      (error) => error instanceof WriterSecretLaneValidationError && /default branch/.test(error.message),
    );
  }
});
