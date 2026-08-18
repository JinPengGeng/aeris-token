import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const testDirectory = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(testDirectory, '..', '..', '..', '..');
const yamlCandidates = [
  path.join(repoRoot, '.github', 'automation', 'node_modules', 'js-yaml'),
  path.join(repoRoot, 'frontend', 'node_modules', 'js-yaml'),
];
const yamlPath = yamlCandidates.find((candidate) => fs.existsSync(candidate));

assert.ok(yamlPath, 'js-yaml is not installed in an approved workspace');
const yaml = require(yamlPath);

const workflowSpecs = [
  {
    path: '.github/workflows/issue-triage.yml',
    commentPermission: 'issues',
    usesWorkflowRun: false,
  },
  {
    path: '.github/workflows/agent-pr-review.yml',
    commentPermission: 'pull-requests',
    usesWorkflowRun: true,
  },
];

const readWorkflow = (relativePath) => {
  const source = fs.readFileSync(path.join(repoRoot, relativePath), 'utf8');
  return { source, document: yaml.load(source) };
};

const serialize = (value) => JSON.stringify(value);
const hasAiKey = (value) => /AERIS_AI_API_KEY/i.test(serialize(value));
const hasSecretReference = (value) => /\$\{\{\s*secrets\./i.test(serialize(value));
const directAiSecretPattern = /\$\{\{\s*secrets\.AERIS_AI_API_KEY\s*\}\}/;

const assertReadOnlyPermissions = (permissions, description) => {
  assert.equal(typeof permissions, 'object', `${description} permissions must be explicit`);
  assert.ok(permissions !== null && !Array.isArray(permissions), `${description} permissions must be a map`);
  assert.equal(permissions.contents, 'read', `${description} must retain contents: read`);
  for (const [scope, access] of Object.entries(permissions)) {
    assert.notEqual(access, 'write', `${description} must not grant ${scope}: write`);
    assert.ok(
      access === 'read' || access === 'none',
      `${description} has unsupported ${scope} permission: ${access}`,
    );
  }
};

const assertCommentWriterPermissions = (job, expectedScope, description) => {
  const permissions = job.permissions;
  assert.equal(typeof permissions, 'object', `${description} permissions must be explicit`);
  assert.ok(permissions !== null && !Array.isArray(permissions), `${description} permissions must be a map`);
  assert.equal(
    permissions[expectedScope],
    'write',
    `${description} must grant ${expectedScope}: write`,
  );
  for (const [scope, access] of Object.entries(permissions)) {
    if (access === 'write') {
      assert.equal(scope, expectedScope, `${description} grants unexpected ${scope}: write`);
    } else {
      assert.ok(
        access === 'read' || access === 'none',
        `${description} has unsupported ${scope} permission: ${access}`,
      );
    }
  }
};

const collectUses = (value, results = []) => {
  if (Array.isArray(value)) {
    for (const item of value) collectUses(item, results);
    return results;
  }
  if (!value || typeof value !== 'object') return results;

  for (const [key, child] of Object.entries(value)) {
    if (key === 'uses' && typeof child === 'string') results.push(child);
    collectUses(child, results);
  }
  return results;
};

const workflowFiles = fs
  .readdirSync(path.join(repoRoot, '.github', 'workflows'), { withFileTypes: true })
  .filter((entry) => entry.isFile() && /\.ya?ml$/i.test(entry.name))
  .map((entry) => path.join('.github', 'workflows', entry.name));

const actionLock = yaml.load(
  fs.readFileSync(path.join(repoRoot, '.github', 'action-lock.yml'), 'utf8'),
);
const actionReferencePattern = /^([^@\s]+)@([0-9a-f]{40})$/;
const dockerDigestPattern = /^docker:\/\/[^@\s]+@sha256:[0-9a-f]{64}$/;
const sourceRefPattern = /^[A-Za-z0-9][A-Za-z0-9._/-]*$/;
const workflowUseEntryPattern = /^\s*(?:-\s*)?uses:\s*([^\s#]+)(?:\s+#\s*([^\s#]+))?\s*$/gm;

const workflowUseEntries = (workflowFile) => {
  const source = fs.readFileSync(path.join(repoRoot, workflowFile), 'utf8');
  return [...source.matchAll(workflowUseEntryPattern)].map((match) => ({
    action: match[1],
    ref: match[2] ?? null,
    workflowFile,
  }));
};

const actionLockKey = ({ repository, ref }) => `${repository}\u0000${ref}`;

const assertDockerDigest = (action, description) => {
  assert.match(
    action,
    dockerDigestPattern,
    `${description} container action must use a lowercase sha256 digest`,
  );
};

const collectCheckoutSteps = (document) =>
  Object.values(document.jobs ?? {}).flatMap((job) =>
    (job.steps ?? []).filter(
      (step) => typeof step.uses === 'string' && step.uses.startsWith('actions/checkout@'),
    ),
  );

const findPhaseStep = (job, phase) =>
  (job.steps ?? []).find((step) => step.env?.AERIS_PHASE === phase);

const assertPhaseRunner = (job, phase, description) => {
  const step = findPhaseStep(job, phase);
  assert.ok(step, `${description} must define a ${phase} phase step`);
  assert.equal(
    step.run,
    `node .github/automation/src/run-phase.mjs ${phase}`,
    `${description} must enter the trusted run-phase ${phase} runner`,
  );
  return step;
};

const assertPreflightKillSwitchException = (job, description) => {
  const condition = String(job.if ?? '');
  assert.match(
    condition,
    /vars\.AERIS_AGENTS_ENABLED\s*==\s*['"]true['"]\s*\|\|\s*vars\.AERIS_AGENTS_ENABLED\s*==\s*['"]1['"]/, 
    `${description} must gate ordinary invocations with the agent kill switch`,
  );
  assert.match(
    condition,
    /github\.event_name\s*==\s*['"]issue_comment['"][\s\S]*github\.event\.comment\.body\s*==\s*['"]\/agent status['"][\s\S]*github\.event\.comment\.body\s*==\s*['"]\/agent cancel['"]/, 
    `${description} must allow /agent status and /agent cancel through preflight when disabled`,
  );
};

for (const spec of workflowSpecs) {
  test(`${spec.path} keeps execution phases and permissions isolated`, () => {
    const { document } = readWorkflow(spec.path);

    assertReadOnlyPermissions(document.permissions, `${spec.path} top level`);

    const jobs = document.jobs ?? {};
    for (const phase of ['preflight', 'reserve', 'analyze', 'publish']) {
      assert.ok(jobs[phase], `${spec.path} must define a ${phase} job`);
    }

    const preflightPermissions = jobs.preflight.permissions ?? document.permissions;
    assertReadOnlyPermissions(preflightPermissions, `${spec.path} preflight`);
    if (spec.usesWorkflowRun) {
      assert.equal(preflightPermissions.checks, 'read', `${spec.path} preflight must read check runs`);
      assert.equal(preflightPermissions.statuses, 'read', `${spec.path} preflight must read commit statuses`);
    }
    assertPreflightKillSwitchException(jobs.preflight, `${spec.path} preflight`);
    assertPhaseRunner(jobs.preflight, 'preflight', `${spec.path} preflight`);
    assert.equal(
      directAiSecretPattern.test(serialize(jobs.preflight)),
      false,
      `${spec.path} preflight must not receive the AI API key value`,
    );
    assert.match(
      serialize(jobs.preflight),
      /AERIS_AI_API_KEY_PRESENT[^}]*secrets\.AERIS_AI_API_KEY\s*!=\s*''/,
      `${spec.path} preflight may inspect only whether the AI API key is configured`,
    );

    const analyzePermissions = jobs.analyze.permissions ?? document.permissions;
    assertReadOnlyPermissions(analyzePermissions, `${spec.path} analyze`);
    if (spec.usesWorkflowRun) {
      assert.equal(analyzePermissions.checks, 'read', `${spec.path} analyze must recheck check runs`);
      assert.equal(analyzePermissions.statuses, 'read', `${spec.path} analyze must recheck commit statuses`);
    }
    assert.notEqual(
      analyzePermissions.issues,
      'write',
      `${spec.path} analyze must not grant issues: write`,
    );
    assert.notEqual(
      analyzePermissions['pull-requests'],
      'write',
      `${spec.path} analyze must not grant pull-requests: write`,
    );
    assert.ok(hasAiKey(jobs.analyze), `${spec.path} analyze must receive the AI API key`);
    assert.match(
      serialize(jobs.analyze),
      directAiSecretPattern,
      `${spec.path} analyze must source the AI API key from its secret`,
    );
    assert.equal(
      hasSecretReference({
        ...jobs.analyze,
        env: Object.fromEntries(
          Object.entries(jobs.analyze.env ?? {}).filter(([name]) => name !== 'AERIS_AI_API_KEY'),
        ),
        steps: (jobs.analyze.steps ?? []).map((step) => ({
          ...step,
          env: Object.fromEntries(
            Object.entries(step.env ?? {}).filter(([name]) => name !== 'AERIS_AI_API_KEY'),
          ),
        })),
      }),
      false,
      `${spec.path} analyze must not receive non-AI secrets`,
    );
    const analyzeSteps = jobs.analyze.steps ?? [];
    const secretSteps = analyzeSteps.filter((step) => directAiSecretPattern.test(serialize(step)));
    assert.equal(secretSteps.length, 1, `${spec.path} analyze must have one AI-secret step`);
    assert.match(
      String(secretSteps[0].if ?? ''),
      /read_reservation\.outputs\.state\s*==\s*['"]reserved['"]/,
      `${spec.path} must inject the AI key only for a reserved analysis`,
    );
    assert.equal(
      secretSteps[0].env?.AERIS_AGENTS_ENABLED,
      '${{ vars.AERIS_AGENTS_ENABLED }}',
      `${spec.path} reserved analyze must receive the kill switch`,
    );
    const passthrough = analyzeSteps.find((step) => /Pass through terminal reservation/i.test(step.name ?? ''));
    assert.ok(passthrough, `${spec.path} must pass terminal reservations without AI secrets`);
    assert.equal(hasSecretReference(passthrough), false, `${spec.path} terminal passthrough must not receive secrets`);
    assert.equal(
      passthrough.run,
      'node .github/automation/src/run-phase.mjs analyze',
      `${spec.path} terminal passthrough must use the trusted analyze phase runner`,
    );

    for (const [jobName, job] of Object.entries(jobs)) {
      if (jobName !== 'analyze' && jobName !== 'preflight') {
        assert.equal(
          hasAiKey(job),
          false,
          `${spec.path} ${jobName} must not receive the AI API key`,
        );
        assert.equal(
          hasSecretReference(job),
          false,
          `${spec.path} ${jobName} must not reference secrets`,
        );
      }
    }

    for (const phase of ['reserve', 'publish']) {
      assertPhaseRunner(jobs[phase], phase, `${spec.path} ${phase}`);
      assertCommentWriterPermissions(
        jobs[phase],
        spec.commentPermission,
        `${spec.path} ${phase}`,
      );
      assert.equal(
        hasAiKey(jobs[phase]),
        false,
        `${spec.path} ${phase} must not receive the AI API key`,
      );
    }
    if (spec.usesWorkflowRun) {
      assert.equal(jobs.publish.permissions.checks, 'read', `${spec.path} publish must recheck check runs`);
      assert.equal(jobs.publish.permissions.statuses, 'read', `${spec.path} publish must recheck commit statuses`);
    }
    assert.ok(
      jobs.reserve.concurrency && typeof jobs.reserve.concurrency === 'object',
      `${spec.path} reserve must define an object write lock`,
    );
    assert.deepEqual(
      jobs.reserve.concurrency,
      jobs.publish.concurrency,
      `${spec.path} reserve and publish must share the same object write lock`,
    );
    assert.equal(
      jobs.reserve.concurrency['cancel-in-progress'],
      false,
      `${spec.path} object write lock must not cancel an active writer`,
    );
    assert.match(
      String(jobs.reserve.concurrency.group),
      /(?:issue|pull_request|workflow_run|inputs)[.\w{} $|'-]*number/i,
      `${spec.path} object write lock must be keyed by the target number`,
    );
    for (const [jobName, dependency] of [['reserve', 'preflight'], ['analyze', 'reserve'], ['publish', 'analyze']]) {
      const condition = String(jobs[jobName].if ?? '');
      assert.match(condition, /always\(\)/, `${spec.path} ${jobName} must propagate terminal artifacts`);
      assert.match(condition, new RegExp(`needs\\.${dependency}\\.result\\s*==\\s*['"]success['"]`));
    }
  });

  test(`${spec.path} checks out only trusted automation and pins actions`, () => {
    const { document } = readWorkflow(spec.path);
    const checkoutSteps = collectCheckoutSteps(document);

    assert.ok(checkoutSteps.length > 0, `${spec.path} must check out trusted automation`);
    for (const step of checkoutSteps) {
      assert.equal(
        step.with?.ref,
        '${{ github.event.repository.default_branch }}',
        `${spec.path} checkout must use the trusted default branch`,
      );
      assert.doesNotMatch(
        serialize(step.with ?? {}),
        /(?:pull_request|workflow_run)\.(?:head|head_sha)/i,
        `${spec.path} checkout must not use a pull request head`,
      );
    }

    const uses = collectUses(document.jobs);
    assert.ok(uses.length > 0, `${spec.path} must contain at least one action`);
    for (const action of uses) {
      if (action.startsWith('./') || action.startsWith('docker://')) continue;
      assert.match(
        action,
        /^[^@\s]+@[0-9a-f]{40}$/,
        `${spec.path} action is not pinned to a full commit SHA: ${action}`,
      );
    }
  });

  test(`${spec.path} keeps workflow-level execution serialization safe`, () => {
    const { document } = readWorkflow(spec.path);
    if (spec.usesWorkflowRun) {
      assert.ok(
        document.concurrency && typeof document.concurrency === 'object',
        `${spec.path} must serialize complete workflow runs`,
      );
      assert.equal(
        document.concurrency['cancel-in-progress'],
        false,
        `${spec.path} workflow-level serialization must not cancel an active run`,
      );
      assert.match(
        String(document.concurrency.group),
        /github\.event_name\s*==\s*['"]workflow_run['"]/,
        `${spec.path} workflow-level serialization must apply only to workflow_run events`,
      );
      assert.match(
        String(document.concurrency.group),
        /workflow_run[.\w{} $|\[\]'"=-]*pull_requests\[0\]\.number/i,
        `${spec.path} workflow-level serialization must be keyed by the workflow_run PR number`,
      );
      assert.match(
        String(document.concurrency.group),
        /workflow_run\.head_sha/i,
        `${spec.path} workflow-level serialization must be keyed by the workflow_run head SHA`,
      );
      assert.match(
        String(document.concurrency.group),
        /\|\|\s*github\.run_id/,
        `${spec.path} non-workflow_run events must fall back to independent run IDs`,
      );
      assert.notEqual(
        document.concurrency.group,
        document.jobs.reserve.concurrency.group,
        `${spec.path} workflow-level serialization must not reuse the job write-lock group`,
      );
    } else if (document.concurrency && typeof document.concurrency === 'object') {
      assert.equal(
        document.concurrency['cancel-in-progress'],
        false,
        `${spec.path} workflow-level concurrency must not cancel an active run`,
      );
    }
  });

  if (spec.usesWorkflowRun) {
    test(`${spec.path} accepts all completed workflow runs with a pull request`, () => {
      const { document } = readWorkflow(spec.path);
      const workflowRun = document.on?.workflow_run;
      assert.ok(workflowRun, `${spec.path} must retain its workflow_run trigger`);
      assert.ok(
        workflowRun.types?.includes('completed'),
        `${spec.path} workflow_run trigger must use the completed type`,
      );

      const preflightCondition = String(document.jobs?.preflight?.if ?? '');
      assert.doesNotMatch(
        preflightCondition,
        /github\.event\.workflow_run\.conclusion/,
        `${spec.path} preflight must not trust the triggering workflow conclusion`,
      );
      assert.match(
        preflightCondition,
        /workflow_run[.\w{} $|\[\]'"=-]*pull_requests\[0\]\.number/i,
        `${spec.path} preflight must require a workflow_run pull request association`,
      );
    });
  }
}

test('action lock is strict, sorted, and exactly covers external workflow actions', () => {
  assert.deepEqual(Object.keys(actionLock).sort(), ['actions', 'version']);
  assert.equal(actionLock.version, 1, 'action lock must use version 1');
  assert.ok(Array.isArray(actionLock.actions), 'action lock actions must be an array');

  const lockKeys = new Set();
  const lockEntries = new Map();
  let previousKey = '';
  for (const entry of actionLock.actions) {
    assert.deepEqual(
      Object.keys(entry).sort(),
      ['ref', 'repository', 'sha'],
      'action lock entries must have only repository, ref, and sha',
    );
    assert.match(entry.repository, /^[^@\s]+\/[^^@\s]+$/, 'action lock repository is invalid');
    assert.match(entry.ref, sourceRefPattern, 'action lock source ref is invalid');
    assert.match(entry.sha, /^[0-9a-f]{40}$/, 'action lock SHA is invalid');
    const key = actionLockKey(entry);
    assert.ok(key > previousKey, 'action lock entries must be strictly sorted by repository and ref');
    assert.equal(lockKeys.has(key), false, `action lock duplicates ${entry.repository}@${entry.ref}`);
    lockKeys.add(key);
    lockEntries.set(key, entry.sha);
    previousKey = key;
  }

  const workflowKeys = new Set();
  const workflowRefs = new Map();
  assert.ok(workflowFiles.length > 0, 'repository must contain workflow files');
  for (const workflowFile of workflowFiles) {
    for (const { action, ref } of workflowUseEntries(workflowFile)) {
      if (action.startsWith('./')) continue;
      if (action.startsWith('docker://')) {
        assertDockerDigest(action, `${workflowFile}`);
        continue;
      }

      const match = action.match(actionReferencePattern);
      assert.ok(match, `${workflowFile} action is not pinned to a full commit SHA: ${action}`);
      assert.match(ref ?? '', sourceRefPattern, `${workflowFile} action must retain a source ref comment`);
      const [, repository, sha] = match;
      const key = actionLockKey({ repository, ref });
      const priorSha = workflowRefs.get(key);
      assert.ok(
        priorSha === undefined || priorSha === sha,
        `${workflowFile} maps ${repository}@${ref} to more than one SHA`,
      );
      workflowRefs.set(key, sha);
      workflowKeys.add(key);
      assert.equal(
        lockEntries.get(key),
        sha,
        `${workflowFile} action ${repository}@${ref} must exactly match action-lock.yml`,
      );
    }
  }
  assert.deepEqual(
    [...lockKeys].sort(),
    [...workflowKeys].sort(),
    'action lock must not contain unused entries and must cover every external workflow action',
  );
});

test('container workflow actions require immutable lowercase sha256 digests', () => {
  assert.doesNotThrow(() =>
    assertDockerDigest(
      `docker://example.invalid/tool@sha256:${'a'.repeat(64)}`,
      'valid container action',
    ),
  );
  for (const action of [
    'docker://example.invalid/tool:latest',
    'docker://example.invalid/tool',
    `docker://example.invalid/tool@sha256:${'A'.repeat(64)}`,
    `docker://example.invalid/tool@sha256:${'a'.repeat(63)}`,
  ]) {
    assert.throws(() => assertDockerDigest(action, 'invalid container action'));
  }
});
