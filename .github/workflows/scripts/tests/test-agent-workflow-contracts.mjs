import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';
import test from 'node:test';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const execFileAsync = promisify(execFile);
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
const imageDigestPattern = /^([a-z0-9]+(?:[._\/-][a-z0-9]+)*)@(sha256:[0-9a-f]{64})$/;
const MAX_ACTION_ENTRIES = 64;
const MAX_LOCAL_ACTION_NODES = 64;
const MAX_LOCAL_ACTION_DEPTH = 8;
const MAX_REMOTE_ACTION_NODES = 64;
const MAX_REMOTE_ACTION_DEPTH = 8;
const MAX_DESCRIPTOR_BYTES = 256 * 1024;
const MAX_FETCH_CONCURRENCY = 4;
const REMOTE_FETCH_TIMEOUT_MS = 60_000;
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

const actionLockKey = ({ repository, ref }) => `${repository.toLowerCase()}\u0000${ref}`;
const imageLockKey = ({ image, ref }) => `${image}\u0000${ref}`;

const assertImageDigest = (image, description) => {
  const match = image.match(imageDigestPattern);
  assert.ok(match, `${description} runtime image must use a lowercase sha256 digest: ${image}`);
  return { image: match[1], digest: match[2] };
};

const collectStepRuntimeImages = (steps, description) => {
  const images = [];
  for (const [stepIndex, step] of steps.entries()) {
    if (
      typeof step?.uses !== 'string' ||
      !step.uses.toLowerCase().startsWith('docker/setup-qemu-action@')
    ) {
      continue;
    }
    assert.ok(
      step.with && typeof step.with === 'object' && !Array.isArray(step.with),
      `${description} setup-qemu step ${stepIndex} must define with.image`,
    );
    assert.equal(
      typeof step.with.image,
      'string',
      `${description} setup-qemu step ${stepIndex} with.image must be a string`,
    );
    images.push(step.with.image);
  }
  return images;
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
    images.push(...collectStepRuntimeImages(job.steps ?? [], `${description} job ${jobName}`));
  }
  return images;
};

const assertRawImageEntriesCoverParsedImages = (parsedImages, rawEntries, description) => {
  assert.deepEqual(
    rawEntries.map((entry) => entry.image).sort(),
    [...parsedImages].sort(),
    `${description} has an image whose source format is not covered by the image lock parser`,
  );
};

const workflowImageEntries = (workflowFile, document) => {
  const source = fs.readFileSync(path.join(repoRoot, workflowFile), 'utf8');
  const parsedImages = collectRuntimeImages(document, workflowFile);
  const rawEntries = sourceImageEntries(source, workflowFile);
  assertRawImageEntriesCoverParsedImages(parsedImages, rawEntries, workflowFile);
  return rawEntries;
};

const validateCompositeSteps = (steps, description) => {
  assert.ok(Array.isArray(steps), `${description} composite action must define runs.steps`);
  const parsedUses = [];
  for (const [index, step] of steps.entries()) {
    assert.ok(
      step && typeof step === 'object' && !Array.isArray(step),
      `${description} runs.steps[${index}] must be a map`,
    );
    const hasUses = Object.hasOwn(step, 'uses');
    const hasRun = Object.hasOwn(step, 'run');
    assert.notEqual(
      hasUses,
      hasRun,
      `${description} runs.steps[${index}] must define exactly one of uses or run`,
    );
    if (hasUses) {
      assert.equal(
        typeof step.uses,
        'string',
        `${description} runs.steps[${index}].uses must be a string`,
      );
      parsedUses.push(step.uses);
    } else {
      assert.equal(
        typeof step.run,
        'string',
        `${description} runs.steps[${index}].run must be a string`,
      );
      assert.equal(
        typeof step.shell,
        'string',
        `${description} runs.steps[${index}] run step must define shell`,
      );
    }
  }
  return parsedUses;
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
  assert.ok(
    fs.statSync(descriptors[0]).size <= MAX_DESCRIPTOR_BYTES,
    `${description} local action descriptor exceeds ${MAX_DESCRIPTOR_BYTES} bytes: ${action}`,
  );
  const realDescriptor = fs.realpathSync(descriptors[0]);
  assert.ok(
    isInsidePath(realRoot, realDescriptor),
    `${description} local action descriptor resolves outside the repository: ${action}`,
  );
  return realDescriptor;
};

const auditNonLocalUseEntry = (entry, description, recordExternal, recordImage) => {
  const { action } = entry;
  if (action.startsWith('docker://')) {
    assert.match(
      entry.ref ?? '',
      sourceRefPattern,
      `${description} container action must retain a source ref comment`,
    );
    const imageReference = action.slice('docker://'.length);
    assertImageDigest(imageReference, `${description} container action`);
    recordImage({ image: imageReference, ref: entry.ref, sourcePath: entry.sourcePath }, description);
    return;
  }
  recordExternal(entry, description);
};

