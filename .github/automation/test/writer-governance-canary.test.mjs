import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  WriterGovernanceCanaryError,
  proveWriterGovernanceCanary,
  runWriterGovernanceCanary,
} from '../src/writer-governance-canary.mjs';

const REPOSITORY = 'JinPengGeng/aeris-token';
const REPOSITORY_ID = 1316750512;
const OWNER_ID = 11525733;
const APP_ID = 4667256;

function completeConnection(nodes = []) {
  return { nodes, totalCount: nodes.length, pageInfo: { hasNextPage: false, endCursor: null } };
}

function classicProtection(extraRulesets = []) {
  const contexts = ['Rust CI / check', 'Frontend CI / check', 'Automation Policy / gate'];
  return {
    mergeCommitAllowed: false,
    rebaseMergeAllowed: false,
    squashMergeAllowed: true,
    isArchived: false,
    isDisabled: false,
    isLocked: false,
    branchProtectionRules: completeConnection([{
      pattern: 'main',
      allowsDeletions: false,
      allowsForcePushes: false,
      blocksCreations: false,
      dismissesStaleReviews: true,
      requiresStatusChecks: true,
      requiresStrictStatusChecks: true,
      isAdminEnforced: true,
      lockAllowsFetchAndMerge: false,
      lockBranch: false,
      requireLastPushApproval: false,
      requiredApprovingReviewCount: 0,
      requiredDeploymentEnvironments: [],
      requiresApprovingReviews: true,
      requiresCodeOwnerReviews: false,
      requiresCommitSignatures: false,
      requiresConversationResolution: true,
      requiresDeployments: false,
      requiresLinearHistory: true,
      restrictsPushes: false,
      restrictsReviewDismissals: false,
      bypassPullRequestAllowances: completeConnection(),
      bypassForcePushAllowances: completeConnection(),
      pushAllowances: completeConnection(),
      reviewDismissalAllowances: completeConnection(),
      requiredStatusChecks: contexts.map((context) => ({
        context,
        app: { databaseId: 15368, slug: 'github-actions' },
      })),
    }]),
    rulesets: completeConnection([{
      id: 'RRS_fence', databaseId: 101, name: 'agent-head-fence-v1',
      enforcement: 'ACTIVE', target: 'BRANCH',
    }, ...extraRulesets]),
  };
}

function writerSnapshot(extraRulesets = []) {
  return {
    governance_fence: {
      repository: { id: REPOSITORY_ID, full_name: REPOSITORY },
      direct_collaborators: {
        affiliation: 'direct',
        truncated: false,
        items: [{ login: 'JinPengGeng', database_id: OWNER_ID, type: 'User', permission: 'ADMIN' }],
      },
      rulesets: {
        includes_parents: true,
        truncated: false,
        items: [{
          id: 101,
          name: 'agent-head-fence-v1',
          target: 'branch',
          enforcement: 'active',
          conditions: { ref_name: { include: ['refs/heads/agent/**'], exclude: [] } },
          rules: [
            { type: 'creation' },
            { type: 'deletion' },
            { type: 'non_fast_forward' },
            { type: 'update' },
          ],
          bypass_actors: [{ actor_id: APP_ID, actor_type: 'Integration', bypass_mode: 'always' }],
        }, ...extraRulesets],
      },
    },
    secret_lane: {
      actions_permissions: { enabled: true, allowed_actions: 'selected', sha_pinning_required: true },
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
        truncated: false,
        items: [{ name: 'main', type: 'branch' }],
      },
    },
  };
}

const trust = Object.freeze({
  repository: REPOSITORY,
  repository_id: REPOSITORY_ID,
  default_branch: 'main',
});
const writerTrust = Object.freeze({
  proof_app_id: APP_ID,
  proof_app_slug: 'aeris-token-writer',
  proof_app_owner_login: 'JinPengGeng',
  proof_app_owner_database_id: OWNER_ID,
});

