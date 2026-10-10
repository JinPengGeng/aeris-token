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

test('npm audit evidence is JSON and the reviewed-exception gate decides the step result', () => {
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
    // The step resolves the gate through GITHUB_WORKSPACE, so mirror the
    // script (and, for the exempted scenario, the exceptions file) into the
    // fixture workspace just like the real checkout would.
    fs.mkdirSync(path.join(fixture, '.github/scripts'), { recursive: true });
    fs.copyFileSync(
      path.join(repoRoot, '.github/scripts/npm-audit-gate.py'),
      path.join(fixture, '.github/scripts/npm-audit-gate.py'));
    const unreviewed = JSON.stringify({
      auditReportVersion: 2,
      vulnerabilities: {
        leftpad: { severity: 'high', via: [{ url: 'https://github.com/advisories/GHSA-aaaa-bbbb-cccc' }] },
      },
    });
    const exempted = JSON.stringify({
      auditReportVersion: 2,
      vulnerabilities: {
        braces: { severity: 'high', via: [{ url: 'https://github.com/advisories/GHSA-vfj7-8cjw-p6xm' }] },
      },
    });
    const prelude = `
      git() { printf '%s\\n' "$MOCK_LOCKFILES"; }
      npm() {
        if [[ "$1" == --version ]]; then printf '10.9.2\\n'; return 0; fi
        if [[ "$*" != *" --json "* ]]; then
          printf 'found 0 vulnerabilities\\n'
          return 0
        fi
        printf '%s\\n' "$MOCK_AUDIT_JSON"
        return "$MOCK_AUDIT_STATUS"
      }
    `;
    const runStep = (env) => spawnSync('bash', ['--noprofile', '--norc', '-c',
      prelude + step.run.replaceAll('${{ matrix.directory }}', '.')], {
      cwd: fixture,
      env: {
        ...process.env,
        ...step.env,
        GITHUB_WORKSPACE: fixture,
        GITHUB_SHA: 'fixture-commit',
        MOCK_LOCKFILES: lockfiles.join('\n'),
        ...env,
      },
      encoding: 'utf8',
    });

    // Clean report: the gate passes regardless of the raw scanner exit code,
    // and the raw status stays recorded in the evidence metadata.
    for (const status of [0, 1, 42]) {
      const result = runStep({
        MOCK_AUDIT_JSON: '{"auditReportVersion":2,"vulnerabilities":{}}',
        MOCK_AUDIT_STATUS: String(status),
      });
      assert.ifError(result.error);
      assert.equal(result.status, 0, result.stdout + result.stderr);
      const evidence = path.join(fixture, 'audit-artifacts/npm');
      const audit = JSON.parse(fs.readFileSync(path.join(evidence, 'audit.json'), 'utf8'));
      const metadata = JSON.parse(fs.readFileSync(path.join(evidence, 'metadata.json'), 'utf8'));
      assert.equal(audit.auditReportVersion, 2);
      assert.equal(metadata.exit_status, status);
      assert.equal(metadata.gate_status, 0);
      assert.equal(metadata.commit, 'fixture-commit');
      assert.equal(metadata.directory, '.');
    }

    // An unreviewed high advisory fails the gate even when npm itself exits 0.
    const unreviewedResult = runStep({ MOCK_AUDIT_JSON: unreviewed, MOCK_AUDIT_STATUS: '0' });
    assert.equal(unreviewedResult.status, 1, unreviewedResult.stdout + unreviewedResult.stderr);
    assert.match(unreviewedResult.stderr, /leftpad/);

    // A reviewed exception (30-day bound, owner, rationale) covers its advisory.
    fs.mkdirSync(path.join(fixture, '.github/security'), { recursive: true });
    fs.writeFileSync(path.join(fixture, '.github/security/npm-audit-exceptions.json'), JSON.stringify({
      exceptions: [{
        directory: '.',
        package: 'braces',
        ghsa_id: 'GHSA-vfj7-8cjw-p6xm',
        owner: 'fixture owner',
        created_on: '2026-10-10',
        expires_on: '2026-11-09',
        review_cadence: 'weekly',
        rationale: 'no fixed release; not reachable',
      }],
    }));
    const exemptedResult = runStep({ MOCK_AUDIT_JSON: exempted, MOCK_AUDIT_STATUS: '1' });
    assert.ifError(exemptedResult.error);
    assert.equal(exemptedResult.status, 0, exemptedResult.stdout + exemptedResult.stderr);
    const exemptedMetadata = JSON.parse(
      fs.readFileSync(path.join(fixture, 'audit-artifacts/npm/metadata.json'), 'utf8'));
    assert.equal(exemptedMetadata.exit_status, 1);
    assert.equal(exemptedMetadata.gate_status, 0);
    fs.rmSync(path.join(fixture, '.github/security/npm-audit-exceptions.json'));
  } finally {
    fs.rmSync(fixture, { recursive: true, force: true });
  }
});
