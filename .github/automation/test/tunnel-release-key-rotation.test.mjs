import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const yaml = require('js-yaml');
const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const read = (name) => fs.readFileSync(path.join(repoRoot, name), 'utf8');
const workflow = () => yaml.load(read('.github/workflows/build-tunnel.yml'));
const publicInputs = [
  'AETHER_TUNNEL_RELEASE_KEY_ID',
  'AETHER_TUNNEL_RELEASE_PUBLIC_KEY',
  'AETHER_TUNNEL_RELEASE_TRUST_KEYS',
];

test('every tunnel release build and publication verifier gets identical public trust inputs', () => {
  const { jobs } = workflow();
  const verify = jobs.release.steps.find((step) => step.name === 'Verify signed release manifest');
  for (const input of publicInputs) {
    const expected = `\${{ vars.${input} }}`;
    assert.equal(jobs.preflight.env[input], expected);
    assert.equal(jobs.build.env[input], expected);
    assert.equal(verify.env[input], expected);
  }
  assert.match(JSON.stringify(jobs.preflight.steps), /tunnel-release-verifier\/Cargo\.toml -- check/);
  const build = jobs.build.steps.find((step) => step.name === 'Build');
  assert.match(build.run, /tunnel-release-verifier\/Cargo\.toml -- check --allow-unconfigured/);
  assert.ok(build.run.indexOf('-- check') < build.run.indexOf('cross build'));
  assert.equal(jobs.build.env.CROSS_CONFIG, '${{ github.workspace }}/apps/aether-tunnel/Cross.toml');
  const passthrough = [...read('apps/aether-tunnel/Cross.toml').matchAll(/"(AETHER_[A-Z_]+)"/g)]
    .map((match) => match[1]);
  assert.deepEqual(passthrough, publicInputs);
});

test('release checks out the production verifier and verifies before publication', () => {
  const steps = workflow().jobs.release.steps;
  const checkout = steps.findIndex((step) => step.uses?.startsWith('actions/checkout@'));
  const verify = steps.findIndex((step) => step.name === 'Verify signed release manifest');
  const publish = steps.findIndex((step) => step.uses?.startsWith('softprops/action-gh-release@'));
  assert.ok(checkout >= 0 && checkout < verify && verify < publish);
  assert.equal(steps[checkout].with['persist-credentials'], false);
  const script = read('.github/workflows/scripts/verify-tunnel-release.sh');
  assert.match(script, /cargo run --quiet --locked/);
  assert.match(script, /tunnel-release-verifier\/Cargo\.toml/);
  assert.match(read('tools/ci/tunnel-release-verifier/src/main.rs'), /apps\/aether-tunnel\/src\/setup\/provenance.rs/);
});

test('only the signing step receives private key material', () => {
  const document = workflow();
  for (const [name, job] of Object.entries(document.jobs)) {
    assert.doesNotMatch(JSON.stringify(job.env ?? {}), /PRIVATE_KEY|secrets\./);
    for (const step of job.steps ?? []) {
      if (name === 'release' && step.name === 'Sign release manifest') {
        assert.equal(step.env.AETHER_TUNNEL_RELEASE_PRIVATE_KEY_PEM,
          '${{ secrets.AETHER_TUNNEL_RELEASE_PRIVATE_KEY_PEM }}');
      } else {
        assert.doesNotMatch(JSON.stringify(step.env ?? {}), /PRIVATE_KEY/);
      }
    }
  }
});
