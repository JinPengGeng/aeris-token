import assert from 'node:assert/strict';
import test from 'node:test';

import {
  branchForIssue,
  evaluateWriterRequest,
  validateWriterChangeSet,
  writerLimitsFromContract,
} from '../src/writer-guard.mjs';

const switches = { globalEnabled: true, writerVariableEnabled: true, writerContractEnabled: true };
const issue = { number: 41, state: 'open', isPullRequest: false, labels: ['bug', 'agent-ready'] };
const limits = {
  maximumFiles: 2,
  maximumPatchBytes: 12,
  maximumFileBytes: 10,
  maximumTotalBytes: 15,
  maximumFixCycles: 2,
};

function request(overrides = {}) {
  return {
    command: '/agent implement',
    actorLogin: 'maintainer',
    actorPermission: 'write',
    issue,
    switches,
    fixCycle: 0,
    ...overrides,
  };
}

test('admits exact writer commands and assigns the deterministic Issue branch', () => {
  assert.deepEqual(evaluateWriterRequest(request()), { allowed: true, reason: null, branch: 'agent/issue-41' });
  assert.equal(evaluateWriterRequest(request({ command: '/agent retry-write', fixCycle: 1 })).allowed, true);
  assert.equal(branchForIssue(41), 'agent/issue-41');
  assert.equal(branchForIssue(0), null);
});

test('fails closed for command, authorization, switches, and Issue admission', () => {
  const cases = [
    [{ command: '/agent implement now' }, 'unsupported_command'],
    [{ actorLogin: 'github-actions[bot]' }, 'bot_actor_not_allowed'],
    [{ actorLogin: '' }, 'invalid_actor'],
    [{ actorLogin: 'not a login' }, 'invalid_actor'],
    [{ actorPermission: 'read' }, 'insufficient_permission'],
    [{ switches: { ...switches, globalEnabled: false } }, 'writer_disabled'],
    [{ switches: { ...switches, writerVariableEnabled: false } }, 'writer_disabled'],
    [{ switches: { ...switches, writerContractEnabled: false } }, 'writer_disabled'],
    [{ issue: { ...issue, state: 'closed' } }, 'issue_not_open'],
    [{ issue: { ...issue, isPullRequest: true } }, 'pull_request_not_allowed'],
    [{ issue: { ...issue, labels: [] } }, 'missing_agent_ready_label'],
    [{ issue: { ...issue, number: -1 } }, 'invalid_issue_number'],
    [{ fixCycle: 1 }, 'invalid_fix_cycle'],
    [{ command: '/agent retry-write', fixCycle: 0 }, 'invalid_fix_cycle'],
    [{ command: '/agent retry-write', fixCycle: 3 }, 'maximum_fix_cycles_exceeded'],
  ];
  for (const [overrides, reason] of cases) {
    assert.equal(evaluateWriterRequest(request(overrides)).reason, reason);
  }
  for (const actorPermission of ['admin', 'maintain']) assert.equal(evaluateWriterRequest(request({ actorPermission })).allowed, true);
});

test('accepts a bounded regular change set and includes its branch', () => {
  const result = evaluateWriterRequest(request({
    changeSet: [{ path: 'src/app.mjs', mode: '100644', bytes: 8 }],
    patchBytes: 8,
    limits,
  }));
  assert.deepEqual(result, { allowed: true, reason: null, branch: 'agent/issue-41', fileCount: 1, patchBytes: 8, totalBytes: 8 });
  assert.equal(evaluateWriterRequest(request({
    changeSet: [{ path: 'src/app.mjs', mode: '100644', bytes: 8 }],
    patchBytes: 13,
    limits,
  })).reason, 'maximum_patch_bytes_exceeded');
});

