import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const yaml = require('js-yaml');
const testDirectory = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(testDirectory, '..', '..', '..');
const workflowPath = path.join(repoRoot, '.github', 'workflows', 'writer-readonly-attestation.yml');
const bashCandidates = process.platform === 'win32'
  ? [
      process.env.AERIS_TEST_BASH,
      'C:/Program Files/Git/bin/bash.exe',
      'C:/Program Files (x86)/Git/bin/bash.exe',
      'D:/Program Files/Git/bin/bash.exe',
    ]
  : ['bash'];
const gitBash = bashCandidates.find((candidate) => candidate && (candidate === 'bash' || fs.existsSync(candidate)));

function workflow() {
  return yaml.load(fs.readFileSync(workflowPath, 'utf8'));
}

test('Writer attestation is a default-branch-only manual workflow without inputs', () => {
  const document = workflow();
  assert.deepEqual(document.on.workflow_dispatch, null);
  assert.deepEqual(Object.keys(document.on), ['workflow_dispatch']);
  assert.match(document.jobs.attest.if, /github\.ref == format\('refs\/heads\/\{0\}', github\.event\.repository\.default_branch\)/);
  assert.equal(document.jobs.attest.environment, 'writer');
  assert.equal(document.permissions.contents, 'read');
  assert.deepEqual(document.jobs.attest.permissions, { contents: 'read' });
  assert.deepEqual(Object.entries(document.jobs.attest.permissions).filter(([, value]) => value === 'write'), []);
});

test('Writer attestation mints one explicitly read-only repository token and has no mutation command', () => {
  const job = workflow().jobs.attest;
  const tokenSteps = job.steps.filter((step) => /create-github-app-token@/.test(step.uses ?? ''));
  assert.equal(tokenSteps.length, 1);
  const permissions = Object.fromEntries(Object.entries(tokenSteps[0].with).filter(([key]) => key.startsWith('permission-')));
  assert.deepEqual(permissions, {
    'permission-administration': 'read',
    'permission-contents': 'read',
    'permission-pull-requests': 'read',
  });
  assert.equal(tokenSteps[0].with.repositories, '${{ github.event.repository.name }}');
  const serialized = JSON.stringify(job);
  assert.match(serialized, /github-app-attestation\.mjs prove-token/);
  assert.doesNotMatch(serialized, /\bgh\s+(pr|issue|api)|git\s+(push|commit)|mergePullRequest|markPullRequestReady|convertPullRequestToDraft/);
  const summary = job.steps.find((step) => /Summarize read-only attestation/.test(step.name));
  assert.equal(summary.env.APP_PERMISSIONS, '${{ steps.writer_app_attestation.outputs.app_permissions }}');
  assert.equal(summary.env.INSTALLATION_PERMISSIONS, '${{ steps.writer_app_attestation.outputs.installation_permissions }}');
  assert.equal(summary.env.REPOSITORY_SELECTION, '${{ steps.writer_app_attestation.outputs.repository_selection }}');
  assert.match(summary.run, /Bot GraphQL identity/);
  const printfNewline = String.raw`\n`;
  assert.equal(printfNewline, '\\n');
  assert.notEqual(printfNewline, '\n');
  const expectedFormats = [
    [`'- App: \`%s\` (#%s)${printfNewline}'`, ['APP_SLUG', 'APP_ID']],
    [`'- App owner: \`%s\` (\`%s\`)${printfNewline}'`, ['APP_OWNER', 'APP_OWNER_TYPE']],
    [`'- App permissions: \`%s\`${printfNewline}'`, ['APP_PERMISSIONS']],
    [`'- Installation: \`%s\`${printfNewline}'`, ['INSTALLATION_ID']],
    [`'- Installation permissions: \`%s\`${printfNewline}'`, ['INSTALLATION_PERMISSIONS']],
    [`'- Repository selection: \`%s\`${printfNewline}'`, ['REPOSITORY_SELECTION']],
    [`'- Repository scope: \`%s\`${printfNewline}'`, ['REPOSITORY']],
    [`'- Bot REST identity: \`%s\` (#%s)${printfNewline}'`, ['BOT_REST_LOGIN', 'BOT_DATABASE_ID']],
    [`'- Bot GraphQL identity: \`%s\` (\`%s\`)${printfNewline}'`, ['BOT_GRAPHQL_LOGIN', 'BOT_NODE_ID']],
  ];
  for (const [format, variables] of expectedFormats) {
    assert.ok(summary.run.includes(`printf -- ${format}`), `missing printf format: ${format}`);
    for (const variable of variables) {
      assert.match(summary.run, new RegExp(`\\"\\$\\{${variable}\\}\\"`));
    }
  }
  assert.doesNotMatch(summary.run, /printf '%s\\n'/);
  assert.doesNotMatch(summary.run, /`\$\{APP_[A-Z_]+\}`/);
  assert.doesNotMatch(JSON.stringify(summary), /TOKEN|PRIVATE_KEY/);
  for (const step of job.steps) {
    if (step.uses) assert.match(step.uses, /^[^@\s]+@[0-9a-f]{40}$/);
  }
});

