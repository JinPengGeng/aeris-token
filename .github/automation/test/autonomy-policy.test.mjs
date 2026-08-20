import assert from 'node:assert/strict';
import test from 'node:test';

import { AutonomyPolicyError, classifyAutonomyPolicy } from '../src/autonomy-policy.mjs';

const config = Object.freeze({
  repository: 'JinPengGeng/aeris-token',
  base_ref: 'main',
  writer_login: 'aeris-writer[bot]',
  branch_prefix: 'agent/issue-',
  maximum_files: 20,
  maximum_changes: 2000,
});

function file(overrides = {}) {
  return {
    filename: 'docs/automation-canary/example.md', status: 'modified', additions: 2, deletions: 1, changes: 3,
    mode: '100644', binary: false, ...overrides,
  };
}

function label(name, id = 1) {
  return { id, name };
}

function snapshot(overrides = {}) {
  return {
    repository: config.repository,
    base: { ref: 'main', sha: 'a'.repeat(40) },
    head: { ref: 'agent/issue-123', sha: 'b'.repeat(40) },
    source: { author: config.writer_login, branch: 'agent/issue-123', repository: config.repository },
    labels: [],
    labels_truncated: false,
    files: [file()],
    truncated: false,
    ...overrides,
  };
}

function assertContractError(action, message) {
  assert.throws(action, (error) => error instanceof AutonomyPolicyError && new RegExp(message).test(error.message));
}

test('an ordinary bounded Writer canary document change is eligible', () => {
  const decision = classifyAutonomyPolicy(snapshot(), config);
  assert.deepEqual(decision, { classification: 'eligible', reasons: [] });
  assert.ok(Object.isFrozen(decision));
  assert.ok(Object.isFrozen(decision.reasons));
});

test('ordinary changes outside the canary allowlist and allowance limits remain manual', () => {
  assert.deepEqual(classifyAutonomyPolicy(snapshot({ files: [file({ filename: 'src/lib.rs' })] }), config), {
    classification: 'manual', reasons: ['manual_path_outside_allowlist'],
  });
  const oversized = Array.from({ length: 21 }, (_, index) => file({ filename: `docs/automation-canary/${index}.md`, changes: 100 }));
  assert.deepEqual(classifyAutonomyPolicy(snapshot({ files: oversized }), config), {
    classification: 'manual', reasons: ['manual_change_limit_exceeded', 'manual_file_limit_exceeded'],
  });
});

test('repository, base, and incomplete snapshot violations deny while ordinary branches remain manual', () => {
  const decision = classifyAutonomyPolicy(snapshot({
    repository: 'other/repository', base: { ref: 'release/v1', sha: 'c'.repeat(40) },
    head: { ref: 'feature/unsafe', sha: 'b'.repeat(40) },
    source: { author: 'contributor', branch: 'feature/unsafe', repository: config.repository }, truncated: true,
  }), config);
  assert.deepEqual(decision, {
    classification: 'deny',
    reasons: ['deny_base_ref_mismatch', 'deny_repository_mismatch', 'deny_snapshot_truncated', 'manual_unmanaged_branch'],
  });
  assert.deepEqual(classifyAutonomyPolicy(snapshot({
    head: { ref: 'feature/human', sha: 'b'.repeat(40) },
    source: { author: 'maintainer', branch: 'feature/human', repository: config.repository },
    files: [],
  }), config), { classification: 'manual', reasons: ['manual_unmanaged_branch'] });
});

