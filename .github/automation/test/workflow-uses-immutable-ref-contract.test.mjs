import assert from 'node:assert/strict';
import { mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const yaml = createRequire(import.meta.url)('js-yaml');
const testDirectory = path.dirname(fileURLToPath(import.meta.url));
const workflowsDirectory = path.resolve(testDirectory, '..', '..', 'workflows');

async function workflowFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = await Promise.all(entries.map(async (entry) => {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) return workflowFiles(entryPath);
    return /\.ya?ml$/i.test(entry.name) ? [entryPath] : [];
  }));
  return files.flat();
}

function collectUses(node, entries, nodePath = '$') {
  if (Array.isArray(node)) {
    node.forEach((value, index) => collectUses(value, entries, `${nodePath}[${index}]`));
    return;
  }
  if (node === null || typeof node !== 'object') return;

  if (Object.hasOwn(node, 'uses')) {
    entries.push({ reference: node.uses, node, nodePath });
  }
  for (const [key, value] of Object.entries(node)) {
    collectUses(value, entries, `${nodePath}.${key}`);
  }
}

function assertImmutableReference(reference, workflowPath) {
  assert.equal(typeof reference, 'string', `${workflowPath}: uses must be a string`);
  if (reference.startsWith('./')) return;

  if (reference.startsWith('docker://')) {
    assert.match(
      reference,
      /^docker:\/\/[^@]+@sha256:[0-9a-f]{64}$/,
      `${workflowPath}: Docker action must use a sha256 digest: ${reference}`,
    );
    return;
  }

  assert.match(
    reference,
    /^[^@\s]+@[0-9a-f]{40}$/,
    `${workflowPath}: external action must use a full 40-character commit SHA: ${reference}`,
  );
}

async function auditWorkflowFile(workflowPath) {
  const document = yaml.load(await readFile(workflowPath, 'utf8'));
  const entries = [];
  collectUses(document, entries);
  for (const entry of entries) {
    const location = `${workflowPath}:${entry.nodePath}`;
    assertImmutableReference(entry.reference, location);
    if (/^actions\/checkout@[0-9a-f]{40}$/.test(entry.reference)) {
      assert.equal(
        entry.node.with?.['persist-credentials'],
        false,
        `${location}: actions/checkout must set persist-credentials to boolean false`,
      );
    }
  }
  return entries;
}

async function withTemporaryWorkflow(source, assertion) {
  const directory = await mkdtemp(path.join(tmpdir(), 'aeris-workflow-uses-'));
  const workflowPath = path.join(directory, 'fixture.yml');
  await writeFile(workflowPath, source);
  try {
    await assertion(workflowPath);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

test('all workflow uses references are immutable and checkouts drop credentials', async () => {
  const files = await workflowFiles(workflowsDirectory);
  assert.ok(files.length > 0, 'expected workflow files');

  const entries = (await Promise.all(files.sort().map(auditWorkflowFile))).flat();
  assert.ok(entries.length > 0, 'expected workflow uses references');
  assert.ok(entries.some((entry) => entry.reference.startsWith('actions/checkout@')));
});

test('structured YAML audit rejects a spaced uses key', async () => {
  await withTemporaryWorkflow(`jobs:\n  test:\n    steps:\n      - uses : actions/setup-node@v5\n`, async (workflowPath) => {
    await assert.rejects(auditWorkflowFile(workflowPath), /full 40-character commit SHA/);
  });
});

test('structured YAML audit rejects a flow mapping checkout without disabled credentials', async () => {
  await withTemporaryWorkflow(
    'jobs:\n  test:\n    steps: [{ uses: actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09 }]\n',
    async (workflowPath) => {
      await assert.rejects(auditWorkflowFile(workflowPath), /persist-credentials to boolean false/);
    },
  );
});

test('structured YAML audit permits local actions and digest-pinned Docker actions', async () => {
  await withTemporaryWorkflow(
    `jobs:\n  test:\n    uses: ./.github/workflows/reusable.yml\n  container:\n    steps:\n      - uses: docker://example.invalid/action@sha256:${'a'.repeat(64)}\n`,
    async (workflowPath) => {
      const entries = await auditWorkflowFile(workflowPath);
      assert.equal(entries.length, 2);
    },
  );
});
