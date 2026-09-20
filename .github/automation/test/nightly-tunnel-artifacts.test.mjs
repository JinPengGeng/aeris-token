import assert from 'node:assert/strict';
import { createHash, generateKeyPairSync } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const require = createRequire(import.meta.url);
const yaml = require('js-yaml');
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const jobs = yaml.load(fs.readFileSync(path.join(root, '.github/workflows/nightly.yml'), 'utf8')).jobs;
const step = (job, name) => {
  const value = jobs[job].steps.find((entry) => entry.name === name);
  assert.ok(value, `${job}: ${name}`);
  return value.run;
};
const publicNames = ['AETHER_TUNNEL_RELEASE_KEY_ID', 'AETHER_TUNNEL_RELEASE_PUBLIC_KEY', 'AETHER_TUNNEL_RELEASE_TRUST_KEYS'];
const secretName = 'AETHER_TUNNEL_RELEASE_PRIVATE_KEY_PEM';
const tag = 'aeris-token-nightly-20260914';
const sourceSha = 'a'.repeat(40);
const archiveNames = ['amd64', 'arm64'].map((arch) => `aether-tunnel-linux-musl-${arch}.tar.gz`);
const signedNames = [...archiveNames, 'SHA256SUMS.txt', 'SHA256SUMS.txt.sig', 'release-provenance.json'];

function fixture(t) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'nightly-tunnel-test-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const env = {
    ...process.env, RUNNER_TEMP: directory, GITHUB_WORKSPACE: root,
    GITHUB_OUTPUT: path.join(directory, 'output'), GITHUB_STEP_SUMMARY: path.join(directory, 'summary'),
    RELEASE_TAG: tag, RELEASE_DATE: '20260914', SOURCE_SHA: sourceSha, SOURCE_SHORT_SHA: sourceSha.slice(0, 7),
    AETHER_TUNNEL_RELEASE_TAG: tag,
    GITHUB_WORKFLOW_REF: 'fixture/nightly.yml@refs/heads/main',
  };
  for (const name of [...publicNames, secretName]) env[name] = '';
  return { directory, env };
}

function bash(script, cwd, env) {
  return spawnSync('bash', ['-euo', 'pipefail', '-c', script], {
    cwd, env, encoding: 'utf8', timeout: 180_000, maxBuffer: 2 * 1024 * 1024,
  });
}

function succeeds(result) {
  assert.equal(result.error, undefined);
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
}

function configure(env) {
  const keys = generateKeyPairSync('ed25519');
  env.AETHER_TUNNEL_RELEASE_KEY_ID = 'ephemeral-test-key';
  env.AETHER_TUNNEL_RELEASE_PUBLIC_KEY = keys.publicKey.export({ type: 'spki', format: 'der' }).subarray(-32).toString('base64');
  env[secretName] = keys.privateKey.export({ type: 'pkcs8', format: 'pem' });
}

function createArchives(directory, env) {
  const binaryDirectory = path.join(directory, 'bin');
  fs.mkdirSync(binaryDirectory);
  fs.writeFileSync(path.join(binaryDirectory, 'aether-tunnel'), 'synthetic binary fixture', { mode: 0o755 });
  for (const name of archiveNames) {
    const result = spawnSync('tar', ['czf', path.join(directory, name), '-C', binaryDirectory, 'aether-tunnel'], { encoding: 'utf8' });
    succeeds(result);
  }
  succeeds(bash(step('tunnel-sign', 'Generate tunnel manifest'), directory, env));
}

test('nightly trust preflight distinguishes empty, partial and valid configuration', (t) => {
  const { directory, env } = fixture(t);
  const script = step('source', 'Resolve optional tunnel signing configuration');
  succeeds(bash(script, root, env));
  assert.equal(fs.readFileSync(env.GITHUB_OUTPUT, 'utf8'), 'enabled=false\n');
  assert.match(fs.readFileSync(env.GITHUB_STEP_SUMMARY, 'utf8'), /gateway nightly continues/);
  fs.unlinkSync(env.GITHUB_OUTPUT);
  env.AETHER_TUNNEL_RELEASE_KEY_ID = 'partial';
  const partial = bash(script, root, env);
  assert.notEqual(partial.status, 0);
  assert.match(partial.stderr, /trust|key|configured/i);
  assert.equal(fs.existsSync(env.GITHUB_OUTPUT), false);
  configure(env);
  succeeds(bash(script, root, env));
  assert.equal(fs.readFileSync(env.GITHUB_OUTPUT, 'utf8'), 'enabled=true\n');
  assert.deepEqual(fs.readdirSync(directory).sort(), ['output', 'summary']);
});

