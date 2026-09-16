import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const yaml = require('js-yaml');
const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');

const read = (name) => fs.readFileSync(path.join(repoRoot, name), 'utf8');
const trackedFiles = execFileSync('git', ['ls-files'], {
  cwd: repoRoot,
  encoding: 'utf8',
}).trim().split('\n').filter(Boolean);
const trackedFileSet = new Set(trackedFiles);
const dependabot = yaml.load(read('.github/dependabot.yml'));

const normalizeDirectory = (directory) => {
  const normalized = directory.replace(/^\/+|\/+$/g, '');
  return normalized || '.';
};

const updateDirectories = (update) => {
  const configured = [];
  if (typeof update.directory === 'string') configured.push(update.directory);
  if (Array.isArray(update.directories)) configured.push(...update.directories);
  assert.ok(configured.length > 0, `updater has no directory: ${JSON.stringify(update)}`);
  assert.equal(
    new Set(configured.map(normalizeDirectory)).size,
    configured.length,
    `updater repeats a directory: ${JSON.stringify(update)}`,
  );
  return configured.map(normalizeDirectory);
};

const updatesFor = (ecosystem) => dependabot.updates
  .filter((update) => update['package-ecosystem'] === ecosystem);

const trackedPackageProjects = trackedFiles
  .filter((file) => /(^|\/)package-lock\.json$/.test(file))
  .map((lockfile) => normalizeDirectory(path.posix.dirname(lockfile)))
  .filter((directory) => trackedFileSet.has(path.posix.join(directory, 'package.json')));

const orphanPackageLockDirectories = trackedFiles
  .filter((file) => /(^|\/)package-lock\.json$/.test(file))
  .map((lockfile) => normalizeDirectory(path.posix.dirname(lockfile)))
  .filter((directory) => !trackedFileSet.has(path.posix.join(directory, 'package.json')));

const trackedDockerDirectories = trackedFiles
  .filter((file) => /dockerfile|containerfile/i.test(path.posix.basename(file)))
  .map((file) => normalizeDirectory(path.posix.dirname(file)));

const assertExactlyOnce = (configuredDirectories, expectedDirectories, label) => {
  for (const directory of [...new Set(expectedDirectories)]) {
    assert.equal(
      configuredDirectories.filter((candidate) => candidate === directory).length,
      1,
      `${label} must configure ${directory} exactly once`,
    );
  }
  for (const directory of new Set(configuredDirectories)) {
    assert.ok(
      expectedDirectories.includes(directory),
      `${label} configures an untracked directory: ${directory}`,
    );
  }
};

test('Dependabot npm coverage matches every tracked manifest and lockfile pair', () => {
  const npmDirectories = updatesFor('npm').flatMap(updateDirectories);
  assertExactlyOnce(npmDirectories, trackedPackageProjects, 'npm updater');
  assert.deepEqual(
    [...new Set(npmDirectories)].sort(),
    [...new Set(trackedPackageProjects)].sort(),
    'npm updater coverage must exactly match tracked package projects',
  );

  const orphanTargets = npmDirectories.filter((directory) =>
    orphanPackageLockDirectories.includes(directory));
  assert.deepEqual(
    orphanTargets,
    [],
    'orphan package-lock.json files must not be configured as npm projects',
  );
});

test('Dependabot Docker coverage matches every tracked Dockerfile directory', () => {
  const dockerDirectories = updatesFor('docker').flatMap(updateDirectories);
  assertExactlyOnce(dockerDirectories, trackedDockerDirectories, 'Docker updater');
  assert.deepEqual(
    [...new Set(dockerDirectories)].sort(),
    [...new Set(trackedDockerDirectories)].sort(),
    'Docker updater coverage must exactly match tracked Dockerfile directories',
  );
});

test('npm and Docker updater entries use bounded review cadence', () => {
  for (const update of [...updatesFor('npm'), ...updatesFor('docker')]) {
    assert.equal(update.schedule?.interval, 'weekly');
    const limit = update['open-pull-requests-limit'];
    assert.ok(
      Number.isInteger(limit) && limit > 0 && limit <= 5,
      `updater PR limit must be a positive integer no greater than five: ${limit}`,
    );

    const groups = Object.values(update.groups ?? {});
    if (update.directories || update['package-ecosystem'] === 'docker') {
      assert.ok(groups.length > 0, `grouped updater must define a minor/patch group: ${JSON.stringify(update)}`);
    }
    for (const group of groups) {
      assert.deepEqual(
        [...(group['update-types'] ?? [])].sort(),
        ['minor', 'patch'],
        `updater groups must retain minor/patch coverage: ${JSON.stringify(update)}`,
      );
    }
  }
});
