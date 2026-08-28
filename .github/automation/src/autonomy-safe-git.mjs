import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const NULL_DEVICE = process.platform === 'win32' ? 'NUL' : '/dev/null';
const SHA = /^[0-9a-f]{40}$/;
const SAFE_REF = /^refs\/(?:heads|tags)\/[A-Za-z0-9._/-]+$/;
const PACKED_REF = /^refs\/[A-Za-z0-9._/-]+$/;
const MAXIMUM_GIT_OUTPUT = 2 * 1024 * 1024;

export class SafeGitError extends Error {
  constructor(message) {
    super(message);
    this.name = 'SafeGitError';
  }
}

function reject(message) {
  throw new SafeGitError(message);
}

function regularFile(file, name) {
  let stat;
  try {
    stat = fs.lstatSync(file);
  } catch {
    reject(`${name} is missing`);
  }
  if (!stat.isFile() || stat.isSymbolicLink()) reject(`${name} must be a regular file`);
}

function directory(file, name) {
  let stat;
  try {
    stat = fs.lstatSync(file);
  } catch {
    reject(`${name} is missing`);
  }
  if (!stat.isDirectory() || stat.isSymbolicLink()) reject(`${name} must be a real directory`);
}

function readSmallFile(file, name, maximumBytes = 1024) {
  regularFile(file, name);
  const stat = fs.statSync(file);
  if (stat.size <= 0 || stat.size > maximumBytes) reject(`${name} size is invalid`);
  return fs.readFileSync(file, 'utf8');
}

function parseSha(value, name) {
  const normalized = value.trim();
  if (!SHA.test(normalized)) reject(`${name} is invalid`);
  return normalized;
}

function resolveLooseRef(gitDirectory, ref) {
  const candidate = path.resolve(gitDirectory, ...ref.split('/'));
  const prefix = `${path.resolve(gitDirectory)}${path.sep}`;
  if (!candidate.startsWith(prefix)) reject('repository HEAD ref escapes the Git directory');
  if (!fs.existsSync(candidate)) return null;
  return parseSha(readSmallFile(candidate, 'repository HEAD ref'), 'repository HEAD ref');
}

function resolvePackedRef(gitDirectory, ref) {
  const packed = path.join(gitDirectory, 'packed-refs');
  if (!fs.existsSync(packed)) return null;
  const content = readSmallFile(packed, 'repository packed refs', 4 * 1024 * 1024);
  let match = null;
  for (const line of content.split(/\r?\n/)) {
    if (!line || line.startsWith('#') || line.startsWith('^')) continue;
    const separator = line.indexOf(' ');
    if (separator !== 40) reject('repository packed refs are malformed');
    const sha = line.slice(0, separator);
    const name = line.slice(separator + 1);
    if (!SHA.test(sha) || !PACKED_REF.test(name) || name.includes('..') || name.includes('//')) {
      reject('repository packed refs are malformed');
    }
    if (name === ref) {
      if (match !== null) reject('repository HEAD ref is duplicated');
      match = sha;
    }
  }
  return match;
}

function readRepositoryHead(gitDirectory) {
  const value = readSmallFile(path.join(gitDirectory, 'HEAD'), 'repository HEAD').trim();
  if (SHA.test(value)) return value;
  if (!value.startsWith('ref: ')) reject('repository HEAD is malformed');
  const ref = value.slice(5);
  if (!SAFE_REF.test(ref) || ref.includes('..') || ref.includes('//')) {
    reject('repository HEAD ref is malformed');
  }
  const sha = resolveLooseRef(gitDirectory, ref) ?? resolvePackedRef(gitDirectory, ref);
  if (sha === null) reject('repository HEAD ref cannot be resolved');
  return sha;
}

