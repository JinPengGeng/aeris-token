import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import { MAX_WRITER_ARTIFACT_BYTES, readWriterArtifact, validateWriterArtifact, validateWriteIntentArtifact, validateWriterCandidateArtifact, validateWriterReceiptArtifact, writeWriterArtifactAtomic } from '../src/writer-phase-contract.mjs';

const sha = (character, length = 40) => character.repeat(length);
function intent(overrides = {}) {
  return {
    schema_version: 1, artifact_type: 'write_intent', intent: {
      repository_id: 123456, repository_name: 'aeris/token', issue_number: 12,
      issue_updated_at: '2026-08-18T09:00:00.000Z', input_sha: sha('b', 64), comment_id: 91, actor: 'maintainer',
      command: '/agent implement', base_sha: sha('c'), policy_sha: sha('d'), run_id: '12345',
      agent: 'writer', branch: 'agent/issue-12', expected_remote_head: null,
    }, ...overrides,
  };
}
function candidate(overrides = {}) {
  return {
    schema_version: 1, artifact_type: 'candidate', state: 'ready', intent: intent().intent,
    patch_sha: sha('e', 64), changed_paths: ['crates/aether/src/parser.rs'], file_sizes: [{ path: 'crates/aether/src/parser.rs', bytes: 42 }], file_count: 1, patch_bytes: 42, total_file_bytes: 42,
    limits: { maximum_files: 50, maximum_patch_bytes: 65536, maximum_file_size_bytes: 524288, maximum_total_file_bytes: 2097152, maximum_fix_cycles: 2 }, fix_cycle: 0,
    tests: { state: 'passed', commands: ['cargo test -p aether'], summary: 'parser tests pass' }, ...overrides,
  };
}
function receipt(overrides = {}) {
  return {
    schema_version: 1, artifact_type: 'receipt', state: 'draft_created', reason: 'draft_created', candidate: candidate(),
    commit_sha: sha('f'), ref: 'agent/issue-12', pr_number: 45, pr_url: 'https://github.com/aeris/token/pull/45', draft: true,
    ...overrides,
  };
}

test('writer artifacts bind all identity inputs and round-trip as copies', () => {
  const validated = validateWriteIntentArtifact(intent());
  assert.deepEqual(validated, intent());
  assert.notEqual(validated, intent());
  assert.equal(validateWriterCandidateArtifact(candidate()).intent.branch, 'agent/issue-12');
  assert.equal(validateWriterReceiptArtifact(receipt()).pr_number, 45);
});

test('writer artifacts reject unknown and secret-like keys at every level', () => {
  assert.throws(() => validateWriteIntentArtifact({ ...intent(), extra: true }), /unexpected keys/);
  assert.throws(() => validateWriteIntentArtifact({ ...intent(), intent: { ...intent().intent, api_token: 'nope' } }), /secret-like key/);
  assert.throws(() => validateWriterCandidateArtifact({ ...candidate(), tests: { ...candidate().tests, bearer: 'nope' } }), /secret-like key/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ tests: { ...candidate().tests, summary: 'Authorization: Bearer credential-value' } })), /sensitive value/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ tests: { ...candidate().tests, summary: 'found ghp_abcdefghijklmnopqrstuvwxyz1234567890 in output' } })), /sensitive value/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ tests: { ...candidate().tests, summary: '-----BEGIN OPENSSH PRIVATE KEY-----' } })), /sensitive value/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ tests: { ...candidate().tests, summary: 'OpenAI response included sk-abcdefghijklmnopqrstuvwxyz123456' } })), /sensitive value/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ tests: { ...candidate().tests, summary: 'OpenAI response included sk-proj-abcdefghijklmnopqrstuvwxyz123456' } })), /sensitive value/);
  assert.throws(() => validateWriterReceiptArtifact(receipt({ candidate: candidate({ tests: { ...candidate().tests, summary: 'AWS key ASIA1234567890ABCDEF appeared in nested candidate tests' } }) })), /sensitive value/);
  assert.equal(validateWriterCandidateArtifact(candidate({ tests: { ...candidate().tests, summary: 'The sk-project label and AKIA acronym are documentation examples.' } })).tests.summary, 'The sk-project label and AKIA acronym are documentation examples.');
});