test('nightly signs with ephemeral Ed25519 keys and production verifier rejects bad evidence', (t) => {
  const { directory, env } = fixture(t);
  createArchives(directory, env);
  const sign = step('tunnel-sign', 'Sign nightly tunnel manifest');
  const missing = bash(sign, directory, env);
  assert.notEqual(missing.status, 0);
  assert.match(missing.stderr, /refusing to publish unsigned/);
  assert.equal(fs.existsSync(path.join(directory, 'SHA256SUMS.txt.sig')), false);
  configure(env);
  succeeds(bash(sign, directory, env));
  assert.equal(fs.readdirSync(directory).some((name) => name.startsWith('nightly-tunnel-sign.')), false);
  const manifest = fs.readFileSync(path.join(directory, 'SHA256SUMS.txt'));
  const provenance = JSON.parse(fs.readFileSync(path.join(directory, 'release-provenance.json'), 'utf8'));
  assert.equal(provenance.source_commit, sourceSha);
  assert.equal(provenance.tag, tag);
  assert.equal(provenance.manifest_sha256, createHash('sha256').update(manifest).digest('hex'));
  const verify = step('publish', 'Verify downloaded tunnel assets');
  succeeds(bash(verify, directory, env));
  fs.appendFileSync(path.join(directory, 'SHA256SUMS.txt'), '\n');
  assert.notEqual(bash(verify, directory, env).status, 0, 'changed signed manifest must fail');
  fs.writeFileSync(path.join(directory, 'SHA256SUMS.txt'), manifest);
  const archive = fs.readFileSync(path.join(directory, archiveNames[0]));
  fs.appendFileSync(path.join(directory, archiveNames[0]), 'tampered');
  assert.notEqual(bash(verify, directory, env).status, 0, 'valid signature cannot authorize changed archive');
  fs.writeFileSync(path.join(directory, archiveNames[0]), archive);
  const envelopePath = path.join(directory, 'SHA256SUMS.txt.sig');
  const envelope = fs.readFileSync(envelopePath, 'utf8');
  fs.writeFileSync(envelopePath, envelope.replace(/signature=.*/, `signature=${Buffer.alloc(64).toString('base64')}`));
  assert.notEqual(bash(verify, directory, env).status, 0, 'bad Ed25519 signature must fail');
  fs.writeFileSync(envelopePath, envelope);
  env[secretName] = generateKeyPairSync('ed25519').privateKey.export({ type: 'pkcs8', format: 'pem' });
  assert.notEqual(bash(sign, directory, env).status, 0, 'signer outside public trust must fail before upload');
  assert.equal(fs.readdirSync(directory).some((name) => name.startsWith('nightly-tunnel-sign.')), false);
});