class FakeClient {
  constructor({ protections = [classicProtection(), classicProtection()], snapshots = [writerSnapshot(), writerSnapshot()] } = {}) {
    this.protections = protections;
    this.snapshots = snapshots;
    this.protectionReads = 0;
    this.snapshotReads = 0;
  }

  async getBranchProtection() {
    return structuredClone(this.protections[this.protectionReads++]);
  }

  async readWriterGovernanceSnapshotOnce() {
    return structuredClone(this.snapshots[this.snapshotReads++]);
  }
}

function environment(outputPath) {
  return {
    GITHUB_REPOSITORY: REPOSITORY,
    GITHUB_REPOSITORY_ID: String(REPOSITORY_ID),
    AERIS_DEFAULT_BRANCH: 'main',
    AERIS_WRITER_APP_ID: String(APP_ID),
    AERIS_WRITER_APP_SLUG: writerTrust.proof_app_slug,
    AERIS_WRITER_APP_OWNER_LOGIN: writerTrust.proof_app_owner_login,
    AERIS_WRITER_APP_OWNER_DATABASE_ID: String(OWNER_ID),
    AERIS_WRITER_TOKEN: 'not-written-to-output',
    GITHUB_OUTPUT: outputPath,
  };
}

test('canary performs exactly two complete classic and REST governance reads', async () => {
  const client = new FakeClient();
  const proof = await proveWriterGovernanceCanary(client, { trust, writerTrust });
  assert.equal(client.protectionReads, 2);
  assert.equal(client.snapshotReads, 2);
  assert.equal(proof.classic.profile, 'direct-squash-v1');
  assert.equal(proof.fence.ruleset_id, 101);
  assert.equal(proof.secret_lane.profile, 'writer-secret-lane-v1');
});

test('canary rejects valid governance drift between its two complete reads', async () => {
  const inactiveGraphQl = {
    id: 'RRS_inactive', databaseId: 202, name: 'inactive-policy',
    enforcement: 'DISABLED', target: 'BRANCH',
  };
  const inactiveRest = {
    id: 202, name: 'inactive-policy', target: 'branch', enforcement: 'disabled',
  };
  const client = new FakeClient({
    protections: [classicProtection(), classicProtection([inactiveGraphQl])],
    snapshots: [writerSnapshot(), writerSnapshot([inactiveRest])],
  });
  await assert.rejects(
    () => proveWriterGovernanceCanary(client, { trust, writerTrust }),
    (error) => error instanceof WriterGovernanceCanaryError && /drifted between complete/.test(error.message),
  );
});

test('canary uses the Finalizer validators and fails closed on an invalid secret lane', async () => {
  const invalid = writerSnapshot();
  invalid.secret_lane.workflow_permissions.default_workflow_permissions = 'write';
  await assert.rejects(
    () => proveWriterGovernanceCanary(new FakeClient({ snapshots: [invalid, invalid] }), { trust, writerTrust }),
    /workflow permissions must be read/,
  );
});

test('CLI emits only a non-sensitive digest, ruleset id, and closed summary', async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-writer-governance-'));
  const outputPath = path.join(directory, 'github-output');
  try {
    const client = new FakeClient();
    const result = await runWriterGovernanceCanary(environment(outputPath), { client });
    assert.match(result.snapshot_sha256, /^[0-9a-f]{64}$/);
    assert.equal(result.governance_fence_ruleset_id, 101);
    assert.deepEqual(result.required_contexts, [
      'Rust CI / check',
      'Frontend CI / check',
      'Automation Policy / gate',
    ]);
    const output = fs.readFileSync(outputPath, 'utf8');
    assert.match(output, new RegExp(`snapshot_sha256=${result.snapshot_sha256}`));
    assert.match(output, /ruleset_id=101/);
    assert.match(output, /snapshot_summary=\{"schema_version":1,/);
    assert.doesNotMatch(output, /not-written-to-output|private[_ -]?key|installation[_ -]?token/i);
    assert.equal(client.protectionReads, 2);
    assert.equal(client.snapshotReads, 2);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