const createUseAuditor = ({
  rootPath,
  recordExternal,
  recordImage,
  maxLocalNodes = MAX_LOCAL_ACTION_NODES,
  maxLocalDepth = MAX_LOCAL_ACTION_DEPTH,
}) => {
  const scannedDescriptors = new Set();
  const activeDescriptors = new Set();
  const discoveredDescriptors = new Set();

  const auditEntry = (entry, description = entry.sourcePath, depth = 0) => {
    const { action } = entry;
    if (action.startsWith('./')) {
      assert.ok(depth <= maxLocalDepth, `local action graph exceeds depth ${maxLocalDepth}`);
      const descriptorPath = resolveLocalActionDescriptor(rootPath, action, description);
      if (!discoveredDescriptors.has(descriptorPath)) {
        assert.ok(
          discoveredDescriptors.size < maxLocalNodes,
          `local action graph exceeds ${maxLocalNodes} nodes`,
        );
        discoveredDescriptors.add(descriptorPath);
      }
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
        const parsedUses = validateCompositeSteps(document.runs.steps, descriptorPath);

        const rawEntries = sourceUseEntries(source, descriptorPath);
        assertRawWorkflowUseEntriesCoverParsedUses(parsedUses, rawEntries, descriptorPath);
        const rawImageEntries = sourceImageEntries(source, descriptorPath);
        assertRawImageEntriesCoverParsedImages(
          collectStepRuntimeImages(document.runs.steps, descriptorPath),
          rawImageEntries,
          descriptorPath,
        );
        for (const imageEntry of rawImageEntries) recordImage(imageEntry, descriptorPath);
        for (const nestedEntry of rawEntries) auditEntry(nestedEntry, descriptorPath, depth + 1);
        scannedDescriptors.add(descriptorPath);
      } finally {
        activeDescriptors.delete(descriptorPath);
      }
      return;
    }

    auditNonLocalUseEntry(entry, description, recordExternal, recordImage);
  };

  return { auditEntry, discoveredDescriptors, scannedDescriptors };
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

const createFixtureUseAuditor = (rootPath, options = {}) => {
  const externalReferences = [];
  const imageEntries = [];
  const auditor = createUseAuditor({
    rootPath,
    recordExternal: ({ action, ref }, description) => {
      const match = action.match(actionReferencePattern);
      assert.ok(match, `${description} indirect action is unsupported or mutable: ${action}`);
      assert.match(ref ?? '', sourceRefPattern, `${description} indirect action must retain a source ref`);
      externalReferences.push({ repository: match[1], sha: match[2], ref });
    },
    recordImage: (entry, description) => {
      assert.match(entry.ref ?? '', sourceRefPattern, `${description} image must retain a source ref`);
      assertImageDigest(entry.image, description);
      imageEntries.push(entry);
    },
    ...options,
  });
  return { ...auditor, externalReferences, imageEntries };
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

const remoteDescriptorPaths = ['action.yml', 'action.yaml'];

const validateRemoteActionDescriptor = (entry, candidates) => {
  const description = `${entry.repository}@${entry.ref}`;
  const found = [];
  for (const descriptorPath of remoteDescriptorPaths) {
    const candidate = candidates.get(descriptorPath);
    assert.ok(candidate, `${description} did not return a result for ${descriptorPath}`);
    assert.ok(
      candidate.status === 200 || candidate.status === 404,
      `${description}/${descriptorPath} returned unexpected HTTP ${candidate.status}`,
    );
    if (candidate.status === 200) {
      assert.equal(typeof candidate.source, 'string', `${description}/${descriptorPath} body is missing`);
      found.push({ path: descriptorPath, source: candidate.source });
    }
  }
  assert.equal(found.length, 1, `${description} must expose exactly one root action.yml or action.yaml`);
  assert.equal(
    found[0].path,
    entry.descriptor.path,
    `${description} descriptor path does not match action-lock.yml`,
  );

  const document = yaml.load(found[0].source);
  assert.ok(
    document && typeof document === 'object' && !Array.isArray(document),
    `${description} descriptor must be a YAML map`,
  );
  assert.ok(
    document.runs && typeof document.runs === 'object' && !Array.isArray(document.runs),
    `${description} descriptor must define runs`,
  );
  const using = document.runs.using;
  const kind = using === 'composite' ? 'composite' : /^node[0-9]+$/.test(using) ? 'node' : null;
  assert.ok(kind, `${description} runs.using is unsupported: ${using}`);
  assert.equal(kind, entry.descriptor.kind, `${description} descriptor kind does not match action-lock.yml`);
  if (kind === 'composite') {
    validateCompositeSteps(document.runs.steps, description);
  } else {
    assert.equal(typeof document.runs.main, 'string', `${description} Node action must define runs.main`);
  }
  return { document, kind, path: found[0].path, source: found[0].source };
};

const readResponseTextBounded = async (response, maxBytes = MAX_DESCRIPTOR_BYTES) => {
  const contentLength = Number(response.headers?.get?.('content-length'));
  assert.ok(
    !Number.isFinite(contentLength) || contentLength <= maxBytes,
    `remote descriptor exceeds ${maxBytes} bytes`,
  );
  if (!response.body?.getReader) {
    const source = await response.text();
    assert.ok(Buffer.byteLength(source, 'utf8') <= maxBytes, `remote descriptor exceeds ${maxBytes} bytes`);
    return source;
  }
  const reader = response.body.getReader();
  const chunks = [];
  let total = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    assert.ok(total <= maxBytes, `remote descriptor exceeds ${maxBytes} bytes`);
    chunks.push(value);
  }
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
};

const signalForFetch = (globalSignal) =>
  globalSignal ? AbortSignal.any([globalSignal, AbortSignal.timeout(15_000)]) : AbortSignal.timeout(15_000);

const fetchRemoteActionDescriptor = async (entry, fetchImpl = fetch, globalSignal) => {
  const candidates = new Map();
  for (const descriptorPath of remoteDescriptorPaths) {
    const url = `https://raw.githubusercontent.com/${entry.repository}/${entry.sha}/${descriptorPath}`;
    const response = await fetchImpl(url, {
      headers: { Accept: 'text/plain' },
      signal: signalForFetch(globalSignal),
    });
    candidates.set(descriptorPath, {
      status: response.status,
      source: response.status === 200 ? await readResponseTextBounded(response) : null,
    });
  }
  return validateRemoteActionDescriptor(entry, candidates);
};