test('intent fixes writer branch to its issue and validates command and hashes', () => {
  assert.throws(() => validateWriteIntentArtifact(intent({ intent: { ...intent().intent, branch: 'agent/issue-13' } })), /bind the issue number/);
  assert.throws(() => validateWriteIntentArtifact(intent({ intent: { ...intent().intent, command: '/agent implement parser' } })), /command format/);
  assert.throws(() => validateWriteIntentArtifact(intent({ intent: { ...intent().intent, input_sha: 'bad' } })), /input_sha format/);
  assert.throws(() => validateWriteIntentArtifact(intent({ intent: { ...intent().intent, command: '/agent implement', expected_remote_head: sha('a') } })), /implement must not bind a remote head/);
  assert.throws(() => validateWriteIntentArtifact(intent({ intent: { ...intent().intent, command: '/agent retry-write' } })), /retry-write must bind a remote head/);
  assert.equal(validateWriteIntentArtifact(intent({ intent: { ...intent().intent, command: '/agent retry-write', expected_remote_head: sha('a') } })).intent.command, '/agent retry-write');
  assert.throws(() => validateWriteIntentArtifact(intent({ intent: { ...intent().intent, issue_updated_at: 'not-a-timestamp' } })), /issue_updated_at format/);
});

test('candidate enforces actual path count, quota, patch state, and bounded tests', () => {
  assert.throws(() => validateWriterCandidateArtifact(candidate({ file_count: 2 })), /changed_paths count/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ patch_bytes: 65537 })), /patch_bytes/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ file_sizes: [{ path: 'crates/aether/src/parser.rs', bytes: 524289 }] })), /file_sizes\[0\] bytes/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ total_file_bytes: 43 })), /must equal file_sizes/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ changed_paths: ['../escape'] })), /unsafe segment/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ state: 'ready', patch_sha: null })), /requires a non-empty patch/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ changed_paths: ['crates/A.rs', 'crates/a.rs'], file_sizes: [{ path: 'crates/A.rs', bytes: 1 }, { path: 'crates/a.rs', bytes: 1 }], file_count: 2, total_file_bytes: 2 })), /case or NFC conflicts/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ changed_paths: ['cafe\u0301.rs'] })), /must be NFC/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ changed_paths: ['.github/workflows/write.yml'] })), /protected/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ changed_paths: ['src/CODEOWNERS/file'] })), /protected/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ changed_paths: ['C:/escape'] })), /relative and slash-separated/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ changed_paths: ['src/.Git/hooks/pre-commit'] })), /protected/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ changed_paths: ['src/config.json:payload'] })), /relative and slash-separated/);
  for (const invalidCharacter of ['<', '>', '"', '|', '?', '*']) {
    assert.throws(() => validateWriterCandidateArtifact(candidate({ changed_paths: [`src/report${invalidCharacter}final.rs`] })), /Windows-invalid character/);
  }
  assert.throws(() => validateWriterCandidateArtifact(candidate({ changed_paths: ['src/CoM1.rs'] })), /Windows-reserved segment/);
  for (const path of ['src/COM¹.rs', 'src/com²', 'src/CoM³.tmp', 'src/LPT¹.rs', 'src/lpt²', 'src/LpT³.tmp', 'src/CONIN$.rs', 'src/conout$', 'src/CLOCK$.tmp']) {
    assert.throws(() => validateWriterCandidateArtifact(candidate({ changed_paths: [path] })), /Windows-reserved segment/, path);
  }
  for (const path of ['src/com10.rs', 'src/lpt0', 'src/console.rs', 'src/conin.rs', 'src/conout.rs', 'src/clock.rs']) {
    assert.equal(validateWriterCandidateArtifact(candidate({ changed_paths: [path], file_sizes: [{ path, bytes: 42 }] })).changed_paths[0], path, path);
  }
  assert.throws(() => validateWriterCandidateArtifact(candidate({ changed_paths: ['src/file.'] })), /Windows-reserved segment/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ patch_bytes: 0 })), /requires a non-empty patch/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ fix_cycle: 1 })), /implement candidate fix_cycle must be zero/);
  assert.throws(() => validateWriterCandidateArtifact(candidate({ intent: { ...intent().intent, command: '/agent retry-write', expected_remote_head: sha('a') }, fix_cycle: 0 })), /retry-write candidate fix_cycle must be positive/);
  assert.equal(validateWriterCandidateArtifact(candidate({ intent: { ...intent().intent, command: '/agent retry-write', expected_remote_head: sha('a') }, fix_cycle: 2 })).fix_cycle, 2);
  const rejected = candidate({ state: 'rejected', patch_sha: null, changed_paths: [], file_sizes: [], file_count: 0, patch_bytes: 0, total_file_bytes: 0 });
  assert.equal(validateWriterCandidateArtifact(rejected).state, 'rejected');
});

