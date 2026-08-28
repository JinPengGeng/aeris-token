import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  AutonomyPreflightError,
  evaluateAutonomyPreflight,
  writeAutonomyPreflightOutput,
} from '../src/autonomy-preflight.mjs';

const SHA = 'a'.repeat(40);

function input(overrides = {}) {
  return {
    repository: 'JinPengGeng/aeris-token',
    repository_id: 1310462380,
    issue_number: 123,
    actor: 'JinPengGeng',
    base_ref: 'refs/heads/main',
    ...overrides,
  };
}

function client(overrides = {}) {
  return {
    async request(_method, endpoint) {
      if (endpoint.endsWith('/git/ref/heads/main')) return overrides.ref ?? { object: { sha: SHA } };
      return overrides.repository ?? {
        id: 1310462380,
        full_name: 'JinPengGeng/aeris-token',
        default_branch: 'main',
        archived: false,
      };
    },
    async getIssue() {
      return overrides.issue ?? {
        number: 123,
        state: 'open',
        updated_at: '2026-08-20T00:00:00Z',
        labels: [{ name: 'agent-ready' }],
      };
    },
    async getCollaboratorPermission() {
      return overrides.permission ?? 'admin';
    },
  };
}

test('accepts a write-authorized dispatch for an open agent-ready Issue', async () => {
  const result = await evaluateAutonomyPreflight(input(), client());
  assert.equal(result.base_sha, SHA);
  assert.equal(result.task_id, 'issue:123');
  assert.equal(Object.isFrozen(result), true);
});

test('creates a missing parent directory for the bound preflight output', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-preflight-'));
  const output = path.join(root, 'nested', 'preflight.json');
  try {
    writeAutonomyPreflightOutput(output, { task_id: 'issue:123' });
    assert.deepEqual(JSON.parse(fs.readFileSync(output, 'utf8')), { task_id: 'issue:123' });
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('rejects a non-writer actor', async () => {
  await assert.rejects(
    evaluateAutonomyPreflight(input(), client({ permission: 'read' })),
    (error) => error instanceof AutonomyPreflightError && /lacks repository write/.test(error.message),
  );
});

test('rejects a missing agent-ready label', async () => {
  await assert.rejects(
    evaluateAutonomyPreflight(input(), client({
      issue: { number: 123, state: 'open', updated_at: '2026-08-20T00:00:00Z', labels: [] },
    })),
    /agent-ready/,
  );
});

test('rejects repository identity or default branch drift', async () => {
  await assert.rejects(
    evaluateAutonomyPreflight(input(), client({
      repository: {
        id: 999,
        full_name: 'JinPengGeng/aeris-token',
        default_branch: 'main',
        archived: false,
      },
    })),
    /repository identity/,
  );
});