test('rejects every unsafe path class including both rename endpoints', () => {
  const cases = [
    ['/', 'path_absolute'], ['C:/x', 'path_absolute'], ['src\\x', 'path_backslash'], ['', 'invalid_path'],
    ['./x', 'path_segment'], ['x/../y', 'path_segment'], ['x//y', 'path_segment'], ['x\u0000y', 'path_control_character'],
    ['e\u0301.txt', 'path_not_nfc'], ['.github/workflows/a.yml', 'forbidden_path'], ['CODEOWNERS', 'forbidden_path'],
    ['docs/CODEOWNERS', 'forbidden_path'], ['.gitmodules', 'forbidden_path'], ['CODEOWNERS/data', 'forbidden_path'],
    ['src/.git/config', 'forbidden_path'], ['.GIT/hooks/x', 'forbidden_path'], ['file.txt:stream', 'path_ads'],
    ['dir/:stream', 'path_ads'], ['CON', 'path_windows_reserved'], ['aux.txt', 'path_windows_reserved'],
    ['Com1.LOG', 'path_windows_reserved'], ['dir/NuL. ', 'path_windows_normalization'],
    ['foo.', 'path_windows_normalization'], ['foo ', 'path_windows_normalization'],
    ['dir/name<suffix', 'path_windows_illegal_character'], ['dir/name>suffix', 'path_windows_illegal_character'],
    ['dir/name"suffix', 'path_windows_illegal_character'], ['dir/name|suffix', 'path_windows_illegal_character'],
    ['dir/name?suffix', 'path_windows_illegal_character'], ['dir/name*suffix', 'path_windows_illegal_character'],
  ];
  for (const [path, reason] of cases) {
    assert.equal(validateWriterChangeSet([{ path, mode: '100644', bytes: 1 }], limits, 1).reason, reason, path);
  }
  assert.equal(validateWriterChangeSet([{ path: 'safe.txt', previousPath: '.github/x', mode: '100644', bytes: 1 }], limits, 1).reason, 'forbidden_path');
  assert.equal(validateWriterChangeSet([{ path: 'safe.txt', fromPath: 'dir/.git/index', mode: '100644', bytes: 1 }], limits, 1).reason, 'forbidden_path');
  for (const field of ['previousPath', 'fromPath']) {
    assert.equal(validateWriterChangeSet([{
      path: 'safe.txt', [field]: 'dir/name?invalid', mode: '100644', bytes: 1,
    }], limits, 1).reason, 'path_windows_illegal_character', field);
  }
  for (const path of ['COM¹.log', 'com²', 'CoM³.tmp', 'LPT¹.log', 'lpt²', 'LpT³.tmp', 'CONIN$.log', 'conout$', 'CLOCK$.tmp']) {
    assert.equal(validateWriterChangeSet([{ path, mode: '100644', bytes: 1 }], limits, 1).reason, 'path_windows_reserved', path);
  }
  for (const field of ['previousPath', 'fromPath']) {
    for (const path of ['COM¹.log', 'LPT³', 'CONOUT$.tmp', 'clock$']) {
      assert.equal(validateWriterChangeSet([{ path: 'safe.txt', [field]: path, mode: '100644', bytes: 1 }], limits, 1).reason, 'path_windows_reserved', `${field}: ${path}`);
    }
  }
  for (const path of ['com10.txt', 'lpt0', 'console.txt', 'conin.txt', 'conout.txt', 'clock.txt']) {
    assert.equal(validateWriterChangeSet([{ path, mode: '100644', bytes: 1 }], limits, 1).allowed, true, path);
  }
  for (const field of ['oldPath', 'toPath', 'status']) {
    assert.equal(validateWriterChangeSet([{
      path: 'safe.txt', [field]: '.github/workflows/write.yml', mode: '100644', bytes: 1,
    }], limits, 1).reason, 'invalid_change', field);
  }
});

