import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const yaml = require('js-yaml');
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');

test('release builds once, scans before any image publication, and attests the copied index', () => {
  const workflow = yaml.load(fs.readFileSync(path.join(root, '.github/workflows/release.yml'), 'utf8'));
  const steps = workflow.jobs.publish.steps;
  const builds = steps.filter((step) => step.uses?.startsWith('docker/build-push-action@'));
  assert.equal(builds.length, 1);
  assert.equal(builds[0].with.push, false);
  assert.match(builds[0].with.outputs, /^type=oci,dest=/);
  assert.equal(builds[0].with.platforms, 'linux/amd64,linux/arm64');
  const scan = steps.findIndex((step) => step.name === 'Scan both immutable runtime images');
  const publish = steps.findIndex((step) => step.id === 'push');
  assert.ok(scan > steps.indexOf(builds[0]) && publish > scan);
  assert.match(steps[scan].run, /release_image_gate\.py scan/);
  assert.match(steps[scan].run, /--expected-index-digest/);
  assert.equal(steps[scan].env.EXPECTED_INDEX_DIGEST, '${{ steps.image.outputs.digest }}');
  assert.match(steps[publish].run, /release_image_gate\.py publish/);
  assert.equal(steps[publish].if, undefined, 'publication must retain the success() gate');
  for (const step of steps.filter((step) => step.name?.match(/^Attest .*image provenance$/))) {
    assert.ok(steps.indexOf(step) > publish);
    assert.equal(step.with['subject-digest'], '${{ steps.push.outputs.digest }}');
  }
  const evidence = steps.find((step) => step.name === 'Retain image gate evidence including failures');
  assert.equal(evidence.if, 'always()');
  assert.ok(evidence.with['retention-days'] >= 30);
  const policy = JSON.parse(fs.readFileSync(path.join(root, '.github/security/release-image-policy.json'), 'utf8'));
  assert.equal(policy.trivy_version, '0.74.0');
  assert.equal(policy.trivy_linux_amd64_sha256, '2ae6fe3ee734b7fdf11335663e18c75ea12dccc76062f09f164a3b0f8be4371a');
  assert.equal(policy.skopeo_image, 'quay.io/skopeo/stable:v1.22.0@sha256:06dd47ee861e143268f0b811cdf1f9d6509b097945de447dbcdfb668ca15364c');
  assert.equal(policy.db_repository, 'ghcr.io/aquasecurity/trivy-db:2');
});

test('image gate executable fixtures reject incomplete scans and mismatched publication', () => {
  execFileSync('python3', ['-B', path.join(root, 'tools/ci/test_release_image_gate.py')], {
    cwd: root, encoding: 'utf8', timeout: 120000, stdio: 'pipe',
  });
});