test('receipt enforces command-bound published states and terminal candidate matrix', () => {
  assert.throws(() => validateWriterReceiptArtifact(receipt({ ref: 'agent/issue-99' })), /does not bind candidate branch/);
  assert.throws(() => validateWriterReceiptArtifact(receipt({ pr_url: 'http://github.com/aeris/token/pull/45' })), /GitHub HTTPS/);
  assert.throws(() => validateWriterReceiptArtifact(receipt({ pr_url: 'https://github.com/aeris/token/pull/45?tab=files' })), /query or hash/);
  const retryCandidate = candidate({ intent: { ...intent().intent, command: '/agent retry-write', expected_remote_head: sha('a') }, fix_cycle: 1 });
  assert.throws(() => validateWriterReceiptArtifact(receipt({ candidate: retryCandidate })), /draft_created receipt requires implement command/);
  assert.throws(() => validateWriterReceiptArtifact(receipt({ state: 'draft_updated' })), /draft_updated receipt requires retry-write command/);
  assert.equal(validateWriterReceiptArtifact(receipt({ state: 'draft_updated', candidate: retryCandidate, reason: 'draft_updated' })).state, 'draft_updated');
  assert.throws(() => validateWriterReceiptArtifact(receipt({ state: 'no_changes', reason: 'no_changes', commit_sha: null, ref: null, pr_number: null, pr_url: null, draft: null })), /no_changes receipt requires rejected candidate/);
  const rejectedCandidate = candidate({ state: 'rejected', patch_sha: null, changed_paths: [], file_sizes: [], file_count: 0, patch_bytes: 0, total_file_bytes: 0 });
  assert.equal(validateWriterReceiptArtifact(receipt({ state: 'no_changes', reason: 'no_changes', candidate: rejectedCandidate, commit_sha: null, ref: null, pr_number: null, pr_url: null, draft: null })).state, 'no_changes');
  assert.throws(() => validateWriterReceiptArtifact(receipt({ state: 'rejected', reason: 'rejected', commit_sha: null, ref: null, pr_number: null, pr_url: null, draft: null })), /rejected receipt requires rejected candidate/);
  const terminal = receipt({ state: 'stale', reason: 'remote_head_changed', commit_sha: null, ref: null, pr_number: null, pr_url: null, draft: null });
  assert.equal(validateWriterReceiptArtifact(terminal).state, 'stale');
  assert.throws(() => validateWriterReceiptArtifact(receipt({ state: 'failed' })), /terminal receipt must not claim/);
});

test('generic validation and atomic IO reject oversized files and leave no temporary artifact', (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-writer-contract-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const file = path.join(directory, 'nested', 'receipt.json');
  writeWriterArtifactAtomic(file, receipt(), 'receipt');
  assert.deepEqual(readWriterArtifact(file, 'receipt'), receipt());
  assert.deepEqual(fs.readdirSync(path.dirname(file)), ['receipt.json']);
  assert.equal(validateWriterArtifact(intent(), 'write_intent').artifact_type, 'write_intent');
  const oversized = path.join(directory, 'oversized.json');
  fs.writeFileSync(oversized, Buffer.alloc(MAX_WRITER_ARTIFACT_BYTES + 1));
  assert.throws(() => readWriterArtifact(oversized), /exceeds maximum size/);
});