test('Writer attestation summary renders shell metacharacters literally', { skip: !gitBash }, () => {
  const summary = workflow().jobs.attest.steps.find((step) => /Summarize read-only attestation/.test(step.name));
  const summaryPath = path.join(testDirectory, `.writer-attestation-summary-${process.pid}-${Date.now()}.md`);
  const summaryPathForBash = summaryPath
    .replace(/^([A-Za-z]):[\\/](.*)$/, (_, drive, rest) => `/${drive.toLowerCase()}/${rest}`)
    .replaceAll('\\', '/');
  const values = {
    APP_ID: '4667256',
    APP_SLUG: 'writer`$(printf injected)`%percent',
    APP_OWNER: 'owner`$(printf injected)`',
    APP_OWNER_TYPE: 'Organization%q',
    APP_PERMISSIONS: 'contents:write`$(printf injected)`',
    INSTALLATION_ID: '155342531`$(printf injected)`',
    INSTALLATION_PERMISSIONS: 'pull_requests:write%q',
    REPOSITORY_SELECTION: 'selected`$(printf injected)`',
    REPOSITORY: 'JinPengGeng/aeris-token`$(printf injected)`',
    BOT_REST_LOGIN: 'aeris-token-writer[bot]`$(printf injected)`',
    BOT_DATABASE_ID: '12345%q',
    BOT_GRAPHQL_LOGIN: 'aeris-token-writer[bot]`$(printf injected)`',
    BOT_NODE_ID: 'MDQ6Qm90MTIz`$(printf injected)`',
  };
  try {
    const result = spawnSync(
      gitBash,
      ['-e', '-u', '-o', 'pipefail', '-c', summary.run],
      {
        encoding: 'utf8',
        env: { ...process.env, ...values, GITHUB_STEP_SUMMARY: summaryPathForBash },
      },
    );
    assert.equal(result.error, undefined, result.error?.message);
    assert.equal(result.status, 0, result.stderr);
    const rendered = fs.readFileSync(summaryPath, 'utf8');
    const expectedLines = [
      '### Writer App read-only attestation',
      '',
      `- App: \`${values.APP_SLUG}\` (#${values.APP_ID})`,
      `- App owner: \`${values.APP_OWNER}\` (\`${values.APP_OWNER_TYPE}\`)`,
      `- App permissions: \`${values.APP_PERMISSIONS}\``,
      `- Installation: \`${values.INSTALLATION_ID}\``,
      `- Installation permissions: \`${values.INSTALLATION_PERMISSIONS}\``,
      `- Repository selection: \`${values.REPOSITORY_SELECTION}\``,
      `- Repository scope: \`${values.REPOSITORY}\``,
      `- Bot REST identity: \`${values.BOT_REST_LOGIN}\` (#${values.BOT_DATABASE_ID})`,
      `- Bot GraphQL identity: \`${values.BOT_GRAPHQL_LOGIN}\` (\`${values.BOT_NODE_ID}\`)`,
    ];
    assert.equal(rendered, `${expectedLines.join('\n')}\n`);
    assert.doesNotMatch(rendered, /%s/);
  } finally {
    fs.rmSync(summaryPath, { force: true });
  }
});
