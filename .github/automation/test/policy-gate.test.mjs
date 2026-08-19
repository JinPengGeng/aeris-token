import assert from 'node:assert/strict';
import test from 'node:test';

import { evaluatePolicyGate, pathMatchesPattern, policyCheckConclusion } from '../src/policy-gate.mjs';

const sha = (character) => character.repeat(40);

function policy(mode = 'shadow') {
  return {
    trusted_source: { ref: 'refs/heads/main' },
    policy_gate: {
      check_name: 'Automation Policy / gate',
      mode,
      human_enable_label: 'automerge-approved',
      require_exact_head_sha: true,
      require_base_up_to_date: true,
      require_conversation_resolution: true,
      required_checks: ['Rust CI / check', 'Frontend CI / check'],
      required_check_sources: [
        { context: 'Rust CI / check', app_id: 15368, app_slug: 'github-actions' },
        { context: 'Frontend CI / check', app_id: 15368, app_slug: 'github-actions' },
      ],
      always_require_human_review: ['.github/**', '**/Cargo.lock', '**/auth/**', 'Dockerfile*'],
      allowlist_paths: ['docs/**', '**/*.md'],
    },
  };
}

function input(mode = 'shadow') {
  return {
    policy: policy(mode),
    repository: 'JinPengGeng/aeris-token',
    pull: {
      number: 37,
      state: 'open',
      draft: false,
      mergeable: true,
      labels: [],
      head: { sha: sha('a'), ref: 'agent/issue-11', repo: { full_name: 'JinPengGeng/aeris-token' } },
      base: { sha: sha('b'), ref: 'main', repo: { full_name: 'JinPengGeng/aeris-token' } },
    },
    files: {
      truncated: false,
      files: [{ filename: 'docs/policy.md', status: 'modified', previous_filename: null }],
    },
    checkRuns: [
      { id: 1, name: 'Rust CI / check', head_sha: sha('a'), status: 'completed', conclusion: 'success', app: { id: 15368, slug: 'github-actions' } },
      { id: 2, name: 'Frontend CI / check', head_sha: sha('a'), status: 'completed', conclusion: 'success', app: { id: 15368, slug: 'github-actions' } },
    ],
    comparison: { base_sha: sha('b'), head_sha: sha('a'), status: 'ahead' },
    reviewThreads: { unresolved: 0, truncated: false },
    expectedHeadSha: sha('a'),
  };
}

test('path matcher supports the policy glob subset without crossing path segments', () => {
  assert.equal(pathMatchesPattern('Cargo.lock', '**/Cargo.lock'), true);
  assert.equal(pathMatchesPattern('crates/core/Cargo.lock', '**/Cargo.lock'), true);
  assert.equal(pathMatchesPattern('src/auth/token.rs', '**/auth/**'), true);
  assert.equal(pathMatchesPattern('.github/workflows/ci.yml', '.github/**'), true);
  assert.equal(pathMatchesPattern('Dockerfile.dev', 'Dockerfile*'), true);
  assert.equal(pathMatchesPattern('nested/Dockerfile.dev', 'Dockerfile*'), false);
  assert.equal(pathMatchesPattern('docs/sub/readme.md', 'docs/*.md'), false);
  assert.throws(() => pathMatchesPattern('../secret', '**/*'));
  assert.throws(() => pathMatchesPattern('src/lib.rs', '../**'));
});

test('shadow mode reports a passing decision but never enables automatic merge', () => {
  const result = evaluatePolicyGate(input());
  assert.deepEqual(result, {
    mode: 'shadow',
    verdict: 'pass',
    enforcement: 'advisory',
    eligible_for_automatic_merge: false,
    reason_codes: [],
    unsuccessful_checks: [],
    human_review_paths: [],
    changed_file_count: 1,
  });
  assert.equal(policyCheckConclusion(result), 'neutral');
});

test('pending required checks remain advisory and cannot enable merge', () => {
  const value = input();
  value.checkRuns[0] = { id: 3, name: 'Rust CI / check', head_sha: sha('a'), status: 'in_progress', conclusion: null, app: { id: 15368, slug: 'github-actions' } };
  const result = evaluatePolicyGate(value);
  assert.equal(result.verdict, 'pending');
  assert.deepEqual(result.reason_codes, ['required_checks_not_successful']);
  assert.deepEqual(result.unsuccessful_checks, ['Rust CI / check']);
  assert.equal(policyCheckConclusion(result), 'neutral');

  const human = evaluatePolicyGate({ ...value, policy: policy('human') });
  assert.equal(human.verdict, 'pending');
  assert.equal(policyCheckConclusion(human), 'failure');
});

