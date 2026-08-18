import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
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
const actionReferencePattern = /^([A-Za-z0-9][A-Za-z0-9_.-]*\/[A-Za-z0-9][A-Za-z0-9_.-]*)@([0-9a-f]{40})$/;
const dockerDigestPattern = /^docker:\/\/[^@\s]+@sha256:[0-9a-f]{64}$/;
const imageDigestPattern = /^([a-z0-9]+(?:[._\/-][a-z0-9]+)*)@(sha256:[0-9a-f]{64})$/;
const sourceRefPattern = /^[A-Za-z0-9][A-Za-z0-9._/-]*$/;
const workflowUseEntryPattern = /^\s*(?:-\s*)?uses:\s*([^\s#]+)(?:\s+#\s*([^\s#]+))?\s*$/gm;
const workflowImageEntryPattern = /^\s*image:\s*([^\s#]+)(?:\s+#\s*([^\s#]+))?\s*$/gm;

const sourceUseEntries = (source, sourcePath) =>
  [...source.matchAll(workflowUseEntryPattern)].map((match) => ({
    action: match[1],
    ref: match[2] ?? null,
    sourcePath,
  }));

const workflowUseEntries = (workflowFile) => {
  const source = fs.readFileSync(path.join(repoRoot, workflowFile), 'utf8');
  return sourceUseEntries(source, workflowFile);
};

const sourceImageEntries = (source, sourcePath) =>
  [...source.matchAll(workflowImageEntryPattern)].map((match) => ({
    image: match[1],
    ref: match[2] ?? null,
    sourcePath,
  }));

const assertRawWorkflowUseEntriesCoverParsedUses = (parsedUses, rawEntries, description) => {
  const rawActions = rawEntries.map((entry) => entry.action).sort();
  assert.deepEqual(
    rawActions,
    [...parsedUses].sort(),
    `${description} has a uses entry whose source format is not covered by the action lock parser`,
  );
};

const actionLockKey = ({ repository, ref }) => `${repository}\u0000${ref}`;
const imageLockKey = ({ image, ref }) => `${image}\u0000${ref}`;

const assertDockerDigest = (action, description) => {
  assert.match(
    action,
    dockerDigestPattern,
    `${description} container action must use a lowercase sha256 digest`,
  );
};

const assertImageDigest = (image, description) => {
  const match = image.match(imageDigestPattern);
  assert.ok(match, `${description} runtime image must use a lowercase sha256 digest: ${image}`);
  return { image: match[1], digest: match[2] };
};

const collectRuntimeImages = (document, description) => {
  const images = [];
  for (const [jobName, job] of Object.entries(document.jobs ?? {})) {
    assert.ok(
      job && typeof job === 'object' && !Array.isArray(job),
      `${description} job ${jobName} must be a map`,
    );
    if (job.container !== undefined) {
      assert.ok(
        job.container && typeof job.container === 'object' && !Array.isArray(job.container),
        `${description} job ${jobName} container shorthand is unsupported by the image lock`,
      );
      assert.equal(
        typeof job.container.image,
        'string',
        `${description} job ${jobName} container.image must be a string`,
      );
      images.push(job.container.image);
    }
    if (job.services !== undefined) {
      assert.ok(
        job.services && typeof job.services === 'object' && !Array.isArray(job.services),
        `${description} job ${jobName} services must be a map`,
      );
      for (const [serviceName, service] of Object.entries(job.services)) {
        assert.ok(
          service && typeof service === 'object' && !Array.isArray(service),
          `${description} service ${jobName}.${serviceName} must be a map`,
        );
        assert.equal(
          typeof service.image,
          'string',
          `${description} service ${jobName}.${serviceName}.image must be a string`,
        );
        images.push(service.image);
      }
    }
    for (const [stepIndex, step] of (job.steps ?? []).entries()) {
      if (typeof step?.uses !== 'string' || !step.uses.startsWith('docker/setup-qemu-action@')) {
        continue;
      }
      assert.ok(
        step.with && typeof step.with === 'object' && !Array.isArray(step.with),
        `${description} job ${jobName} setup-qemu step ${stepIndex} must define with.image`,
      );
      assert.equal(
        typeof step.with.image,
        'string',
        `${description} job ${jobName} setup-qemu step ${stepIndex} with.image must be a string`,
      );
      images.push(step.with.image);
    }
  }
  return images;
};

const workflowImageEntries = (workflowFile, document) => {
  const source = fs.readFileSync(path.join(repoRoot, workflowFile), 'utf8');
  const parsedImages = collectRuntimeImages(document, workflowFile);
  const rawEntries = sourceImageEntries(source, workflowFile);
  assert.deepEqual(
    rawEntries.map((entry) => entry.image).sort(),
    [...parsedImages].sort(),
    `${workflowFile} has an image whose source format is not covered by the image lock parser`,
  );
  return rawEntries;
};

const isInsidePath = (rootPath, candidatePath) => {
  const relative = path.relative(rootPath, candidatePath);
  return (
    relative !== '' &&
    !path.isAbsolute(relative) &&
    relative !== '..' &&
    !relative.startsWith(`..${path.sep}`)
  );
};

const resolveLocalActionDescriptor = (rootPath, action, description) => {
  assert.ok(action.startsWith('./'), `${description} local action must start with ./`);
  const realRoot = fs.realpathSync(rootPath);
  const candidate = path.resolve(realRoot, action.slice(2));
  assert.ok(isInsidePath(realRoot, candidate), `${description} local action escapes the repository: ${action}`);
  assert.ok(fs.existsSync(candidate), `${description} local action path is missing: ${action}`);
  assert.ok(
    fs.statSync(candidate).isDirectory(),
    `${description} local action path must be a directory: ${action}`,
  );

  const realDirectory = fs.realpathSync(candidate);
  assert.ok(
    isInsidePath(realRoot, realDirectory),
    `${description} local action resolves outside the repository: ${action}`,
  );
  const descriptors = ['action.yml', 'action.yaml']
    .map((name) => path.join(realDirectory, name))
    .filter((descriptor) => fs.existsSync(descriptor));
  assert.equal(
    descriptors.length,
    1,
    `${description} local action must have exactly one action.yml or action.yaml descriptor: ${action}`,
  );
  assert.ok(
    fs.statSync(descriptors[0]).isFile(),
    `${description} local action descriptor must be a file: ${action}`,
  );
  const realDescriptor = fs.realpathSync(descriptors[0]);
  assert.ok(
    isInsidePath(realRoot, realDescriptor),
    `${description} local action descriptor resolves outside the repository: ${action}`,
  );
  return realDescriptor;
};

const createUseAuditor = ({ rootPath, recordExternal }) => {
  const scannedDescriptors = new Set();
  const activeDescriptors = new Set();
  const dockerReferences = new Set();

  const auditEntry = (entry, description = entry.sourcePath) => {
    const { action } = entry;
    if (action.startsWith('./')) {
      const descriptorPath = resolveLocalActionDescriptor(rootPath, action, description);
      assert.equal(
        activeDescriptors.has(descriptorPath),
        false,
        `${description} local action cycle includes ${action}`,
      );
      if (scannedDescriptors.has(descriptorPath)) return;

      activeDescriptors.add(descriptorPath);
      try {
        const source = fs.readFileSync(descriptorPath, 'utf8');
        const document = yaml.load(source);
        assert.ok(
          document && typeof document === 'object' && !Array.isArray(document),
          `${descriptorPath} action descriptor must be a map`,
        );
        assert.ok(
          document.runs && typeof document.runs === 'object' && !Array.isArray(document.runs),
          `${descriptorPath} action descriptor must define runs`,
        );
        assert.equal(
          document.runs.using,
          'composite',
          `${descriptorPath} only composite local actions are supported by the recursive audit`,
        );
        assert.ok(
          Array.isArray(document.runs.steps),
          `${descriptorPath} composite action must define runs.steps`,
        );

        const parsedUses = [];
        for (const [index, step] of document.runs.steps.entries()) {
          assert.ok(
            step && typeof step === 'object' && !Array.isArray(step),
            `${descriptorPath} runs.steps[${index}] must be a map`,
          );
          const hasUses = Object.hasOwn(step, 'uses');
          const hasRun = Object.hasOwn(step, 'run');
          assert.notEqual(
            hasUses,
            hasRun,
            `${descriptorPath} runs.steps[${index}] must define exactly one of uses or run`,
          );
          if (hasUses) {
            assert.equal(
              typeof step.uses,
              'string',
              `${descriptorPath} runs.steps[${index}].uses must be a string`,
            );
            parsedUses.push(step.uses);
          } else {
            assert.equal(
              typeof step.run,
              'string',
              `${descriptorPath} runs.steps[${index}].run must be a string`,
            );
            assert.equal(
              typeof step.shell,
              'string',
              `${descriptorPath} runs.steps[${index}] run step must define shell`,
            );
          }
        }

        const rawEntries = sourceUseEntries(source, descriptorPath);
        assertRawWorkflowUseEntriesCoverParsedUses(parsedUses, rawEntries, descriptorPath);
        for (const nestedEntry of rawEntries) auditEntry(nestedEntry, descriptorPath);
        scannedDescriptors.add(descriptorPath);
      } finally {
        activeDescriptors.delete(descriptorPath);
      }
      return;
    }

    if (action.startsWith('docker://')) {
      assertDockerDigest(action, description);
      dockerReferences.add(action);
      return;
    }

    recordExternal(entry, description);
  };

  return { auditEntry, dockerReferences, scannedDescriptors };
};

const withFixtureRepo = (files, callback) => {
  const fixtureRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-action-audit-'));
  try {
    for (const [relativePath, source] of Object.entries(files)) {
      const target = path.join(fixtureRoot, relativePath);
      fs.mkdirSync(path.dirname(target), { recursive: true });
      fs.writeFileSync(target, source, 'utf8');
    }
    return callback(fixtureRoot);
  } finally {
    fs.rmSync(fixtureRoot, { recursive: true, force: true });
  }
};

const createFixtureUseAuditor = (rootPath) => {
  const externalReferences = [];
  const auditor = createUseAuditor({
    rootPath,
    recordExternal: ({ action, ref }, description) => {
      const match = action.match(actionReferencePattern);
      assert.ok(match, `${description} indirect action is unsupported or mutable: ${action}`);
      assert.match(ref ?? '', sourceRefPattern, `${description} indirect action must retain a source ref`);
      externalReferences.push({ repository: match[1], sha: match[2], ref });
    },
  });
  return { ...auditor, externalReferences };
};

const imageEntriesFromWorkflowSource = (source, description) => {
  const document = yaml.load(source);
  const parsedImages = collectRuntimeImages(document, description);
  const rawEntries = sourceImageEntries(source, description);
  assert.deepEqual(
    rawEntries.map((entry) => entry.image).sort(),
    [...parsedImages].sort(),
    `${description} has an image whose source format is not covered by the image lock parser`,
  );
  return rawEntries;
};

const assertFixtureImageLockCoverage = (lockImages, imageEntries, description) => {
  const lock = new Map(lockImages.map((entry) => [imageLockKey(entry), entry.digest]));
  assert.equal(lock.size, lockImages.length, `${description} fixture image lock contains duplicates`);
  const observed = new Map();
  for (const entry of imageEntries) {
    assert.match(entry.ref ?? '', sourceRefPattern, `${description} image must retain a source ref`);
    const { image, digest } = assertImageDigest(entry.image, description);
    const key = imageLockKey({ image, ref: entry.ref });
    assert.equal(lock.get(key), digest, `${description} image is missing from the fixture lock`);
    observed.set(key, digest);
  }
  assert.deepEqual(
    [...lock.keys()].sort(),
    [...observed.keys()].sort(),
    `${description} fixture image lock contains a stale entry`,
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

test('action and image locks are strict, sorted, and exactly cover workflow supply-chain references', () => {
  assert.deepEqual(Object.keys(actionLock).sort(), ['actions', 'images', 'version']);
  assert.equal(actionLock.version, 1, 'action lock must use version 1');
  assert.ok(Array.isArray(actionLock.actions), 'action lock actions must be an array');
  assert.ok(Array.isArray(actionLock.images), 'action lock images must be an array');

  const lockKeys = new Set();
  const lockEntries = new Map();
  let previousKey = '';
  for (const entry of actionLock.actions) {
    assert.deepEqual(
      Object.keys(entry).sort(),
      ['ref', 'repository', 'sha'],
      'action lock entries must have only repository, ref, and sha',
    );
    assert.match(
      entry.repository,
      /^[A-Za-z0-9][A-Za-z0-9_.-]*\/[A-Za-z0-9][A-Za-z0-9_.-]*$/,
      'action lock repository must be an owner/repository pair',
    );
    assert.match(entry.ref, sourceRefPattern, 'action lock source ref is invalid');
    assert.match(entry.sha, /^[0-9a-f]{40}$/, 'action lock SHA is invalid');
    const key = actionLockKey(entry);
    assert.ok(key > previousKey, 'action lock entries must be strictly sorted by repository and ref');
    assert.equal(lockKeys.has(key), false, `action lock duplicates ${entry.repository}@${entry.ref}`);
    lockKeys.add(key);
    lockEntries.set(key, entry.sha);
    previousKey = key;
  }

  const imageLockKeys = new Set();
  const imageLockEntries = new Map();
  previousKey = '';
  for (const entry of actionLock.images) {
    assert.deepEqual(
      Object.keys(entry).sort(),
      ['digest', 'image', 'ref'],
      'image lock entries must have only image, ref, and digest',
    );
    assert.match(
      entry.image,
      /^[a-z0-9]+(?:[._\/-][a-z0-9]+)*$/,
      'image lock image name is invalid',
    );
    assert.match(entry.ref, sourceRefPattern, 'image lock source ref is invalid');
    assert.match(entry.digest, /^sha256:[0-9a-f]{64}$/, 'image lock digest is invalid');
    const key = imageLockKey(entry);
    assert.ok(key > previousKey, 'image lock entries must be strictly sorted by image and ref');
    assert.equal(imageLockKeys.has(key), false, `image lock duplicates ${entry.image}@${entry.ref}`);
    imageLockKeys.add(key);
    imageLockEntries.set(key, entry.digest);
    previousKey = key;
  }

  const workflowKeys = new Set();
  const workflowRefs = new Map();
  const workflowImageKeys = new Set();
  const workflowImageRefs = new Map();
  const useAuditor = createUseAuditor({
    rootPath: repoRoot,
    recordExternal: ({ action, ref }, description) => {
      const match = action.match(actionReferencePattern);
      assert.ok(match, `${description} action is unsupported or not pinned to a full commit SHA: ${action}`);
      assert.match(ref ?? '', sourceRefPattern, `${description} action must retain a source ref comment`);
      const [, repository, sha] = match;
      const key = actionLockKey({ repository, ref });
      const priorSha = workflowRefs.get(key);
      assert.ok(
        priorSha === undefined || priorSha === sha,
        `${description} maps ${repository}@${ref} to more than one SHA`,
      );
      workflowRefs.set(key, sha);
      workflowKeys.add(key);
      assert.equal(
        lockEntries.get(key),
        sha,
        `${description} action ${repository}@${ref} must exactly match action-lock.yml`,
      );
    },
  });

  assert.ok(workflowFiles.length > 0, 'repository must contain workflow files');
  for (const workflowFile of workflowFiles) {
    const { document } = readWorkflow(workflowFile);
    const parsedUses = collectUses(document.jobs);
    const rawEntries = workflowUseEntries(workflowFile);
    assertRawWorkflowUseEntriesCoverParsedUses(parsedUses, rawEntries, workflowFile);
    for (const entry of rawEntries) useAuditor.auditEntry(entry, workflowFile);

    for (const entry of workflowImageEntries(workflowFile, document)) {
      assert.match(
        entry.ref ?? '',
        sourceRefPattern,
        `${workflowFile} image must retain a source ref comment`,
      );
      const { image, digest } = assertImageDigest(entry.image, workflowFile);
      const key = imageLockKey({ image, ref: entry.ref });
      const priorDigest = workflowImageRefs.get(key);
      assert.ok(
        priorDigest === undefined || priorDigest === digest,
        `${workflowFile} maps ${image}@${entry.ref} to more than one digest`,
      );
      workflowImageRefs.set(key, digest);
      workflowImageKeys.add(key);
      assert.equal(
        imageLockEntries.get(key),
        digest,
        `${workflowFile} image ${image}@${entry.ref} must exactly match action-lock.yml`,
      );
    }
  }
  assert.deepEqual(
    [...lockKeys].sort(),
    [...workflowKeys].sort(),
    'action lock must not contain unused entries and must cover every external workflow action',
  );
  assert.deepEqual(
    [...imageLockKeys].sort(),
    [...workflowImageKeys].sort(),
    'image lock must not contain unused entries and must cover every workflow runtime image',
  );
  assert.ok(useAuditor.dockerReferences instanceof Set, 'docker action references must be explicitly audited');
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

test('action lock parser rejects YAML uses syntax it cannot map to a source ref', () => {
  const source = [
    'jobs:',
    '  check:',
    '    steps:',
    '      - uses : actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09 # v5',
  ].join('\n');
  const parsedUses = collectUses(yaml.load(source).jobs);
  assert.deepEqual(parsedUses, ['actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09']);
  assert.throws(() =>
    assertRawWorkflowUseEntriesCoverParsedUses(
      parsedUses,
      [...source.matchAll(workflowUseEntryPattern)].map((match) => ({ action: match[1] })),
      'synthetic workflow',
    ),
  );
});

test('local composite actions are recursively audited and expose indirect external actions', () => {
  const pinnedSha = 'a'.repeat(40);
  withFixtureRepo(
    {
      '.github/actions/outer/action.yml': [
        'name: outer',
        'description: outer fixture',
        'runs:',
        '  using: composite',
        '  steps:',
        '    - uses: ./.github/actions/inner',
      ].join('\n'),
      '.github/actions/inner/action.yaml': [
        'name: inner',
        'description: inner fixture',
        'runs:',
        '  using: composite',
        '  steps:',
        `    - uses: example/safe-action@${pinnedSha} # v1`,
        '    - run: echo safe',
        '      shell: bash',
      ].join('\n'),
    },
    (fixtureRoot) => {
      const auditor = createFixtureUseAuditor(fixtureRoot);
      auditor.auditEntry({ action: './.github/actions/outer', sourcePath: 'fixture workflow' });
      assert.deepEqual(auditor.externalReferences, [
        { repository: 'example/safe-action', sha: pinnedSha, ref: 'v1' },
      ]);
      assert.equal(auditor.scannedDescriptors.size, 2);
    },
  );
});

test('recursive local action audit rejects mutable indirect and remote reusable workflow references', () => {
  withFixtureRepo(
    {
      '.github/actions/mutable/action.yml': [
        'name: mutable',
        'description: mutable fixture',
        'runs:',
        '  using: composite',
        '  steps:',
        '    - uses: example/unsafe-action@v1 # v1',
      ].join('\n'),
    },
    (fixtureRoot) => {
      const auditor = createFixtureUseAuditor(fixtureRoot);
      assert.throws(
        () => auditor.auditEntry({ action: './.github/actions/mutable', sourcePath: 'fixture workflow' }),
        /unsupported or mutable/,
      );
      assert.throws(
        () =>
          auditor.auditEntry({
            action: `example/repository/.github/workflows/reuse.yml@${'b'.repeat(40)}`,
            ref: 'v1',
            sourcePath: 'fixture workflow',
          }),
        /unsupported or mutable/,
      );
    },
  );
});

test('recursive local action audit rejects cycles, missing or ambiguous descriptors, traversal, and unsupported runs', () => {
  const composite = (uses) =>
    [
      'name: fixture',
      'description: fixture',
      'runs:',
      '  using: composite',
      '  steps:',
      `    - uses: ${uses}`,
    ].join('\n');

  withFixtureRepo(
    {
      '.github/actions/a/action.yml': composite('./.github/actions/b'),
      '.github/actions/b/action.yml': composite('./.github/actions/a'),
      '.github/actions/ambiguous/action.yml': composite('./.github/actions/a'),
      '.github/actions/ambiguous/action.yaml': composite('./.github/actions/a'),
      '.github/actions/unsupported/action.yml': [
        'name: unsupported',
        'description: unsupported fixture',
        'runs:',
        '  using: node20',
        '  main: index.js',
      ].join('\n'),
    },
    (fixtureRoot) => {
      assert.throws(
        () =>
          createFixtureUseAuditor(fixtureRoot).auditEntry({
            action: './.github/actions/a',
            sourcePath: 'fixture workflow',
          }),
        /cycle/,
      );
      assert.throws(
        () =>
          createFixtureUseAuditor(fixtureRoot).auditEntry({
            action: './.github/actions/missing',
            sourcePath: 'fixture workflow',
          }),
        /missing/,
      );
      assert.throws(
        () =>
          createFixtureUseAuditor(fixtureRoot).auditEntry({
            action: './.github/actions/ambiguous',
            sourcePath: 'fixture workflow',
          }),
        /exactly one/,
      );
      assert.throws(
        () =>
          createFixtureUseAuditor(fixtureRoot).auditEntry({
            action: './../outside',
            sourcePath: 'fixture workflow',
          }),
        /escapes the repository/,
      );
      assert.throws(
        () =>
          createFixtureUseAuditor(fixtureRoot).auditEntry({
            action: './.github/actions/unsupported',
            sourcePath: 'fixture workflow',
          }),
        /only composite/,
      );
    },
  );
});

test('docker action audit records digests and rejects mutable tags', () => {
  withFixtureRepo({}, (fixtureRoot) => {
    const auditor = createFixtureUseAuditor(fixtureRoot);
    const immutable = `docker://example.invalid/tool@sha256:${'c'.repeat(64)}`;
    auditor.auditEntry({ action: immutable, sourcePath: 'fixture workflow' });
    assert.deepEqual([...auditor.dockerReferences], [immutable]);
    assert.throws(
      () => auditor.auditEntry({ action: 'docker://example.invalid/tool:latest', sourcePath: 'fixture workflow' }),
      /lowercase sha256 digest/,
    );
  });
});

test('runtime service, container, and setup-qemu images require exact immutable lock entries', () => {
  const postgresDigest = `sha256:${'d'.repeat(64)}`;
  const mysqlDigest = `sha256:${'e'.repeat(64)}`;
  const qemuDigest = `sha256:${'f'.repeat(64)}`;
  const source = [
    'jobs:',
    '  service_job:',
    '    runs-on: ubuntu-latest',
    '    services:',
    '      db:',
    `        image: postgres@${postgresDigest} # 16`,
    '    steps:',
    '      - run: true',
    '  container_job:',
    '    runs-on: ubuntu-latest',
    '    container:',
    `      image: mysql@${mysqlDigest} # 8.0`,
    '    steps:',
    `      - uses: docker/setup-qemu-action@${'a'.repeat(40)}`,
    '        with:',
    `          image: docker.io/tonistiigi/binfmt@${qemuDigest} # latest`,
  ].join('\n');
  const entries = imageEntriesFromWorkflowSource(source, 'image fixture');
  const lockImages = [
    { image: 'docker.io/tonistiigi/binfmt', ref: 'latest', digest: qemuDigest },
    { image: 'mysql', ref: '8.0', digest: mysqlDigest },
    { image: 'postgres', ref: '16', digest: postgresDigest },
  ];
  assert.doesNotThrow(() => assertFixtureImageLockCoverage(lockImages, entries, 'image fixture'));
  assert.throws(
    () => assertFixtureImageLockCoverage(lockImages.slice(1), entries, 'missing lock fixture'),
    /missing from the fixture lock/,
  );
  assert.throws(
    () =>
      assertFixtureImageLockCoverage(
        [...lockImages, { image: 'redis', ref: '7', digest: `sha256:${'1'.repeat(64)}` }],
        entries,
        'stale lock fixture',
      ),
    /stale entry/,
  );
});

test('runtime image audit rejects mutable tags and hidden setup-qemu defaults', () => {
  for (const source of [
    [
      'jobs:',
      '  test:',
      '    runs-on: ubuntu-latest',
      '    services:',
      '      db:',
      '        image: postgres:16 # 16',
    ].join('\n'),
    [
      'jobs:',
      '  test:',
      '    runs-on: ubuntu-latest',
      '    container:',
      '      image: mysql:8.0 # 8.0',
    ].join('\n'),
  ]) {
    const entries = imageEntriesFromWorkflowSource(source, 'mutable image fixture');
    assert.throws(() => assertImageDigest(entries[0].image, 'mutable image fixture'), /sha256 digest/);
  }

  const hiddenQemuDefault = [
    'jobs:',
    '  test:',
    '    runs-on: ubuntu-latest',
    '    steps:',
    `      - uses: docker/setup-qemu-action@${'a'.repeat(40)}`,
  ].join('\n');
  assert.throws(
    () => imageEntriesFromWorkflowSource(hiddenQemuDefault, 'setup-qemu fixture'),
    /must define with.image/,
  );
});
