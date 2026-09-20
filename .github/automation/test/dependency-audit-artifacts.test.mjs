import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import yaml from 'js-yaml';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const workflow = yaml.load(fs.readFileSync(path.join(repoRoot, '.github/workflows/dependency-audit.yml'), 'utf8'));

test('npm audit evidence is JSON and the actual scanner exit status remains required', () => {
  const job = workflow.jobs.npm;
  const step = job.steps.find((candidate) => candidate.name === 'Verify lockfile and audit dependencies');
  const upload = job.steps.find((candidate) => candidate.name === 'Upload npm advisory evidence');
  assert.equal(upload.if, 'always()');
  assert.equal(upload.with.path, '${{ matrix.directory }}/audit-artifacts/npm');
  assert.equal(step['continue-on-error'], undefined);
  assert.equal(job['continue-on-error'], undefined);
  assert.ok(workflow.jobs.check.needs.includes('npm'));

  const lockfiles = job.strategy.matrix.directory.map((directory) =>
    directory === '.' ? 'package-lock.json' : `${directory}/package-lock.json`);
  const fixture = fs.mkdtempSync(path.join(process.env.AGENT_TMP_DIR || os.tmpdir(), 'npm-audit-artifacts-'));
  try {
    fs.writeFileSync(path.join(fixture, 'package-lock.json'), '{}');
    const prelude = `
      git() { printf '%s\\n' "$MOCK_LOCKFILES"; }
      npm() {
        if [[ "$1" == --version ]]; then printf '10.9.2\\n'; return 0; fi
        if [[ "$*" != *" --json "* ]]; then
          printf 'found 0 vulnerabilities\\n'
          return 0
        fi
        printf '{"auditReportVersion":2,"vulnerabilities":{}}\\n'
        return "$MOCK_AUDIT_STATUS"
      }
    `;
    for (const status of [0, 1, 42]) {
      const result = spawnSync('bash', ['--noprofile', '--norc', '-c',
        prelude + step.run.replaceAll('${{ matrix.directory }}', '.')], {
        cwd: fixture,
        env: {
          ...process.env,
          ...step.env,
          GITHUB_WORKSPACE: fixture,
          GITHUB_SHA: 'fixture-commit',
          MOCK_LOCKFILES: lockfiles.join('\n'),
          MOCK_AUDIT_STATUS: String(status),
        },
        encoding: 'utf8',
      });
      assert.ifError(result.error);
      assert.equal(result.status, status, result.stdout + result.stderr);
      const evidence = path.join(fixture, 'audit-artifacts/npm');
      const audit = JSON.parse(fs.readFileSync(path.join(evidence, 'audit.json'), 'utf8'));
      const metadata = JSON.parse(fs.readFileSync(path.join(evidence, 'metadata.json'), 'utf8'));
      assert.equal(audit.auditReportVersion, 2);
      assert.equal(metadata.exit_status, status);
      assert.equal(metadata.commit, 'fixture-commit');
      assert.equal(metadata.directory, '.');
    }
  } finally {
    fs.rmSync(fixture, { recursive: true, force: true });
  }
});