test('rejects Unicode and case-fold path collisions, links, malformed files, and size violations', () => {
  const cases = [
    [[{ path: 'A.txt', mode: '100644', bytes: 1 }, { path: 'a.txt', mode: '100644', bytes: 1 }], 'path_collision'],
    [[{ path: 'link', mode: '120000', bytes: 1 }], 'non_regular_mode'],
    [[{ path: 'submodule', mode: 0o160000, bytes: 1 }], 'non_regular_mode'],
    [[{ path: 'unknown-mode', bytes: 1 }], 'non_regular_mode'],
    [[{ path: 'x', mode: '100644', bytes: -1 }], 'invalid_file_bytes'],
    [[{ path: 'x', mode: '100644', bytes: 11 }], 'maximum_file_bytes_exceeded'],
    [[{ path: 'x', mode: '100644', bytes: 8 }, { path: 'y', mode: '100644', bytes: 8 }], 'maximum_total_bytes_exceeded'],
    [[{ path: 'a', mode: '100644', bytes: 1 }, { path: 'b', mode: '100644', bytes: 1 }, { path: 'c', mode: '100644', bytes: 1 }], 'maximum_files_exceeded'],
    [[], 'empty_change_set'],
  ];
  for (const [changeSet, reason] of cases) assert.equal(validateWriterChangeSet(changeSet, limits, 1).reason, reason);
  assert.equal(validateWriterChangeSet([{ path: 'x', mode: '100644', bytes: 1 }], limits).reason, 'invalid_patch_bytes');
  assert.equal(validateWriterChangeSet([{ path: 'x', mode: '100644', bytes: 1 }], limits, 0).reason, 'invalid_patch_bytes');
  assert.equal(validateWriterChangeSet([{ path: 'x', mode: '100644', bytes: 1 }], limits, 13).reason, 'maximum_patch_bytes_exceeded');
  assert.equal(validateWriterChangeSet([{ path: 'x', mode: '100644', bytes: 1 }], {}).reason, 'invalid_limits');
  assert.equal(validateWriterChangeSet([{ path: 'x', mode: '100644', bytes: 1 }], { ...limits, maximumFiles: 51 }, 1).reason, 'invalid_limits');
  assert.equal(validateWriterChangeSet([{ path: 'x', mode: '100644', bytes: 1 }], {
    ...limits,
    maximum_patch_bytes: 1,
  }, 1).reason, 'invalid_limits');
  assert.equal(validateWriterChangeSet([{ path: 'x', mode: '100644', bytes: 1 }], {
    ...limits,
    unexpected: 1,
  }, 1).reason, 'invalid_limits');
  assert.equal(validateWriterChangeSet([{ path: 'x', mode: 0o100755, bytes: 1 }], limits, 1).allowed, true);
  assert.deepEqual(writerLimitsFromContract({
    maximum_files: 50,
    maximum_file_size_bytes: 524_288,
    maximum_total_file_bytes: 2_097_152,
    maximum_patch_bytes: 65_536,
    maximum_fix_cycles: 2,
  }), {
    maximumFiles: 50, maximumPatchBytes: 65_536, maximumFileBytes: 524_288,
    maximumTotalBytes: 2_097_152, maximumFixCycles: 2,
  });
  assert.equal(writerLimitsFromContract({ maximum_files: 1, maximum_file_size_bytes: 1 }), null);
  assert.equal(writerLimitsFromContract({
    maximum_files: 1, maximum_file_size_bytes: 1, maximum_total_file_bytes: 1,
    maximum_patch_bytes: 1, maximum_fix_cycles: 1, maximumPatchBytes: 1,
  }), null);
  assert.equal(writerLimitsFromContract({
    maximum_files: 51,
    maximum_file_size_bytes: 15,
    maximum_total_file_bytes: 15,
    maximum_patch_bytes: 1,
    maximum_fix_cycles: 2,
  }), null);
  assert.equal(validateWriterChangeSet([{ path: 'x', mode: '100644', bytes: 1 }], {
    maximum_files: 51,
    maximum_file_size_bytes: 15,
    maximum_total_file_bytes: 15,
    maximum_patch_bytes: 1,
    maximum_fix_cycles: 2,
  }).reason, 'invalid_limits');
  assert.equal(writerLimitsFromContract({
    maximum_files: 1, maximum_file_size_bytes: 1, maximum_total_file_bytes: 1,
    maximum_patch_bytes: 65_537, maximum_fix_cycles: 2,
  }), null);
  assert.equal(writerLimitsFromContract({
    maximum_files: 1, maximum_file_size_bytes: 1, maximum_total_file_bytes: 1,
    maximum_patch_bytes: 1, maximum_fix_cycles: 3,
  }), null);
  assert.equal(validateWriterChangeSet([{ path: 'x', mode: '100644', bytes: 1 }], { max_files: 2, max_bytes: 15 }, 1).reason, 'invalid_limits');
});