const runGitLsRemote = async (entry, globalSignal) => {
  const repository = entry.repository.toLowerCase();
  const headRef = `refs/heads/${entry.ref}`;
  const tagRef = `refs/tags/${entry.ref}`;
  const peeledTagRef = `${tagRef}^{}`;
  const env = { ...process.env, GCM_INTERACTIVE: 'Never', GIT_TERMINAL_PROMPT: '0' };
  for (const name of ['GH_TOKEN', 'GITHUB_TOKEN', 'GIT_ASKPASS', 'SSH_ASKPASS']) delete env[name];
  const { stdout } = await execFileAsync(
    'git',
    [
      '-c',
      'credential.helper=',
      '-c',
      'http.https://github.com/.extraheader=',
      'ls-remote',
      '--heads',
      '--tags',
      `https://github.com/${repository}.git`,
      headRef,
      tagRef,
      peeledTagRef,
    ],
    {
      encoding: 'utf8',
      env,
      maxBuffer: 64 * 1024,
      signal: globalSignal,
      timeout: 15_000,
      windowsHide: true,
    },
  );
  return stdout;
};

const resolveActionRef = async (entry, lsRemoteImpl = runGitLsRemote, globalSignal) => {
  const headRef = `refs/heads/${entry.ref}`;
  const tagRef = `refs/tags/${entry.ref}`;
  const peeledTagRef = `${tagRef}^{}`;
  const allowedRefs = new Set([headRef, tagRef, peeledTagRef]);
  const resolved = new Map();
  const output = await lsRemoteImpl(entry, globalSignal);
  assert.equal(typeof output, 'string', `${entry.repository}@${entry.ref} ls-remote output is missing`);
  assert.ok(Buffer.byteLength(output, 'utf8') <= 64 * 1024, 'ls-remote output exceeds 65536 bytes');
  for (const line of output.split(/\r?\n/).filter(Boolean)) {
    const match = line.match(/^([0-9a-f]{40})\t([^\s]+)$/);
    assert.ok(match, `${entry.repository}@${entry.ref} ls-remote output is malformed`);
    assert.ok(allowedRefs.has(match[2]), `${entry.repository}@${entry.ref} returned an unexpected ref`);
    assert.equal(resolved.has(match[2]), false, `${entry.repository}@${entry.ref} returned a duplicate ref`);
    resolved.set(match[2], match[1]);
  }
  const branchSha = resolved.get(headRef) ?? null;
  const tagSha = resolved.get(peeledTagRef) ?? resolved.get(tagRef) ?? null;
  assert.ok(tagSha || branchSha, `${entry.repository}@${entry.ref} is neither a tag nor a branch`);
  assert.equal(!tagSha || !branchSha, true, `${entry.repository}@${entry.ref} is ambiguously both tag and branch`);
  const resolvedSha = tagSha ?? branchSha;
  assert.equal(
    resolvedSha,
    entry.sha,
    `${entry.repository}@${entry.ref} does not resolve to the locked commit SHA`,
  );
  return resolvedSha;
};

const fetchRemoteActionDescriptors = async (
  entries,
  fetchImpl = fetch,
  lsRemoteImpl = runGitLsRemote,
) => {
  assert.ok(entries.length <= MAX_ACTION_ENTRIES, `action lock exceeds ${MAX_ACTION_ENTRIES} entries`);
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(new Error('remote provenance aggregate timeout')), REMOTE_FETCH_TIMEOUT_MS);
  const results = new Array(entries.length);
  let next = 0;
  const worker = async () => {
    while (true) {
      const index = next;
      next += 1;
      if (index >= entries.length) return;
      const entry = entries[index];
      const manifest = await fetchRemoteActionDescriptor(entry, fetchImpl, controller.signal);
      await resolveActionRef(entry, lsRemoteImpl, controller.signal);
      results[index] = manifest;
    }
  };
  try {
    await Promise.all(
      Array.from({ length: Math.min(MAX_FETCH_CONCURRENCY, entries.length) }, () => worker()),
    );
    return results;
  } catch (error) {
    controller.abort(error);
    throw error;
  } finally {
    clearTimeout(timeout);
  }
};

const auditRemoteManifestGraph = ({
  rootKeys,
  lockEntries,
  manifests,
  recordExternal,
  recordImage,
  maxNodes = MAX_REMOTE_ACTION_NODES,
  maxDepth = MAX_REMOTE_ACTION_DEPTH,
}) => {
  const scanned = new Set();
  const active = new Set();
  const discovered = new Set();

  const auditKey = (key, depth = 0) => {
    assert.ok(depth <= maxDepth, `remote action graph exceeds depth ${maxDepth}`);
    if (!discovered.has(key)) {
      assert.ok(discovered.size < maxNodes, `remote action graph exceeds ${maxNodes} nodes`);
      discovered.add(key);
    }
    assert.equal(active.has(key), false, `remote composite action cycle includes ${key.replace('\u0000', '@')}`);
    if (scanned.has(key)) return;
    const entry = lockEntries.get(key);
    assert.ok(entry, `remote action ${key.replace('\u0000', '@')} is missing from action-lock.yml`);
    const manifest = manifests.get(key);
    assert.ok(manifest, `remote action ${entry.repository}@${entry.ref} descriptor was not fetched`);
    if (manifest.kind === 'node') {
      scanned.add(key);
      return;
    }

    active.add(key);
    try {
      const description = `${entry.repository}@${entry.ref}/${manifest.path}`;
      const parsedUses = validateCompositeSteps(manifest.document.runs.steps, description);
      const rawEntries = sourceUseEntries(manifest.source, description);
      assertRawWorkflowUseEntriesCoverParsedUses(parsedUses, rawEntries, description);
      const rawImageEntries = sourceImageEntries(manifest.source, description);
      assertRawImageEntriesCoverParsedImages(
        collectStepRuntimeImages(manifest.document.runs.steps, description),
        rawImageEntries,
        description,
      );
      for (const imageEntry of rawImageEntries) recordImage(imageEntry, description);
      for (const nestedEntry of rawEntries) {
        assert.equal(
          nestedEntry.action.startsWith('./'),
          false,
          `${description} remote composite local uses are unsupported: ${nestedEntry.action}`,
        );
        if (nestedEntry.action.startsWith('docker://')) {
          auditNonLocalUseEntry(nestedEntry, description, recordExternal, recordImage);
          continue;
        }
        recordExternal(nestedEntry, description);
        const match = nestedEntry.action.match(actionReferencePattern);
        assert.ok(match, `${description} nested action is unsupported: ${nestedEntry.action}`);
        auditKey(actionLockKey({ repository: match[1], ref: nestedEntry.ref }), depth + 1);
      }
      scanned.add(key);
    } finally {
      active.delete(key);
    }
  };

  for (const key of rootKeys) auditKey(key);
  return scanned;
};