test('manual-hold labels deny only the intended pull requests', () => {
  assert.deepEqual(classifyAutonomyPolicy(snapshot({ labels: [label('do-not-merge')] }), config), {
    classification: 'deny', reasons: ['deny_do_not_merge_label'],
  });
  assert.deepEqual(classifyAutonomyPolicy(snapshot({ labels: [label('autonomy-manual')] }), config), {
    classification: 'deny', reasons: ['deny_autonomy_manual_label'],
  });

  const human = {
    head: { ref: 'feature/human', sha: 'b'.repeat(40) },
    source: { author: 'maintainer', branch: 'feature/human', repository: config.repository },
    files: [],
  };
  assert.deepEqual(classifyAutonomyPolicy(snapshot({ ...human, labels: [label('autonomy-manual')] }), config), {
    classification: 'manual', reasons: ['manual_unmanaged_branch'],
  });
  assert.deepEqual(classifyAutonomyPolicy(snapshot({ ...human, labels: [label('do-not-merge')] }), config), {
    classification: 'deny', reasons: ['deny_do_not_merge_label', 'manual_unmanaged_branch'],
  });
  assert.deepEqual(classifyAutonomyPolicy(snapshot(human), config), {
    classification: 'manual', reasons: ['manual_unmanaged_branch'],
  });
});

test('governed paths, unsafe statuses, modes, binary data, duplicates, and case conflicts deny', () => {
  const files = [
    file({ filename: '.github/workflows/write.yml' }),
    file({ filename: 'CODEOWNERS', status: 'removed', mode: '120000', binary: true }),
    file({ filename: 'docs/automation-canary/duplicate.md' }),
    file({ filename: 'docs/automation-canary/duplicate.md' }),
    file({ filename: 'docs/automation-canary/Case.md' }),
    file({ filename: 'docs/automation-canary/case.md', previous_filename: 'docs/old.md' }),
  ];
  assert.deepEqual(classifyAutonomyPolicy(snapshot({ files }), config), {
    classification: 'deny',
    reasons: [
      'deny_binary_file', 'deny_case_fold_conflict', 'deny_duplicate_path', 'deny_governed_path',
      'deny_non_regular_mode', 'deny_unsafe_file_status', 'manual_path_outside_allowlist',
    ],
  });
});

test('malformed snapshots and configurations reject instead of guessing', () => {
  assertContractError(() => classifyAutonomyPolicy({ ...snapshot(), files: [file({ mode: undefined })] }, config), 'mode is invalid');
  assertContractError(() => classifyAutonomyPolicy({ ...snapshot(), files: [file({ filename: '../escape.md' })] }, config), 'escapes');
  assertContractError(() => classifyAutonomyPolicy({ ...snapshot(), head: { ref: 'agent/x', sha: 'B'.repeat(40) } }, config), 'format');
  assertContractError(() => classifyAutonomyPolicy({ ...snapshot(), files: undefined }, config), 'unexpected keys|array');
  assertContractError(() => classifyAutonomyPolicy({ ...snapshot(), labels: undefined }, config), 'labels must be an array');
  assertContractError(() => classifyAutonomyPolicy({ ...snapshot(), labels: [{ name: 'do-not-merge' }] }, config), 'unexpected keys');
  assertContractError(() => classifyAutonomyPolicy({ ...snapshot(), labels: [label('one'), label('ONE', 2)] }, config), 'duplicate name');
  assertContractError(() => classifyAutonomyPolicy({ ...snapshot(), labels: [label('one'), label('two')] }, config), 'duplicate id');
  assertContractError(() => classifyAutonomyPolicy(snapshot(), { ...config, maximum_files: 21 }), 'limits');
  assertContractError(() => classifyAutonomyPolicy(snapshot(), { ...config, branch_prefix: 'agent/' }), 'end with a hyphen');
});

test('a label snapshot explicitly marked incomplete denies', () => {
  assert.deepEqual(classifyAutonomyPolicy(snapshot({ labels_truncated: true }), config), {
    classification: 'deny', reasons: ['deny_labels_snapshot_truncated'],
  });
});

test('renames and empty change sets are never eligible', () => {
  assert.deepEqual(classifyAutonomyPolicy(snapshot({ files: [file({ status: 'renamed', previous_filename: 'docs/automation-canary/old.md' })] }), config), {
    classification: 'deny', reasons: ['deny_unsafe_file_status'],
  });
  assert.deepEqual(classifyAutonomyPolicy(snapshot({ files: [] }), config), {
    classification: 'deny', reasons: ['deny_empty_change_set'],
  });
});
