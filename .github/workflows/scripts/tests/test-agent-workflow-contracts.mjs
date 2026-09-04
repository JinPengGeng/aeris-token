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

// issue-triage runs the merged two-job shape (#179 Phase 2): prepare assembles
// trigger context and the reservation without any AI secret, analyze holds the
// AI key for exactly one step and then publishes through the same trusted
// phase runner. agent-pr-review keeps the four phase-isolated jobs.
const workflowSpecs = [
  {
    path: '.github/workflows/issue-triage.yml',
    commentPermission: 'issues',
    usesWorkflowRun: false,
    shape: 'merged',
  },
  {
    path: '.github/workflows/agent-pr-review.yml',
    commentPermission: 'pull-requests',
    usesWorkflowRun: true,
    shape: 'phased',
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
const retiredModelVariablePattern =
  /AERIS_AI_MODEL_(TRIAGE|PLANNER|WRITER|SECURITY|FALLBACK|CONFLICT_RESOLVER|CONFLICT_REVIEWER)/;

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

const assertKeyPresenceInspectionOnly = (job, description) => {
  assert.equal(
    directAiSecretPattern.test(serialize(job)),
    false,
    `${description} must not receive the AI API key value`,
  );
  assert.match(
    serialize(job),
    /AERIS_AI_API_KEY_PRESENT[^}]*secrets\.AERIS_AI_API_KEY\s*!=\s*''/,
    `${description} may inspect only whether the AI API key is configured`,
  );
  const withoutPresenceCheck = serialize(job).replaceAll("secrets.AERIS_AI_API_KEY != ''", '');
  assert.equal(
    hasSecretReference(withoutPresenceCheck),
    false,
    `${description} must not reference secrets beyond the key presence check`,
  );
};

const assertReservedAnalysisStep = (analyze, description) => {
  const analyzeSteps = analyze.steps ?? [];
  const secretSteps = analyzeSteps.filter((step) => directAiSecretPattern.test(serialize(step)));
  assert.equal(secretSteps.length, 1, `${description} must have one AI-secret step`);
  assert.match(
    String(secretSteps[0].if ?? ''),
    /read_reservation\.outputs\.state\s*==\s*['"]reserved['"]/,
    `${description} must inject the AI key only for a reserved analysis`,
  );
  assert.equal(
    secretSteps[0].env?.AERIS_AGENTS_ENABLED,
    '${{ vars.AERIS_AGENTS_ENABLED }}',
    `${description} reserved analyze must receive the kill switch`,
  );
  const passthrough = analyzeSteps.find((step) => /Pass through terminal reservation/i.test(step.name ?? ''));
  assert.ok(passthrough, `${description} must pass terminal reservations without AI secrets`);
  assert.equal(hasSecretReference(passthrough), false, `${description} terminal passthrough must not receive secrets`);
  assert.equal(
    passthrough.run,
    'node .github/automation/src/run-phase.mjs analyze',
    `${description} terminal passthrough must use the trusted analyze phase runner`,
  );
};

const assertObjectWriteLock = (job, description) => {
  assert.ok(
    job.concurrency && typeof job.concurrency === 'object',
    `${description} must define an object write lock`,
  );
  assert.equal(
    job.concurrency['cancel-in-progress'],
    false,
    `${description} object write lock must not cancel an active writer`,
  );
  assert.match(
    String(job.concurrency.group),
    /(?:issue|pull_request|workflow_run|inputs)[.\w{} $|'-]*number/i,
    `${description} object write lock must be keyed by the target number`,
  );
};

const assertPhasedShape = (spec, document) => {
  const jobs = document.jobs ?? {};
  for (const phase of ['preflight', 'reserve', 'analyze', 'publish']) {
    assert.ok(jobs[phase], `${spec.path} must define a ${phase} job`);
  }

  assert.equal(jobs.preflight.environment, 'agent', `${spec.path} preflight must load the agent environment`);
  assert.equal(jobs.analyze.environment, 'agent', `${spec.path} analyze must load the agent environment`);

  const preflightPermissions = jobs.preflight.permissions ?? document.permissions;
  assertReadOnlyPermissions(preflightPermissions, `${spec.path} preflight`);
  if (spec.usesWorkflowRun) {
    assert.equal(preflightPermissions.checks, 'read', `${spec.path} preflight must read check runs`);
    assert.equal(preflightPermissions.statuses, 'read', `${spec.path} preflight must read commit statuses`);
  }
  assertPreflightKillSwitchException(jobs.preflight, `${spec.path} preflight`);
  assertPhaseRunner(jobs.preflight, 'preflight', `${spec.path} preflight`);
  assertKeyPresenceInspectionOnly(jobs.preflight, `${spec.path} preflight`);

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
  assertReservedAnalysisStep(jobs.analyze, spec.path);

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
  assertObjectWriteLock(jobs.reserve, `${spec.path} reserve`);
  assert.deepEqual(
    jobs.reserve.concurrency,
    jobs.publish.concurrency,
    `${spec.path} reserve and publish must share the same object write lock`,
  );
  for (const [jobName, dependency] of [['reserve', 'preflight'], ['analyze', 'reserve'], ['publish', 'analyze']]) {
    const condition = String(jobs[jobName].if ?? '');
    assert.match(condition, /always\(\)/, `${spec.path} ${jobName} must propagate terminal artifacts`);
    assert.match(condition, new RegExp(`needs\\.${dependency}\\.result\\s*==\\s*['"]success['"]`));
  }
};

const assertMergedShape = (spec, document) => {
  const jobs = document.jobs ?? {};
  assert.deepEqual(
    Object.keys(jobs).sort(),
    ['analyze', 'prepare'],
    `${spec.path} must define exactly the prepare and analyze jobs`,
  );
  const prepare = jobs.prepare;
  const analyze = jobs.analyze;

  assert.equal(prepare.environment, 'agent', `${spec.path} prepare must load the agent environment`);
  assert.equal(analyze.environment, 'agent', `${spec.path} analyze must load the agent environment`);

  // prepare assembles the trigger context and the reservation. It may write
  // comments and labels but must never see the AI API key value.
  assertCommentWriterPermissions(prepare, spec.commentPermission, `${spec.path} prepare`);
  assertPreflightKillSwitchException(prepare, `${spec.path} prepare`);
  assertKeyPresenceInspectionOnly(prepare, `${spec.path} prepare`);

  const labelStep = (prepare.steps ?? []).find(
    (step) => typeof step.uses === 'string' && step.uses.startsWith('actions/github-script@'),
  );
  assert.ok(labelStep, `${spec.path} prepare must apply the triage status label`);
  assert.match(
    String(labelStep.if ?? ''),
    /github\.event_name\s*==\s*['"]issues['"]\s*&&\s*github\.event\.action\s*==\s*['"]opened['"]/,
    `${spec.path} label step must run only for a newly opened issue`,
  );
  assert.match(
    String(labelStep.with?.script ?? ''),
    /status:triage/,
    `${spec.path} label step must apply the status:triage label`,
  );

  const preflightStep = assertPhaseRunner(prepare, 'preflight', `${spec.path} prepare`);
  const reserveStep = assertPhaseRunner(prepare, 'reserve', `${spec.path} prepare`);
  assert.ok(
    prepare.steps.indexOf(preflightStep) < prepare.steps.indexOf(reserveStep),
    `${spec.path} prepare must run preflight before reserve`,
  );
  assert.equal(
    reserveStep.env?.AERIS_INPUT_PATH,
    preflightStep.env?.AERIS_OUTPUT_PATH,
    `${spec.path} reserve must consume the preflight artifact from the same runner`,
  );

  const uploadStep = (prepare.steps ?? []).find(
    (step) => typeof step.uses === 'string' && step.uses.startsWith('actions/upload-artifact@'),
  );
  assert.ok(uploadStep, `${spec.path} prepare must publish the reservation artifact`);
  assert.match(
    String(uploadStep.with?.name ?? ''),
    /^issue-reservation-/,
    `${spec.path} prepare must upload the reservation artifact`,
  );

  // analyze holds the AI key for exactly one reserved-analysis step and then
  // publishes through the same trusted phase runner without any secret.
  assertCommentWriterPermissions(analyze, spec.commentPermission, `${spec.path} analyze`);
  const needs = Array.isArray(analyze.needs) ? analyze.needs : [analyze.needs];
  assert.deepEqual(needs, ['prepare'], `${spec.path} analyze must wait for prepare`);
  const analyzeCondition = String(analyze.if ?? '');
  assert.match(analyzeCondition, /always\(\)/, `${spec.path} analyze must propagate terminal artifacts`);
  assert.match(
    analyzeCondition,
    /needs\.prepare\.result\s*==\s*['"]success['"]/,
    `${spec.path} analyze must require a successful prepare`,
  );

  const downloadStep = (analyze.steps ?? []).find(
    (step) => typeof step.uses === 'string' && step.uses.startsWith('actions/download-artifact@'),
  );
  assert.ok(downloadStep, `${spec.path} analyze must fetch the reservation artifact`);
  assert.equal(
    downloadStep.with?.name,
    uploadStep.with?.name,
    `${spec.path} analyze must download the exact reservation artifact prepare uploaded`,
  );

  assertReservedAnalysisStep(analyze, spec.path);
  assert.equal(
    hasSecretReference({
      ...analyze,
      env: Object.fromEntries(
        Object.entries(analyze.env ?? {}).filter(([name]) => name !== 'AERIS_AI_API_KEY'),
      ),
      steps: (analyze.steps ?? []).map((step) => ({
        ...step,
        env: Object.fromEntries(
          Object.entries(step.env ?? {}).filter(([name]) => name !== 'AERIS_AI_API_KEY'),
        ),
      })),
    }),
    false,
    `${spec.path} analyze must not receive non-AI secrets`,
  );

  const publishStep = assertPhaseRunner(analyze, 'publish', `${spec.path} analyze`);
  assert.equal(
    hasSecretReference(publishStep),
    false,
    `${spec.path} publish must not receive secrets`,
  );
  assert.equal(
    hasAiKey(publishStep),
    false,
    `${spec.path} publish must not receive the AI API key`,
  );
  const analyzeStep = findPhaseStep(analyze, 'analyze');
  assert.ok(
    analyze.steps.indexOf(analyzeStep) < analyze.steps.indexOf(publishStep),
    `${spec.path} analyze must run analysis before publish`,
  );
  assert.equal(
    publishStep.env?.AERIS_INPUT_PATH,
    analyzeStep.env?.AERIS_OUTPUT_PATH,
    `${spec.path} publish must consume the analysis artifact from the same runner`,
  );

  assertObjectWriteLock(prepare, `${spec.path} prepare`);
  assert.deepEqual(
    analyze.concurrency,
    prepare.concurrency,
    `${spec.path} prepare and analyze must share the same object write lock`,
  );
};

for (const spec of workflowSpecs) {
  test(`${spec.path} keeps execution phases and permissions isolated`, () => {
    const { document } = readWorkflow(spec.path);

    assertReadOnlyPermissions(document.permissions, `${spec.path} top level`);
    assert.doesNotMatch(
      serialize(document.jobs),
      retiredModelVariablePattern,
      `${spec.path} must not read retired per-role or fallback model variables`,
    );
    if (spec.shape === 'merged') {
      assertMergedShape(spec, document);
    } else {
      assertPhasedShape(spec, document);
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
