import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const SHA = /^[0-9a-f]{40}$/;
const MAX_BYTES = 16 * 1024 * 1024;

function requireCondition(condition, message) { if (!condition) throw new Error(message); }
function hash(value) { return createHash('sha256').update(value).digest('hex'); }

export function collectExactPullDiff({ repoRoot, repository, pullNumber, baseSha, headSha, maximumBytes = MAX_BYTES, exec = execFileSync }) {
  requireCondition(SHA.test(baseSha) && SHA.test(headSha), 'exact diff SHAs are invalid');
  requireCondition(Number.isSafeInteger(pullNumber) && pullNumber > 0, 'exact diff pull number is invalid');
  requireCondition(Number.isSafeInteger(maximumBytes) && maximumBytes > 0 && maximumBytes <= MAX_BYTES, 'exact diff byte limit is invalid');
  const options = { cwd: repoRoot, encoding: 'buffer', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: maximumBytes + 1, env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: process.platform === 'win32' ? 'NUL' : '/dev/null' } };
  const git = (args, limit = maximumBytes + 1) => exec('git', ['-c', 'protocol.file.allow=never', '-c', 'protocol.ext.allow=never', ...args], { ...options, maxBuffer: limit });
  const expectedUrl = `https://github.com/${repository}.git`;
  const origin = git(['remote', 'get-url', 'origin'], 4096).toString('utf8').trim().replace(/\/$/, '');
  requireCondition(origin === expectedUrl || origin === expectedUrl.slice(0, -4), 'exact diff origin does not match repository');
  git(['fetch', '--no-tags', '--no-recurse-submodules', '--depth=1', origin, baseSha], 1024 * 1024);
  git(['fetch', '--no-tags', '--no-recurse-submodules', '--depth=1', origin, `refs/pull/${pullNumber}/head`], 1024 * 1024);
  requireCondition(git(['rev-parse', 'FETCH_HEAD'], 4096).toString('utf8').trim() === headSha, 'exact diff pull ref changed');
  for (const sha of [baseSha, headSha]) requireCondition(git(['cat-file', '-t', sha], 4096).toString('utf8').trim() === 'commit', 'exact diff commit is unavailable');

  const raw = git(['diff', '--raw', '--full-index', '--no-abbrev', '--no-renames', '--no-ext-diff', '--no-textconv', '-z', baseSha, headSha, '--']);
  let rawText;
  try { rawText = new TextDecoder('utf-8', { fatal: true }).decode(raw); } catch { throw new Error('exact diff paths are not UTF-8'); }
  const fields = rawText.split('\0');
  const files = [];
  for (let index = 0; index < fields.length - 1; index += 2) {
    const match = /^:([0-7]{6}) ([0-7]{6}) ([0-9a-f]{40}) ([0-9a-f]{40}) ([A-Z])$/.exec(fields[index]);
    requireCondition(match && fields[index + 1].length > 0, 'exact diff raw manifest is invalid');
    requireCondition(match[1] !== '160000' && match[2] !== '160000', 'exact diff contains a submodule');
    files.push({ path: fields[index + 1], status: match[5], old_blob_sha: match[3], new_blob_sha: match[4] });
  }
  requireCondition(files.length > 0 && files.length <= 300, 'exact diff file count is invalid');
  const numstat = git(['diff', '--numstat', '--no-renames', '--no-ext-diff', '--no-textconv', '-z', baseSha, headSha, '--']).toString('utf8');
  requireCondition(!numstat.split('\0').some((entry) => /^-\t-\t/.test(entry)), 'exact diff contains binary content');
  const patch = git(['diff', '--binary', '--full-index', '--no-renames', '--no-ext-diff', '--no-textconv', baseSha, headSha, '--']);
  requireCondition(patch.length > 0 && patch.length <= maximumBytes, 'exact diff exceeds byte limit');
  requireCondition(!patch.includes(0), 'exact diff contains binary content');
  const manifest = files.sort((left, right) => left.path.localeCompare(right.path));
  let patchText;
  try { patchText = new TextDecoder('utf-8', { fatal: true }).decode(patch); } catch { throw new Error('exact diff is not UTF-8'); }
  return { files: manifest, patch: patchText, evidence: { base_sha: baseSha, head_sha: headSha, manifest_sha: hash(Buffer.from(JSON.stringify(manifest))), raw_diff_sha: hash(patch), file_count: manifest.length, patch_bytes: patch.length } };
}