function safeProcessEnvironment(scratch, values) {
  const environment = {};
  for (const name of ['PATH', 'SystemRoot', 'SYSTEMROOT', 'WINDIR', 'ComSpec', 'COMSPEC', 'PATHEXT']) {
    if (typeof process.env[name] === 'string' && process.env[name].length > 0) {
      environment[name] = process.env[name];
    }
  }
  const home = path.join(scratch, 'home');
  const xdg = path.join(scratch, 'xdg');
  fs.mkdirSync(home, { recursive: true, mode: 0o700 });
  fs.mkdirSync(xdg, { recursive: true, mode: 0o700 });
  return {
    ...environment,
    HOME: home,
    XDG_CONFIG_HOME: xdg,
    TMPDIR: scratch,
    TMP: scratch,
    TEMP: scratch,
    LC_ALL: 'C',
    GIT_CONFIG_NOSYSTEM: '1',
    GIT_CONFIG_SYSTEM: NULL_DEVICE,
    GIT_CONFIG_GLOBAL: NULL_DEVICE,
    GIT_CONFIG_COUNT: '0',
    GIT_ATTR_NOSYSTEM: '1',
    GIT_TERMINAL_PROMPT: '0',
    GIT_OPTIONAL_LOCKS: '0',
    GIT_NO_REPLACE_OBJECTS: '1',
    GIT_PAGER: 'cat',
    ...values,
  };
}

export function createSafeGitContext({ repositoryRoot, baseSha, temporaryDirectory = os.tmpdir() }) {
  if (typeof repositoryRoot !== 'string' || repositoryRoot.length === 0) reject('repositoryRoot is invalid');
  if (typeof temporaryDirectory !== 'string' || temporaryDirectory.length === 0) {
    reject('temporaryDirectory is invalid');
  }
  if (!SHA.test(baseSha)) reject('baseSha is invalid');

  const requestedRoot = path.resolve(repositoryRoot);
  const root = fs.realpathSync.native(requestedRoot);
  const sourceGitDirectory = path.join(root, '.git');
  directory(sourceGitDirectory, 'repository Git directory');
  const sourceObjects = path.join(sourceGitDirectory, 'objects');
  directory(sourceObjects, 'repository object directory');
  if (readRepositoryHead(sourceGitDirectory) !== baseSha) {
    reject('repository HEAD changed during Agent execution');
  }

  const scratchParent = fs.realpathSync.native(path.resolve(temporaryDirectory));
  const scratch = fs.mkdtempSync(path.join(scratchParent, 'aeris-safe-git-'));
  const gitDirectory = path.join(scratch, 'git');
  const objectDirectory = path.join(gitDirectory, 'objects');
  const indexFile = path.join(scratch, 'index');
  fs.mkdirSync(path.join(objectDirectory, 'info'), { recursive: true, mode: 0o700 });
  fs.mkdirSync(path.join(objectDirectory, 'pack'), { recursive: true, mode: 0o700 });
  fs.mkdirSync(path.join(gitDirectory, 'refs', 'heads'), { recursive: true, mode: 0o700 });
  fs.writeFileSync(path.join(gitDirectory, 'HEAD'), `${baseSha}\n`, { mode: 0o600 });
  fs.writeFileSync(
    path.join(gitDirectory, 'config'),
    '[core]\n\trepositoryformatversion = 0\n\tbare = false\n',
    { mode: 0o600 },
  );

  const environment = safeProcessEnvironment(scratch, {
    GIT_DIR: gitDirectory,
    GIT_COMMON_DIR: gitDirectory,
    GIT_WORK_TREE: root,
    GIT_INDEX_FILE: indexFile,
    GIT_OBJECT_DIRECTORY: objectDirectory,
    GIT_ALTERNATE_OBJECT_DIRECTORIES: sourceObjects,
  });
  let disposed = false;

  function run(args, { encoding = 'utf8' } = {}) {
    if (disposed) reject('safe Git context is disposed');
    if (!Array.isArray(args) || args.length === 0 || args.some((value) => typeof value !== 'string')) {
      reject('Git arguments are invalid');
    }
    try {
      return execFileSync('git', args, {
        cwd: root,
        encoding,
        env: environment,
        timeout: 30_000,
        maxBuffer: MAXIMUM_GIT_OUTPUT,
        stdio: ['ignore', 'pipe', 'pipe'],
      });
    } catch (error) {
      const stderr = typeof error?.stderr === 'string' ? error.stderr.trim() : '';
      reject(`git ${args[0]} failed${stderr ? `: ${stderr}` : ''}`);
    }
  }

  function dispose() {
    if (disposed) return;
    disposed = true;
    fs.rmSync(scratch, { recursive: true, force: true });
  }

  try {
    run(['cat-file', '-e', `${baseSha}^{commit}`]);
    run(['read-tree', baseSha]);
  } catch (error) {
    dispose();
    throw error;
  }

  return Object.freeze({ root, run, dispose });
}