test('untrusted or wrong-head check signals never satisfy a required context', () => {
  const value = input();
  value.checkRuns[0] = { ...value.checkRuns[0], id: 9, app: { id: 999, slug: 'other-app' } };
  value.checkRuns.push({ ...value.checkRuns[0], id: 10, head_sha: sha('d'), app: { id: 15368, slug: 'github-actions' } });
  const result = evaluatePolicyGate(value);
  assert.equal(result.verdict, 'pending');
  assert.deepEqual(result.unsuccessful_checks, ['Rust CI / check']);
});

test('a malformed newer trusted check run cannot be hidden behind an older success', () => {
  const value = input();
  value.checkRuns.unshift({
    ...value.checkRuns[0],
    id: '3',
  });
  assert.throws(() => evaluatePolicyGate(value), /trusted check run ID is invalid/);
});

test('stale heads and non-current bases fail closed', () => {
  const stale = input();
  stale.expectedHeadSha = sha('c');
  stale.comparison.status = 'diverged';
  const result = evaluatePolicyGate(stale);
  assert.equal(result.verdict, 'block');
  assert.deepEqual(result.reason_codes, ['stale_head_sha', 'base_not_up_to_date']);
});

test('unresolved conversations and cross-repository heads block', () => {
  const value = input();
  value.pull.head.repo.full_name = 'outside/fork';
  value.reviewThreads.unresolved = 2;
  const result = evaluatePolicyGate(value);
  assert.equal(result.verdict, 'block');
  assert.deepEqual(result.reason_codes, ['head_repository_mismatch', 'unresolved_review_threads']);
});

test('a workflow target that closed before live evaluation fails closed', () => {
  const value = input();
  value.pull.state = 'closed';
  const result = evaluatePolicyGate(value);
  assert.equal(result.verdict, 'block');
  assert.deepEqual(result.reason_codes, ['pull_request_not_open']);
  assert.equal(result.eligible_for_automatic_merge, false);
});

test('human mode and sensitive paths require manual merge', () => {
  const value = input('human');
  value.files.files = [{ filename: '.github/workflows/policy.yml', status: 'modified', previous_filename: null }];
  const result = evaluatePolicyGate(value);
  assert.equal(result.verdict, 'human_required');
  assert.deepEqual(result.reason_codes, ['human_review_path_changed', 'human_mode_requires_manual_merge']);
  assert.deepEqual(result.human_review_paths, ['.github/workflows/policy.yml']);
  assert.equal(policyCheckConclusion(result), 'success');
});

test('rename sources and every deletion participate in human-review classification', () => {
  const renamed = input();
  renamed.files.files = [{
    filename: 'docs/renamed.yml',
    status: 'renamed',
    previous_filename: '.github/workflows/policy.yml',
  }];
  const renameResult = evaluatePolicyGate(renamed);
  assert.equal(renameResult.verdict, 'human_required');
  assert.deepEqual(renameResult.reason_codes, ['human_review_path_changed']);
  assert.deepEqual(renameResult.human_review_paths, ['.github/workflows/policy.yml']);

  const deleted = input();
  deleted.files.files = [{ filename: 'docs/obsolete.md', status: 'removed', previous_filename: null }];
  const deleteResult = evaluatePolicyGate(deleted);
  assert.equal(deleteResult.verdict, 'human_required');
  assert.deepEqual(deleteResult.reason_codes, ['human_review_path_changed', 'file_deleted']);
  assert.deepEqual(deleteResult.human_review_paths, ['docs/obsolete.md']);
});

test('Phase 4 rejects label and allowlist modes even if policy lists them for future phases', () => {
  assert.throws(() => evaluatePolicyGate(input('label')), /mode is invalid/);
  assert.throws(() => evaluatePolicyGate(input('allowlist')), /mode is invalid/);
});

test('malformed or truncated GitHub inputs are rejected rather than projected', () => {
  const truncatedFiles = input();
  truncatedFiles.files.truncated = true;
  assert.throws(() => evaluatePolicyGate(truncatedFiles), /truncated/);

  const truncatedThreads = input();
  truncatedThreads.reviewThreads.truncated = true;
  assert.throws(() => evaluatePolicyGate(truncatedThreads), /truncated/);

  const comparisonDrift = input();
  comparisonDrift.comparison.head_sha = sha('c');
  const result = evaluatePolicyGate(comparisonDrift);
  assert.equal(result.verdict, 'block');
  assert.deepEqual(result.reason_codes, ['comparison_binding_mismatch']);

  const hiddenRename = input();
  hiddenRename.files.files = [{ filename: 'docs/new.md', status: 'renamed', previous_filename: null }];
  assert.throws(() => evaluatePolicyGate(hiddenRename), /previous filename/);

  const unknownStatus = input();
  unknownStatus.files.files = [{ filename: 'docs/new.md', status: 'unknown', previous_filename: null }];
  assert.throws(() => evaluatePolicyGate(unknownStatus), /status is invalid/);
});