const collectCheckoutSteps = (document) =>
  Object.values(document.jobs ?? {}).flatMap((job) =>
    (job.steps ?? []).filter(
      (step) =>
        typeof step.uses === 'string' && step.uses.toLowerCase().startsWith('actions/checkout@'),
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

test('every checkout declares an explicit credential policy with only two write-path exceptions', () => {
  const allowedPersistentCheckouts = new Set([
    '.github/workflows/build-tunnel.yml\u0000update-readme\u00000',
    '.github/workflows/sync-upstream.yml\u0000sync\u00003',
  ]);
  const observedPersistentCheckouts = new Set();
  let checkoutCount = 0;

  for (const workflowFile of workflowFiles) {
    const canonicalWorkflowFile = workflowFile.replaceAll('\\', '/');
    const { source, document } = readWorkflow(workflowFile);
    for (const [jobName, job] of Object.entries(document.jobs ?? {})) {
      for (const [stepIndex, step] of (job.steps ?? []).entries()) {
        if (
          typeof step.uses !== 'string' ||
          !step.uses.toLowerCase().startsWith('actions/checkout@')
        ) {
          continue;
        }
        checkoutCount += 1;
        assert.equal(
          typeof step.with?.['persist-credentials'],
          'boolean',
          `${workflowFile} ${jobName} checkout ${stepIndex} must explicitly declare boolean persist-credentials`,
        );
        if (step.with['persist-credentials']) {
          const identity = `${canonicalWorkflowFile}\u0000${jobName}\u0000${stepIndex}`;
          assert.ok(
            allowedPersistentCheckouts.has(identity),
            `${workflowFile} ${jobName} checkout ${stepIndex} may not persist credentials`,
          );
          observedPersistentCheckouts.add(identity);
        }
      }
    }

    if (canonicalWorkflowFile === '.github/workflows/build-tunnel.yml') {
      assert.match(source, /This job pushes the generated README update with GITHUB_TOKEN/);
    }
    if (canonicalWorkflowFile === '.github/workflows/sync-upstream.yml') {
      assert.match(source, /bounded sync script fetches and pushes with this Sync App token/);
    }
  }

  assert.ok(checkoutCount > 0, 'repository must contain checkout steps');
  assert.deepEqual(
    [...observedPersistentCheckouts].sort(),
    [...allowedPersistentCheckouts].sort(),
    'only the two documented write-path checkouts may persist credentials',
  );
});

const auditSupplyChainContracts = async ({ verifyRemote }) => {
  assert.deepEqual(Object.keys(actionLock).sort(), ['actions', 'images', 'version']);
  assert.equal(actionLock.version, 1, 'action lock must use version 1');
  assert.ok(Array.isArray(actionLock.actions), 'action lock actions must be an array');
  assert.ok(Array.isArray(actionLock.images), 'action lock images must be an array');
  assert.ok(
    actionLock.actions.length <= MAX_ACTION_ENTRIES,
    `action lock must not exceed ${MAX_ACTION_ENTRIES} entries`,
  );

  const lockKeys = new Set();
  const lockEntries = new Map();
  let previousKey = '';
  for (const entry of actionLock.actions) {
    assert.deepEqual(
      Object.keys(entry).sort(),
      ['descriptor', 'ref', 'repository', 'sha'],
      'action lock entries must have only repository, ref, sha, and descriptor',
    );
    assert.match(
      entry.repository,
      /^[A-Za-z0-9][A-Za-z0-9_.-]*\/[A-Za-z0-9][A-Za-z0-9_.-]*$/,
      'action lock repository must be an owner/repository pair',
    );
    assert.match(entry.ref, sourceRefPattern, 'action lock source ref is invalid');
    assert.match(entry.sha, /^[0-9a-f]{40}$/, 'action lock SHA is invalid');
    assert.ok(
      entry.descriptor && typeof entry.descriptor === 'object' && !Array.isArray(entry.descriptor),
      'action lock descriptor must be a map',
    );
    assert.deepEqual(
      Object.keys(entry.descriptor).sort(),
      ['kind', 'path'],
      'action lock descriptor must have only kind and path',
    );
    assert.ok(
      remoteDescriptorPaths.includes(entry.descriptor.path),
      'action lock descriptor path must be action.yml or action.yaml',
    );
    assert.ok(
      ['composite', 'node'].includes(entry.descriptor.kind),
      'action lock descriptor kind must be composite or node',
    );
    const key = actionLockKey(entry);
    assert.ok(key > previousKey, 'action lock entries must be strictly sorted by repository and ref');
    assert.equal(lockKeys.has(key), false, `action lock duplicates ${entry.repository}@${entry.ref}`);
    lockKeys.add(key);
    lockEntries.set(key, entry);
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
  const recordImage = (entry, description) => {
    assert.match(
      entry.ref ?? '',
      sourceRefPattern,
      `${description} image must retain a source ref comment`,
    );
    const { image, digest } = assertImageDigest(entry.image, description);
    const key = imageLockKey({ image, ref: entry.ref });
    const priorDigest = workflowImageRefs.get(key);
    assert.ok(
      priorDigest === undefined || priorDigest === digest,
      `${description} maps ${image}@${entry.ref} to more than one digest`,
    );
    workflowImageRefs.set(key, digest);
    workflowImageKeys.add(key);
    assert.equal(
      imageLockEntries.get(key),
      digest,
      `${description} image ${image}@${entry.ref} must exactly match action-lock.yml`,
    );
  };
  const recordExternal = ({ action, ref }, description) => {
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
      lockEntries.get(key)?.sha,
      sha,
      `${description} action ${repository}@${ref} must exactly match action-lock.yml`,
    );
  };
  const useAuditor = createUseAuditor({
    rootPath: repoRoot,
    recordExternal,
    recordImage,
  });

  assert.ok(workflowFiles.length > 0, 'repository must contain workflow files');
  for (const workflowFile of workflowFiles) {
    const { document } = readWorkflow(workflowFile);
    const parsedUses = collectUses(document.jobs);
    const rawEntries = workflowUseEntries(workflowFile);
    assertRawWorkflowUseEntriesCoverParsedUses(parsedUses, rawEntries, workflowFile);
    for (const entry of rawEntries) useAuditor.auditEntry(entry, workflowFile);

    for (const entry of workflowImageEntries(workflowFile, document)) recordImage(entry, workflowFile);
  }

  if (verifyRemote) {
    const fetchedManifests = await fetchRemoteActionDescriptors(actionLock.actions);
    const manifests = new Map(
      actionLock.actions.map((entry, index) => [actionLockKey(entry), fetchedManifests[index]]),
    );
    auditRemoteManifestGraph({
      rootKeys: [...workflowKeys],
      lockEntries,
      manifests,
      recordExternal,
      recordImage,
    });
    assert.deepEqual(
      [...lockKeys].sort(),
      [...workflowKeys].sort(),
      'action lock must not contain unused entries and must cover every external action',
    );
    assert.deepEqual(
      [...imageLockKeys].sort(),
      [...workflowImageKeys].sort(),
      'image lock must not contain unused entries and must cover every runtime image',
    );
  }
};

test('action and image locks are strict and cover direct workflow references', async () => {
  await auditSupplyChainContracts({ verifyRemote: false });
});

test('required Automation Contracts CI runs the live provenance audit without credentials', () => {
  const { document } = readWorkflow('.github/workflows/frontend-ci.yml');
  const step = document.jobs?.automation?.steps?.find(
    (candidate) => candidate.name === 'Verify remote action provenance',
  );
  assert.ok(step, 'Automation Contracts must include the live remote provenance step');
  assert.match(step.run ?? '', /--test-name-pattern="live remote action provenance"/);
  assert.equal(step.env?.AERIS_VERIFY_REMOTE_ACTIONS, '1');
  assert.equal(Object.hasOwn(step.env ?? {}, 'GITHUB_TOKEN'), false);
  assert.equal(hasSecretReference(step), false);
});

test(
  'live remote action provenance is verified at exact refs and SHAs',
  { skip: process.env.AERIS_VERIFY_REMOTE_ACTIONS !== '1' },
  async () => {
    await auditSupplyChainContracts({ verifyRemote: true });
  },
);

test('container workflow actions require immutable lowercase sha256 digests', () => {
  const auditor = createFixtureUseAuditor(repoRoot);
  auditor.auditEntry({
    action: `docker://example.invalid/tool@sha256:${'a'.repeat(64)}`,
    ref: 'v1',
    sourcePath: 'valid container action',
  });
  assert.equal(auditor.imageEntries.length, 1);
  for (const action of [
    'docker://example.invalid/tool:latest',
    'docker://example.invalid/tool',
    `docker://example.invalid/tool@sha256:${'A'.repeat(64)}`,
    `docker://example.invalid/tool@sha256:${'a'.repeat(63)}`,
  ]) {
    assert.throws(() =>
      createFixtureUseAuditor(repoRoot).auditEntry({
        action,
        ref: 'v1',
        sourcePath: 'invalid container action',
      }),
    );
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

test('remote descriptor validator fails closed on provenance and metadata mismatches', () => {
  const entry = {
    repository: 'example/action',
    ref: 'v1',
    sha: 'a'.repeat(40),
    descriptor: { path: 'action.yml', kind: 'node' },
  };
  const candidates = (actionYml, actionYaml = { status: 404, source: null }) =>
    new Map([
      ['action.yml', actionYml],
      ['action.yaml', actionYaml],
    ]);
  const nodeSource = ['name: fixture', 'runs:', '  using: node20', '  main: index.js'].join('\n');
  assert.equal(
    validateRemoteActionDescriptor(
      entry,
      candidates({ status: 200, source: nodeSource }),
    ).kind,
    'node',
  );
  assert.throws(
    () =>
      validateRemoteActionDescriptor(
        entry,
        candidates({ status: 404, source: null }),
      ),
    /exactly one/,
  );
  assert.throws(
    () =>
      validateRemoteActionDescriptor(
        entry,
        candidates(
          { status: 200, source: nodeSource },
          { status: 200, source: nodeSource },
        ),
      ),
    /exactly one/,
  );
  assert.throws(
    () =>
      validateRemoteActionDescriptor(
        { ...entry, descriptor: { path: 'action.yaml', kind: 'node' } },
        candidates({ status: 200, source: nodeSource }),
      ),
    /path does not match/,
  );
  assert.throws(
    () =>
      validateRemoteActionDescriptor(
        { ...entry, descriptor: { path: 'action.yml', kind: 'composite' } },
        candidates({ status: 200, source: nodeSource }),
      ),
    /kind does not match/,
  );
  assert.throws(
    () =>
      validateRemoteActionDescriptor(
        entry,
        candidates({ status: 200, source: 'runs: [' }),
      ),
  );
  assert.throws(
    () =>
      validateRemoteActionDescriptor(
        entry,
        candidates({ status: 200, source: ['runs:', '  using: docker', '  image: Dockerfile'].join('\n') }),
      ),
    /unsupported/,
  );
  assert.throws(
    () =>
      validateRemoteActionDescriptor(
        entry,
        candidates({ status: 503, source: null }),
      ),
    /unexpected HTTP 503/,
  );
});

test('action repository identities are canonical across case aliases', () => {
  assert.equal(
    actionLockKey({ repository: 'Docker/Setup-QEMU-Action', ref: 'v3' }),
    actionLockKey({ repository: 'docker/setup-qemu-action', ref: 'v3' }),
  );
});

test('git source refs resolve unambiguously to the locked commit', async () => {
  const lockedSha = 'a'.repeat(40);
  const tagObjectSha = 'b'.repeat(40);
  const entry = {
    repository: 'example/action',
    ref: 'v1',
    sha: lockedSha,
    descriptor: { path: 'action.yml', kind: 'node' },
  };
  const resolver = (output) => async () => output;
  await assert.doesNotReject(() =>
    resolveActionRef(
      entry,
      resolver(`${lockedSha}\trefs/tags/v1\n`),
    ),
  );
  await assert.doesNotReject(() =>
    resolveActionRef(
      entry,
      resolver(
        `${tagObjectSha}\trefs/tags/v1\n${lockedSha}\trefs/tags/v1^{}\n`,
      ),
    ),
  );
  await assert.doesNotReject(() =>
    resolveActionRef(
      entry,
      resolver(`${lockedSha}\trefs/heads/v1\n`),
    ),
  );
  await assert.rejects(
    () =>
      resolveActionRef(
        entry,
        resolver(`${lockedSha}\trefs/tags/v1\n${lockedSha}\trefs/heads/v1\n`),
      ),
    /ambiguously both tag and branch/,
  );
  await assert.rejects(
    () =>
      resolveActionRef(
        entry,
        resolver(`${'c'.repeat(40)}\trefs/tags/v1\n`),
      ),
    /does not resolve to the locked commit SHA/,
  );
  await assert.rejects(
    () =>
      resolveActionRef(
        entry,
        resolver(`${lockedSha}\trefs/tags/v1\n${lockedSha}\trefs/tags/unexpected\n`),
      ),
    /unexpected ref/,
  );
  await assert.rejects(() => resolveActionRef(entry, resolver('not ls-remote output\n')), /malformed/);
  await assert.rejects(
    () => resolveActionRef(entry, async () => Promise.reject(new Error('network failed'))),
    /network failed/,
  );
});

test('remote provenance fetches are size, count, and concurrency bounded', async () => {
  await assert.rejects(
    () =>
      readResponseTextBounded(
        { headers: { get: () => null }, text: async () => 'x'.repeat(17) },
        16,
      ),
    /exceeds 16 bytes/,
  );
  await assert.rejects(
    () => fetchRemoteActionDescriptors(Array.from({ length: MAX_ACTION_ENTRIES + 1 })),
    /action lock exceeds/,
  );

  const entries = Array.from({ length: MAX_FETCH_CONCURRENCY + 2 }, (_, index) => ({
    repository: `example/action-${index}`,
    ref: `v${index}`,
    sha: String(index + 1).repeat(40),
    descriptor: { path: 'action.yml', kind: 'node' },
  }));
  let active = 0;
  let peak = 0;
  const fetchImpl = async (url) => {
    active += 1;
    peak = Math.max(peak, active);
    await new Promise((resolve) => setTimeout(resolve, 2));
    active -= 1;
    const entry = entries.find((candidate) => url.includes(`/${candidate.repository}/`));
    assert.ok(entry, `unexpected bounded-fetch URL: ${url}`);
    if (url.endsWith('/action.yml')) {
      return {
        status: 200,
        headers: { get: () => null },
        text: async () => ['name: fixture', 'runs:', '  using: node20', '  main: index.js'].join('\n'),
      };
    }
    if (url.endsWith('/action.yaml')) {
      return { status: 404, headers: { get: () => null }, text: async () => '' };
    }
    assert.fail(`unexpected bounded-fetch URL: ${url}`);
  };
  const lsRemoteImpl = async (entry) => `${entry.sha}\trefs/tags/${entry.ref}\n`;
  assert.equal(
    (await fetchRemoteActionDescriptors(entries, fetchImpl, lsRemoteImpl)).length,
    entries.length,
  );
  assert.ok(peak <= MAX_FETCH_CONCURRENCY, `fetch concurrency exceeded limit: ${peak}`);
});

test('remote composite graph recursively audits actions and all runtime images', () => {
  const digestA = `sha256:${'d'.repeat(64)}`;
  const digestB = `sha256:${'e'.repeat(64)}`;
  const root = {
    repository: 'example/root',
    ref: 'v1',
    sha: 'a'.repeat(40),
    descriptor: { path: 'action.yml', kind: 'composite' },
  };
  const dependency = {
    repository: 'example/dependency',
    ref: 'v2',
    sha: 'b'.repeat(40),
    descriptor: { path: 'action.yml', kind: 'node' },
  };
  const qemu = {
    repository: 'docker/setup-qemu-action',
    ref: 'v3',
    sha: 'c'.repeat(40),
    descriptor: { path: 'action.yml', kind: 'node' },
  };
  const source = [
    'name: remote composite fixture',
    'runs:',
    '  using: composite',
    '  steps:',
    `    - uses: example/dependency@${dependency.sha} # v2`,
    `    - uses: docker://example.invalid/tool@${digestA} # stable`,
    `    - uses: docker/setup-qemu-action@${qemu.sha} # v3`,
    '      with:',
    `        image: docker.io/tonistiigi/binfmt@${digestB} # latest`,
  ].join('\n');
  const lockEntries = new Map([root, dependency, qemu].map((entry) => [actionLockKey(entry), entry]));
  const manifests = new Map([
    [actionLockKey(root), { document: yaml.load(source), kind: 'composite', path: 'action.yml', source }],
    [actionLockKey(dependency), { document: {}, kind: 'node', path: 'action.yml', source: '' }],
    [actionLockKey(qemu), { document: {}, kind: 'node', path: 'action.yml', source: '' }],
  ]);
  const observedActions = new Set([actionLockKey(root)]);
  const images = [];
  const recordExternal = ({ action, ref }, description) => {
    const match = action.match(actionReferencePattern);
    assert.ok(match, `${description} nested action must be pinned`);
    const key = actionLockKey({ repository: match[1], ref });
    assert.equal(lockEntries.get(key)?.sha, match[2], `${description} nested action must match lock`);
    observedActions.add(key);
  };
  const recordImage = (entry, description) => {
    assert.match(entry.ref ?? '', sourceRefPattern, `${description} image source ref is missing`);
    assertImageDigest(entry.image, description);
    images.push(entry);
  };
  assert.deepEqual(
    [...auditRemoteManifestGraph({
      rootKeys: [actionLockKey(root)],
      lockEntries,
      manifests,
      recordExternal,
      recordImage,
    })].sort(),
    [actionLockKey(dependency), actionLockKey(qemu), actionLockKey(root)].sort(),
  );
  assert.deepEqual([...observedActions].sort(), [...lockEntries.keys()].sort());
  assert.deepEqual(
    images.map((entry) => entry.image).sort(),
    [`docker.io/tonistiigi/binfmt@${digestB}`, `example.invalid/tool@${digestA}`].sort(),
  );
  assert.throws(
    () =>
      auditRemoteManifestGraph({
        rootKeys: [actionLockKey(root)],
        lockEntries,
        manifests,
        recordExternal,
        recordImage,
        maxDepth: 0,
      }),
    /exceeds depth 0/,
  );
  assert.throws(
    () =>
      auditRemoteManifestGraph({
        rootKeys: [actionLockKey(root)],
        lockEntries,
        manifests,
        recordExternal,
        recordImage,
        maxNodes: 1,
      }),
    /exceeds 1 nodes/,
  );

  for (const unsafeUses of ['./nested', 'example/dependency@v2']) {
    const unsafeSource = [
      'name: unsafe remote composite',
      'runs:',
      '  using: composite',
      '  steps:',
      `    - uses: ${unsafeUses} # v2`,
    ].join('\n');
    const unsafeManifests = new Map(manifests);
    unsafeManifests.set(actionLockKey(root), {
      document: yaml.load(unsafeSource),
      kind: 'composite',
      path: 'action.yml',
      source: unsafeSource,
    });
    assert.throws(() =>
      auditRemoteManifestGraph({
        rootKeys: [actionLockKey(root)],
        lockEntries,
        manifests: unsafeManifests,
        recordExternal,
        recordImage,
      }),
    );
  }

  for (const imageLines of [[], ['      with:', '        image: binfmt:latest # latest']]) {
    const unsafeQemuSource = [
      'name: unsafe remote qemu composite',
      'runs:',
      '  using: composite',
      '  steps:',
      `    - uses: docker/setup-qemu-action@${qemu.sha} # v3`,
      ...imageLines,
    ].join('\n');
    const unsafeManifests = new Map(manifests);
    unsafeManifests.set(actionLockKey(root), {
      document: yaml.load(unsafeQemuSource),
      kind: 'composite',
      path: 'action.yml',
      source: unsafeQemuSource,
    });
    assert.throws(() =>
      auditRemoteManifestGraph({
        rootKeys: [actionLockKey(root)],
        lockEntries,
        manifests: unsafeManifests,
        recordExternal,
        recordImage,
      }),
    );
  }
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

test('local composite audit bounds descriptor bytes, graph nodes, and recursion depth', () => {
  const composite = (uses) =>
    [
      'name: bounded fixture',
      'description: bounded fixture',
      'runs:',
      '  using: composite',
      '  steps:',
      `    - uses: ${uses}`,
    ].join('\n');
  const terminal = [
    'name: terminal fixture',
    'description: terminal fixture',
    'runs:',
    '  using: composite',
    '  steps:',
    '    - run: echo bounded',
    '      shell: bash',
  ].join('\n');

  withFixtureRepo(
    {
      '.github/actions/oversized/action.yml': `${terminal}\n#${'x'.repeat(MAX_DESCRIPTOR_BYTES)}`,
    },
    (fixtureRoot) => {
      assert.throws(
        () =>
          createFixtureUseAuditor(fixtureRoot).auditEntry({
            action: './.github/actions/oversized',
            sourcePath: 'oversized fixture workflow',
          }),
        /descriptor exceeds/,
      );
    },
  );

  withFixtureRepo(
    {
      '.github/actions/a/action.yml': composite('./.github/actions/b'),
      '.github/actions/b/action.yml': composite('./.github/actions/c'),
      '.github/actions/c/action.yml': terminal,
    },
    (fixtureRoot) => {
      assert.throws(
        () =>
          createFixtureUseAuditor(fixtureRoot, { maxLocalNodes: 2 }).auditEntry({
            action: './.github/actions/a',
            sourcePath: 'node-bound fixture workflow',
          }),
        /exceeds 2 nodes/,
      );
      assert.throws(
        () =>
          createFixtureUseAuditor(fixtureRoot, { maxLocalDepth: 1 }).auditEntry({
            action: './.github/actions/a',
            sourcePath: 'depth-bound fixture workflow',
          }),
        /exceeds depth 1/,
      );
    },
  );
});

test('docker action audit requires exact image lock coverage and rejects mutable tags', () => {
  withFixtureRepo({}, (fixtureRoot) => {
    const auditor = createFixtureUseAuditor(fixtureRoot);
    const digest = `sha256:${'c'.repeat(64)}`;
    const immutable = `docker://example.invalid/tool@${digest}`;
    auditor.auditEntry({ action: immutable, ref: 'v1', sourcePath: 'fixture workflow' });
    const lock = [{ image: 'example.invalid/tool', ref: 'v1', digest }];
    assert.doesNotThrow(() =>
      assertFixtureImageLockCoverage(lock, auditor.imageEntries, 'docker action fixture'),
    );
    assert.throws(
      () => assertFixtureImageLockCoverage([], auditor.imageEntries, 'missing docker lock fixture'),
      /missing from the fixture lock/,
    );
    assert.throws(
      () =>
        assertFixtureImageLockCoverage(
          [...lock, { image: 'example.invalid/stale', ref: 'v1', digest }],
          auditor.imageEntries,
          'stale docker lock fixture',
        ),
      /stale entry/,
    );
    assert.throws(
      () =>
        assertFixtureImageLockCoverage(
          [{ ...lock[0], digest: `sha256:${'d'.repeat(64)}` }],
          auditor.imageEntries,
          'wrong docker digest fixture',
        ),
      /missing from the fixture lock/,
    );
    assert.throws(
      () =>
        auditor.auditEntry({
          action: 'docker://example.invalid/tool:latest',
          ref: 'latest',
          sourcePath: 'fixture workflow',
        }),
      /lowercase sha256 digest/,
    );
    assert.throws(
      () => auditor.auditEntry({ action: immutable, sourcePath: 'fixture workflow' }),
      /source ref comment/,
    );
  });
});

test('recursive local composite setup-qemu images require explicit immutable lock coverage', () => {
  const qemuDigest = `sha256:${'f'.repeat(64)}`;
  const qemuAction = `docker/setup-qemu-action@${'a'.repeat(40)}`;
  withFixtureRepo(
    {
      '.github/actions/qemu/action.yml': [
        'name: qemu fixture',
        'description: qemu fixture',
        'runs:',
        '  using: composite',
        '  steps:',
        `    - uses: ${qemuAction} # v3`,
        '      with:',
        `        image: docker.io/tonistiigi/binfmt@${qemuDigest} # latest`,
      ].join('\n'),
    },
    (fixtureRoot) => {
      const auditor = createFixtureUseAuditor(fixtureRoot);
      auditor.auditEntry({ action: './.github/actions/qemu', sourcePath: 'fixture workflow' });
      assert.doesNotThrow(() =>
        assertFixtureImageLockCoverage(
          [{ image: 'docker.io/tonistiigi/binfmt', ref: 'latest', digest: qemuDigest }],
          auditor.imageEntries,
          'nested qemu fixture',
        ),
      );
      assert.throws(
        () => assertFixtureImageLockCoverage([], auditor.imageEntries, 'unlocked nested qemu fixture'),
        /missing from the fixture lock/,
      );
    },
  );

  for (const imageLine of [null, '        image: docker.io/tonistiigi/binfmt:latest # latest']) {
    withFixtureRepo(
      {
        '.github/actions/qemu/action.yml': [
          'name: unsafe qemu fixture',
          'description: unsafe qemu fixture',
          'runs:',
          '  using: composite',
          '  steps:',
          `    - uses: ${qemuAction} # v3`,
          ...(imageLine === null ? [] : ['      with:', imageLine]),
        ].join('\n'),
      },
      (fixtureRoot) => {
        assert.throws(
          () =>
            createFixtureUseAuditor(fixtureRoot).auditEntry({
              action: './.github/actions/qemu',
              sourcePath: 'fixture workflow',
            }),
          imageLine === null ? /must define with.image/ : /lowercase sha256 digest/,
        );
      },
    );
  }
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

  for (const repository of ['docker/setup-qemu-action', 'Docker/Setup-QEMU-Action']) {
    const hiddenQemuDefault = [
      'jobs:',
      '  test:',
      '    runs-on: ubuntu-latest',
      '    steps:',
      `      - uses: ${repository}@${'a'.repeat(40)}`,
    ].join('\n');
    assert.throws(
      () => imageEntriesFromWorkflowSource(hiddenQemuDefault, 'setup-qemu fixture'),
      /must define with.image/,
    );
  }
});
