import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import yaml from 'js-yaml';

import {
  CHANGE_FILTERS_PATH,
  loadChangeFilters,
  matchedChangeFilterGroups,
  validateChangeFilters,
} from '../src/change-filters.mjs';
import { ContractError } from '../src/config.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, '..', '..', '..');

const EXPECTED_GROUPS = {
  rust: [
    'Cargo.toml',
    'Cargo.lock',
    'rust-toolchain.toml',
    'crates/**',
    'apps/**',
    '.github/workflows/rust-ci.yml',
  ],
  data: [
    'Cargo.toml',
    'Cargo.lock',
    'rust-toolchain.toml',
    'crates/aether-data/**',
    '.github/workflows/rust-ci.yml',
  ],
  frontend: ['frontend/**', '.github/workflows/frontend-ci.yml'],
  automation: [
    '.github/agents.yml',
    '.github/ai-executors.json',
    '.github/automation-policy.yml',
    '.github/change-filters.yml',
    '.github/automation/**',
    '.github/upstream-sync-policy.yml',
    '.github/workflows/**',
  ],
};

test('change-filters.yml keeps the four CI filter groups pinned', () => {
  const parsed = yaml.load(fs.readFileSync(path.join(repoRoot, CHANGE_FILTERS_PATH), 'utf8'));
  assert.deepEqual(parsed, EXPECTED_GROUPS);
});

test('both CI workflows consume the shared change filters file', () => {
  for (const workflow of ['.github/workflows/rust-ci.yml', '.github/workflows/frontend-ci.yml']) {
    const text = fs.readFileSync(path.join(repoRoot, workflow), 'utf8').replace(/\r\n/g, '\n');
    assert.match(text, /^\s+filters: \.github\/change-filters\.yml$/m, workflow);
    assert.doesNotMatch(text, /filters: \|/, workflow);
  }
});

test('the runtime matcher loads the shared file from the trusted checkout', () => {
  const filters = loadChangeFilters(repoRoot);
  assert.deepEqual(Object.keys(filters), ['rust', 'data', 'frontend', 'automation']);
});

test('matcher classifies representative paths exactly like the CI filters', () => {
  const filters = loadChangeFilters(repoRoot);
  const cases = [
    // Docs-only and otherwise unfiltered changes hit no group.
    [['docs/guide.md', 'README.md'], []],
    [['LICENSE'], []],
    [['aether-vscodex/web/src/app.ts'], []],
    // Rust workspace changes.
    [['crates/aether-gateway/src/lib.rs'], ['rust']],
    [['crates/aether-data/src/lib.rs'], ['data', 'rust']],
    [['apps/aether-gateway/src/main.rs'], ['rust']],
    [['Cargo.toml'], ['data', 'rust']],
    [['Cargo.lock'], ['data', 'rust']],
    [['rust-toolchain.toml'], ['data', 'rust']],
    // Frontend changes.
    [['frontend/src/app.ts'], ['frontend']],
    // Automation and workflow changes.
    [['.github/agents.yml'], ['automation']],
    [['.github/automation/src/engine.mjs'], ['automation']],
    [['.github/change-filters.yml'], ['automation']],
    [['.github/workflows/rust-ci.yml'], ['automation', 'data', 'rust']],
    [['.github/workflows/frontend-ci.yml'], ['automation', 'frontend']],
    [['.github/workflows/agent-pr-review.yml'], ['automation']],
    // Mixed changes report every matched group.
    [['docs/guide.md', 'frontend/package.json'], ['frontend']],
    [['crates/aether-data/src/lib.rs', 'frontend/src/app.ts'], ['data', 'frontend', 'rust']],
  ];
  for (const [files, expected] of cases) {
    assert.deepEqual(matchedChangeFilterGroups(filters, files).sort(), [...expected].sort(), files.join(', '));
  }
});

test('exact patterns do not leak into nested or sibling paths', () => {
  const filters = validateChangeFilters({ pinned: ['Cargo.toml', 'crates/**'] });
  assert.deepEqual(matchedChangeFilterGroups(filters, ['Cargo.toml']), ['pinned']);
  assert.deepEqual(matchedChangeFilterGroups(filters, ['sub/Cargo.toml']), []);
  assert.deepEqual(matchedChangeFilterGroups(filters, ['Cargo.toml.bak']), []);
  assert.deepEqual(matchedChangeFilterGroups(filters, ['crates/aether-data/src/lib.rs']), ['pinned']);
  assert.deepEqual(matchedChangeFilterGroups(filters, ['crates']), []);
  assert.deepEqual(matchedChangeFilterGroups(filters, ['cratesfoo/x.rs']), []);
});

test('patterns outside the exact_or_directory_recursive subset fail closed', () => {
  const unsupported = [
    '!docs/**',
    '/absolute',
    'trailing/',
    'back\\slash',
    'bracket[0-9]',
    'quest?on',
    'mid/**/*.rs',
    '**/*.rs',
    '**',
    '',
    42,
    null,
  ];
  for (const pattern of unsupported) {
    assert.throws(() => validateChangeFilters({ group: [pattern] }), ContractError, String(pattern));
  }
  assert.throws(() => validateChangeFilters(null), ContractError);
  assert.throws(() => validateChangeFilters({}), ContractError);
  assert.throws(() => validateChangeFilters({ group: [] }), ContractError);
  assert.throws(() => validateChangeFilters({ group: 'crates/**' }), ContractError);
});