function jobCondition(expression, needs, cancelled = false) {
  // Evaluate only the small checked-in expression subset used by these jobs.
  // Actionlint validates the real GitHub expression syntax independently.
  const expanded = expression.replace(/needs\.([\w-]+)\.(outputs\.(\w+)|result)/g, (_, job, field, output) =>
    JSON.stringify(output ? needs[job]?.outputs?.[output] ?? '' : needs[job]?.result ?? ''))
    .replaceAll('cancelled()', String(cancelled));
  assert.doesNotMatch(expanded, /needs\.|\b(?:always|success|failure)\(/);
  return Function(`return (${expanded});`)();
}

test('nightly job conditions allow intentional omission and reject failed or cancelled signing', () => {
  for (const enabled of ['false', 'true']) {
    for (const signed of ['success', 'skipped', 'failure', 'cancelled']) {
      const needs = {
        source: { result: 'success', outputs: { publish: 'true', tunnel_enabled: enabled } },
        frontend: { result: 'success' }, build: { result: 'success' }, 'tunnel-sign': { result: signed },
      };
      const expected = signed === 'success' || (enabled === 'false' && signed === 'skipped');
      assert.equal(jobCondition(jobs.package.if, needs), expected, `${enabled}/${signed}`);
      needs.package = { result: expected ? 'success' : 'skipped' };
      assert.equal(jobCondition(jobs.publish.if, needs), expected);
      assert.equal(jobCondition(jobs.package.if, needs, true), false);
      assert.equal(jobCondition(jobs.publish.if, needs, true), false);
      needs.source.result = 'failure';
      assert.equal(jobCondition(jobs.package.if, needs), false);
      assert.equal(jobCondition(jobs.publish.if, needs), false);
    }
  }
});

test('nightly outcome script accepts only intended tunnel skips and alerts on enabled signing failure', (t) => {
  const { directory, env } = fixture(t);
  const fakeBin = path.join(directory, 'fake-bin');
  fs.mkdirSync(fakeBin);
  const log = path.join(directory, 'alert-commands');
  fs.writeFileSync(path.join(fakeBin, 'gh'), `#!${process.execPath}\nrequire('fs').appendFileSync(process.env.FIXTURE_GH_LOG,JSON.stringify(process.argv.slice(2))+'\\n');\n`, { mode: 0o755 });
  Object.assign(env, {
    PATH: `${fakeBin}${path.delimiter}${env.PATH}`, FIXTURE_GH_LOG: log,
    SOURCE_RESULT: 'success', PUBLISH_GATE: 'true', FRONTEND_RESULT: 'success', BUILD_RESULT: 'success',
    PACKAGE_RESULT: 'success', PUBLISH_RESULT: 'success', GITHUB_REPOSITORY: 'fixture/repository', GITHUB_RUN_ID: '1',
  });
  const script = step('notify', 'Evaluate outcomes and alert on failure');
  for (const [enabled, tunnel, signing] of [['false', 'skipped', 'skipped'], ['true', 'success', 'success']]) {
    Object.assign(env, { TUNNEL_ENABLED: enabled, TUNNEL_RESULT: tunnel, TUNNEL_SIGN_RESULT: signing });
    succeeds(bash(script, directory, env));
    assert.equal(fs.existsSync(log), false, 'successful/intentional omitted tunnel must not create an alert');
  }
  for (const signing of ['failure', 'skipped', 'cancelled']) {
    Object.assign(env, { TUNNEL_ENABLED: 'true', TUNNEL_RESULT: 'success', TUNNEL_SIGN_RESULT: signing, PACKAGE_RESULT: 'skipped', PUBLISH_RESULT: 'skipped' });
    succeeds(bash(script, directory, env));
    const calls = fs.readFileSync(log, 'utf8').trim().split('\n').map(JSON.parse);
    assert.ok(calls.some((args) => args[0] === 'issue' && args[1] === 'create'), `${signing} signing must alert`);
    fs.unlinkSync(log);
  }
});

test('actual package and publication scripts handle signed and gateway-only asset inventories', (t) => {
  for (const enabled of [false, true]) {
    const { directory, env } = fixture(t);
    env.TUNNEL_ENABLED = String(enabled);
    env.VERSION = tag;
    env.SOURCE_REF = tag;
    for (const file of ['install.sh', 'update.sh', 'docker-compose.yml', 'docker-compose.single-node.yml', 'docker-compose.redis-durable.yml', '.env.example', 'generate_keys.sh', 'README.md', 'LICENSE']) {
      fs.copyFileSync(path.join(root, file), path.join(directory, file));
    }
    for (const arch of ['amd64', 'arm64']) {
      const location = path.join(directory, `artifacts/aether-gateway-linux-${arch}`);
      fs.mkdirSync(location, { recursive: true });
      fs.writeFileSync(path.join(location, 'aether-gateway'), 'gateway fixture');
    }
    fs.mkdirSync(path.join(directory, 'artifacts/frontend-dist'));
    fs.writeFileSync(path.join(directory, 'artifacts/frontend-dist/index.html'), '<p>fixture</p>');
    if (enabled) {
      const bundle = path.join(directory, 'artifacts/nightly-tunnel-signed');
      fs.mkdirSync(bundle);
      createArchives(bundle, env);
      configure(env);
      succeeds(bash(step('tunnel-sign', 'Sign nightly tunnel manifest'), bundle, env));
    }
    succeeds(bash(step('package', 'Build nightly packages'), directory, env));
    const assets = path.join(directory, 'release-assets');
    const expected = [`aether-${tag}-linux-amd64.tar.gz`, `aether-${tag}-linux-arm64.tar.gz`, 'install.sh', 'SHA256SUMS', ...(enabled ? signedNames : [])].sort();
    assert.deepEqual(fs.readdirSync(assets).sort(), expected);
    succeeds(bash('sha256sum --strict -c SHA256SUMS', assets, env));
    const archive = spawnSync('tar', ['tzf', path.join(assets, `aether-${tag}-linux-amd64.tar.gz`)], { encoding: 'utf8' });
    succeeds(archive);
    assert.match(archive.stdout, /\/bin\/aether-gateway/);
    fs.writeFileSync(path.join(assets, 'AETHER_NIGHTLY_PROVENANCE.sigstore.json'), '{}');
    const fakeBin = path.join(directory, 'fake-bin');
    fs.mkdirSync(fakeBin);
    const log = path.join(directory, 'gh-commands');
    fs.writeFileSync(path.join(fakeBin, 'gh'), `#!${process.execPath}\nconst fs=require('fs');const args=process.argv.slice(2);fs.appendFileSync(process.env.FIXTURE_GH_LOG,JSON.stringify(args)+'\\n');if(args[0]==='release'&&args[1]==='view')console.log(fs.readdirSync('release-assets').join('\\n'));\n`, { mode: 0o755 });
    env.PATH = `${fakeBin}${path.delimiter}${env.PATH}`;
    env.FIXTURE_GH_LOG = log;
    env.REPOSITORY = 'fixture/repository';
    env.GHCR_IMAGE = 'ghcr.io/fixture/repository';
    succeeds(bash(step('publish', 'Publish GitHub Release'), directory, env));
    const calls = fs.readFileSync(log, 'utf8').trim().split('\n').map(JSON.parse);
    assert.deepEqual(calls.map((args) => args.slice(0, 2).join(' ')), ['release create', 'release upload', 'release edit', 'release view']);
    const uploaded = calls[1].filter((arg) => arg.startsWith('release-assets/')).map((arg) => path.basename(arg)).sort();
    assert.deepEqual(uploaded, [...expected, 'AETHER_NIGHTLY_PROVENANCE.sigstore.json'].sort());
    const notes = fs.readFileSync(path.join(directory, 'nightly-release-notes.md'), 'utf8');
    assert.equal(notes.includes('Signed tunnel archives:'), enabled);
  }
});
