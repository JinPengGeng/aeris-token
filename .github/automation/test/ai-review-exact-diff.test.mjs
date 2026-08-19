import assert from 'node:assert/strict';
import test from 'node:test';

import { collectExactPullDiff } from '../src/ai-review-exact-diff.mjs';

const sha = (character) => character.repeat(40);
const base = sha('a');
const head = sha('b');

function executor(overrides = {}) {
  return (_file, argv) => {
    const args = argv.slice(4);
    const command = args.join(' ');
    if (command === 'remote get-url origin') return Buffer.from(overrides.origin ?? 'https://github.com/JinPengGeng/aeris-token.git\n');
    if (args[0] === 'fetch') return Buffer.alloc(0);
    if (command === 'rev-parse FETCH_HEAD') return Buffer.from(`${overrides.fetchHead ?? head}\n`);
    if (args[0] === 'cat-file') return Buffer.from('commit\n');
    if (args[0] === 'diff' && args.includes('--raw')) return Buffer.from(overrides.raw ?? `:100644 100644 ${sha('c')} ${sha('d')} M\0README.md\0`);
    if (args[0] === 'diff' && args.includes('--numstat')) return Buffer.from(overrides.numstat ?? '1\t1\tREADME.md\0');
    if (args[0] === 'diff' && args.includes('--binary')) return Buffer.from(overrides.patch ?? 'diff --git a/README.md b/README.md\n-old\n+new\n');
    throw new Error(`unexpected git command: ${command}`);
  };
}

const collect = (overrides = {}, values = {}) => collectExactPullDiff({ repoRoot: 'repo', repository: 'JinPengGeng/aeris-token', pullNumber: 37, baseSha: base, headSha: head, exec: executor(overrides), ...values });

test('exact diff binds the fetched PR head and complete manifest hashes', () => {
  const value = collect();
  assert.equal(value.evidence.base_sha, base);
  assert.equal(value.evidence.head_sha, head);
  assert.equal(value.files[0].path, 'README.md');
  assert.equal(value.evidence.manifest_sha.length, 64);
  assert.equal(value.evidence.raw_diff_sha.length, 64);
});

test('exact diff rejects head races, origin drift, binary, submodule, and oversize data', () => {
  assert.throws(() => collect({ fetchHead: sha('e') }), /pull ref changed/);
  assert.throws(() => collect({ origin: 'https://github.com/other/repo.git' }), /origin/);
  assert.throws(() => collect({ numstat: '-\t-\timage.png\0' }), /binary/);
  assert.throws(() => collect({ patch: Buffer.from([100, 105, 102, 102, 0, 1]) }), /binary/);
  assert.throws(() => collect({ raw: `:160000 160000 ${sha('c')} ${sha('d')} M\0vendor\0` }), /submodule/);
  assert.throws(() => collect({ patch: 'x'.repeat(65) }, { maximumBytes: 64 }), /byte limit/);
});

test('exact diff rejects command-shaped SHA input before invoking git', () => {
  assert.throws(() => collect({}, { headSha: `${head};--upload-pack=evil` }), /SHAs/);
});
