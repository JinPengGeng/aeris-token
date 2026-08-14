import assert from 'node:assert/strict';
import test from 'node:test';

import { evaluateRequiredChecks } from '../src/required-checks.mjs';

const required = ['Rust CI / check', 'Frontend CI / check'];
const success = (id, name, completedAt = '2026-08-13T00:00:00Z') => ({
  id,
  name,
  status: 'completed',
  conclusion: 'success',
  completed_at: completedAt,
});

test('required checks reject one success plus one pending result', () => {
  const result = evaluateRequiredChecks(
    required,
    [
      success(1, required[0]),
      { id: 2, name: required[1], status: 'in_progress', conclusion: null, started_at: '2026-08-13T00:01:00Z' },
    ],
    [],
  );
  assert.deepEqual(result, { ready: false, unsuccessful: [required[1]] });
});

test('required checks reject one success plus one failure result', () => {
  const result = evaluateRequiredChecks(
    required,
    [success(1, required[0])],
    [{ id: 2, context: required[1], state: 'failure', updated_at: '2026-08-13T00:01:00Z' }],
  );
  assert.deepEqual(result, { ready: false, unsuccessful: [required[1]] });
});

test('required checks reject a missing context', () => {
  const result = evaluateRequiredChecks(required, [success(1, required[0])], []);
  assert.deepEqual(result, { ready: false, unsuccessful: [required[1]] });
});

test('required checks accept both successful contexts using only the latest result', () => {
  const result = evaluateRequiredChecks(
    required,
    [
      { id: 1, name: required[0], status: 'completed', conclusion: 'failure', completed_at: '2026-08-13T00:00:00Z' },
      success(2, required[0], '2026-08-13T00:02:00Z'),
    ],
    [{ id: 3, context: required[1], state: 'success', updated_at: '2026-08-13T00:03:00Z' }],
  );
  assert.deepEqual(result, { ready: true, unsuccessful: [] });
});

test('a newer pending rerun overrides an older success that completes later', () => {
  const result = evaluateRequiredChecks(
    [required[0]],
    [
      success(100, required[0], '2026-08-13T00:02:00Z'),
      {
        id: 101,
        name: required[0],
        status: 'in_progress',
        conclusion: null,
        started_at: '2026-08-13T00:01:00Z',
      },
    ],
    [],
  );
  assert.deepEqual(result, { ready: false, unsuccessful: [required[0]] });
});

test('a check run is authoritative over a same-context commit status', () => {
  const result = evaluateRequiredChecks(
    [required[0]],
    [{ id: 1, name: required[0], status: 'completed', conclusion: 'failure', started_at: '2026-08-13T00:00:00Z' }],
    [{ id: 2, context: required[0], state: 'success', created_at: '2026-08-13T00:01:00Z' }],
  );
  assert.deepEqual(result, { ready: false, unsuccessful: [required[0]] });

  const successfulCheck = evaluateRequiredChecks(
    [required[0]],
    [{ id: 3, name: required[0], status: 'completed', conclusion: 'success', started_at: '2026-08-13T00:02:00Z' }],
    [{ id: 4, context: required[0], state: 'failure', created_at: '2026-08-13T00:03:00Z' }],
  );
  assert.deepEqual(successfulCheck, { ready: true, unsuccessful: [] });
});
