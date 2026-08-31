import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const yaml = require('js-yaml');
const testDirectory = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(testDirectory, '..', '..', '..');
const workflowPath = path.join(repoRoot, '.github', 'workflows', 'release.yml');

const expectedActions = new Map([
  ['actions/checkout', 'fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09'],
  ['actions/setup-node', '49933ea5288caeca8642d1e84afbd3f7d6820020'],
  ['actions/upload-artifact', '330a01c490aca151604b8cf639adc76d48f6c5d4'],
  ['dtolnay/rust-toolchain', '4360b52568e2003a75bf9bc1d59f33a8e3fc893c'],
  ['Swatinem/rust-cache', '6323deb102c322ba6fcbdcafc7e3dddab59af2b6'],
  ['taiki-e/install-action', '7a74ec2e18628d3a08d2fd4b55aea54cd5de1cfd'],
  ['actions/download-artifact', '634f93cb2916e3fdff6788551b99b062d0335ce0'],
  ['docker/setup-qemu-action', 'c7c53464625b32c7a7e944ae62b3e17d2b600130'],
  ['docker/setup-buildx-action', '8d2750c68a42422c14e847fe6c8ac0403b4cbd6f'],
  ['docker/login-action', 'c94ce9fb468520275223c153574b00df6fe4bcc9'],
  ['docker/metadata-action', 'c299e40c65443455700f0fdfc63efafe5b349051'],
  ['docker/build-push-action', '10e90e3645eae34f1e60eeb005ba3a3d33f178e8'],
  ['softprops/action-gh-release', '3bb12739c298aeb8a4eeaf626c5b8d85266b0e65'],
]);

function workflow() {
  return yaml.load(fs.readFileSync(workflowPath, 'utf8'));
}

function actionRefs(document) {
  return Object.values(document.jobs)
    .flatMap((job) => job.steps ?? [])
    .map((step) => step.uses)
    .filter(Boolean);
}

test('release writes exist only in the single protected publish job', () => {
  const document = workflow();
  assert.deepEqual(document.permissions, { contents: 'read' });
  assert.equal(document.jobs.publish.environment, 'release');
  assert.deepEqual(document.jobs.publish.permissions, { contents: 'write', packages: 'write' });
  assert.ok(document.jobs.publish.needs.includes('package'));
  assert.ok(!document.jobs.publish.needs.includes('release-approval-canary'));
  assert.equal(document.jobs['github-release'], undefined);

  for (const [name, job] of Object.entries(document.jobs)) {
    if (name === 'publish') continue;
    assert.doesNotMatch(JSON.stringify(job.permissions ?? {}), /write/);
  }

  const publishSteps = JSON.stringify(document.jobs.publish.steps);
  assert.match(publishSteps, /docker\/build-push-action@/);
  assert.match(publishSteps, /softprops\/action-gh-release@/);
  assert.match(publishSteps, /gh api -X DELETE/);
});

test('release approval canary is explicit, default-branch-only, and read-only', () => {
  const document = workflow();
  const canary = document.jobs['release-approval-canary'];
  assert.deepEqual(document.on.workflow_dispatch.inputs.release_approval_canary, {
    description: 'Confirm the release Environment approval boundary without publishing',
    required: false,
    type: 'boolean',
    default: false,
  });
  assert.equal(canary.environment, 'release');
  assert.deepEqual(canary.permissions, { contents: 'read' });
  assert.match(canary.if, /inputs\.release_approval_canary == true/);
  assert.match(canary.if, /github\.ref_type == 'branch'/);
  assert.match(canary.if, /github\.event\.repository\.default_branch/);
  assert.deepEqual(canary.needs, 'preflight');
  assert.deepEqual(canary.outputs, {
    run_id: '${{ steps.evidence.outputs.run_id }}',
    ref: '${{ steps.evidence.outputs.ref }}',
    sha: '${{ steps.evidence.outputs.sha }}',
  });
  assert.equal(canary.steps.length, 1);
  assert.equal(canary.steps[0].uses, undefined);
  assert.doesNotMatch(JSON.stringify(canary.steps), /publish|release-assets|gh api/i);
  for (const name of ['frontend', 'build', 'package', 'publish']) {
    assert.match(document.jobs[name].if, /inputs\.release_approval_canary != true/);
  }
});

test('release workflow pins every action to an approved immutable commit', () => {
  const document = workflow();
  const refs = actionRefs(document);
  assert.ok(refs.length > 0);
  for (const ref of refs) {
    const match = /^(?<action>[^@]+)@(?<sha>[0-9a-f]{40})$/.exec(ref);
    assert.ok(match, `action reference must use a full 40-character SHA: ${ref}`);
    assert.equal(expectedActions.get(match.groups.action), match.groups.sha, `unexpected action SHA: ${ref}`);
  }
  assert.deepEqual(new Set(refs.map((ref) => ref.split('@')[0])), new Set(expectedActions.keys()));
  for (const job of Object.values(document.jobs)) {
    for (const step of job.steps ?? []) {
      if (step.uses?.startsWith('actions/checkout@')) {
        assert.deepEqual(step.with, { 'persist-credentials': false });
      }
    }
  }
});
