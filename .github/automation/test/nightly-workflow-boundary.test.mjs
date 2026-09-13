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
const workflowPath = path.join(repoRoot, '.github', 'workflows', 'nightly.yml');
const releaseWorkflowPath = path.join(repoRoot, '.github', 'workflows', 'release.yml');

function workflowSource() {
  return fs.readFileSync(workflowPath, 'utf8');
}

function workflow() {
  return yaml.load(workflowSource());
}

function actionRefs(document) {
  return Object.values(document.jobs)
    .flatMap((job) => job.steps ?? [])
    .map((step) => step.uses)
    .filter(Boolean);
}

function releaseApprovedActions() {
  const releaseDocument = yaml.load(fs.readFileSync(releaseWorkflowPath, 'utf8'));
  return new Map(
    actionRefs(releaseDocument).map((ref) => {
      const at = ref.lastIndexOf('@');
      return [ref.slice(0, at), ref.slice(at + 1)];
    }),
  );
}

function findStep(document, jobName, predicate) {
  const step = (document.jobs[jobName].steps ?? []).find(predicate);
  assert.ok(step, `expected a matching step in job ${jobName}`);
  return step;
}

test('nightly triggers are a daily cron plus manual dispatch, never a tag push', () => {
  const document = workflow();
  assert.deepEqual(document.on.schedule, [{ cron: '43 18 * * *' }]);
  assert.ok(Object.hasOwn(document.on, 'workflow_dispatch'));
  assert.equal(document.on.push, undefined, 'nightly must not trigger on pushes');
  assert.equal(document.on.pull_request, undefined, 'nightly must not trigger on pull requests');
  assert.deepEqual(document.concurrency, { group: 'nightly-main', 'cancel-in-progress': false });
  // A scheduled tag can never collide with the versioned release line: the
  // entire workflow must not reference that namespace at all.
  assert.doesNotMatch(workflowSource(), /aeris-token-v/);
});

test('nightly write permissions exist only in the publish and alert jobs', () => {
  const document = workflow();
  assert.deepEqual(document.permissions, { actions: 'read', contents: 'read' });
  assert.deepEqual(document.jobs.publish.permissions, {
    actions: 'read', attestations: 'write', contents: 'write', 'id-token': 'write', packages: 'write',
  });
  assert.deepEqual(document.jobs.notify.permissions, { actions: 'read', issues: 'write' });

  for (const [name, job] of Object.entries(document.jobs)) {
    if (name === 'publish' || name === 'notify') continue;
    assert.doesNotMatch(JSON.stringify(job.permissions ?? {}), /write/, `${name} must stay read-only`);
  }
});

test('nightly pins every action to the release-approved immutable set', () => {
  const document = workflow();
  const refs = actionRefs(document);
  assert.ok(refs.length > 0);
  const approved = releaseApprovedActions();
  for (const ref of refs) {
    const match = /^(?<action>[^@]+)@(?<sha>[0-9a-f]{40})$/.exec(ref);
    assert.ok(match, `action reference must use a full 40-character SHA: ${ref}`);
    assert.equal(approved.get(match.groups.action), match.groups.sha, `action not approved by release.yml: ${ref}`);
  }
  for (const job of Object.values(document.jobs)) {
    for (const step of job.steps ?? []) {
      if (step.uses?.startsWith('actions/checkout@')) {
        assert.equal(step.with?.['persist-credentials'], false);
        // CodeQL actions/cache-poisoning/poisonable-step: a computed checkout
        // ref reads as an untrusted checkout in cache-writable scheduled or
        // dispatched jobs. Every job checks out the run commit by default.
        assert.equal(step.with?.ref, undefined, 'checkout must not take a computed ref');
      }
    }
  }
});

test('nightly tag is date-stamped, unique per UTC day, and only built from main', () => {
  const source = workflowSource();
  const document = workflow();
  const snapshot = findStep(document, 'source', (step) => step.id === 'snapshot');

  // Uniqueness under immutable releases: the tag embeds the UTC date.
  assert.match(snapshot.run, /date -u \+%Y%m%d/);
  assert.match(snapshot.run, /aeris-token-nightly-\$\{date_stamp\}/);

  // Default-branch enforcement and the idempotent same-day skip.
  assert.match(snapshot.run, /refs\/heads\/main/);
  assert.match(snapshot.run, /git ls-remote --tags origin/);

  // The dated tag is created once and never moved: no ref patching, no asset
  // clobbering, no rolling-tag mutation of any kind.
  assert.doesNotMatch(source, /-X PATCH/);
  assert.doesNotMatch(source, /--clobber/);
  assert.doesNotMatch(source, /force=true/);

  // Every downstream job is gated on the snapshot decision.
  for (const name of ['frontend', 'build', 'package', 'publish']) {
    assert.match(document.jobs[name].if, /needs\.source\.outputs\.publish == 'true'/, `${name} must wait for the gate`);
  }
});

test('nightly quality gate pins the push-lane required check contexts', () => {
  const document = workflow();
  const snapshot = findStep(document, 'source', (step) => step.id === 'snapshot');
  for (const context of ['Rust CI / check', 'Frontend CI / check']) {
    assert.ok(snapshot.run.includes(context), `quality gate must check ${context}`);
  }
  // Automation Policy / gate must NOT be required: that context only exists on
  // pull_request/workflow_dispatch runs, never on main push commits, so
  // requiring it would deadlock the nightly gate.
  assert.doesNotMatch(snapshot.run, /required_contexts=\([^)]*"Automation Policy \/ gate"/);
  assert.match(snapshot.run, /check-runs\?per_page=100/);
  // Path-filtered pushes leave a lane's context absent on the exact snapshot;
  // the gate must walk first-parent ancestors and inherit the verdict instead
  // of deadlocking on absent.
  assert.match(snapshot.run, /context_state_walk/);
  assert.match(snapshot.run, /commits\?sha=\$\{sha\}&per_page=15/);
  assert.match(snapshot.run, /inherited from/);
  // Conservative default: publish is opt-in only after every context is green.
  assert.match(snapshot.run, /echo "publish=false"/);
  assert.match(snapshot.run, /echo "publish=true"/);
});

test('nightly publishes signed tunnel archives alongside the dated package', () => {
  const document = workflow();
  const source = workflowSource();
  const tunnel = document.jobs.tunnel;
  assert.ok(tunnel, 'nightly must build tunnel artifacts');
  assert.match(tunnel.if, /needs\.source\.outputs\.publish == 'true'/);
  assert.deepEqual(tunnel.strategy.matrix.include.map(({ name, target }) => ({ name, target })), [
    { name: 'linux-musl-amd64', target: 'x86_64-unknown-linux-musl' },
    { name: 'linux-musl-arm64', target: 'aarch64-unknown-linux-musl' },
  ]);
  assert.match(tunnelSource(tunnel), /aether-tunnel-\$\{\{ matrix\.name \}\}\.tar\.gz/);
  assert.match(source, /sha256sum aether-tunnel-linux-musl-amd64\.tar\.gz aether-tunnel-linux-musl-arm64\.tar\.gz > SHA256SUMS\.txt/);

  const signing = findStep(document, 'tunnel-sign', (step) => step.name === 'Sign nightly tunnel manifest');
  const signer = fs.readFileSync(path.join(repoRoot, '.github/workflows/scripts/sign-nightly-tunnel.sh'), 'utf8');
  assert.match(signing.run, /sign-nightly-tunnel\.sh/);
  assert.match(signer, /SHA256SUMS\.txt/);
  assert.match(signer, /verify-tunnel-release\.sh/);
  assert.match(signer, /refusing to publish unsigned/);
  assert.equal(signing.env.AETHER_TUNNEL_RELEASE_PRIVATE_KEY_PEM, '${{ secrets.AETHER_TUNNEL_RELEASE_PRIVATE_KEY_PEM }}');
  for (const input of ['AETHER_TUNNEL_RELEASE_KEY_ID', 'AETHER_TUNNEL_RELEASE_PUBLIC_KEY', 'AETHER_TUNNEL_RELEASE_TRUST_KEYS']) {
    assert.equal(signing.env[input], tunnel.env[input]);
  }
  assert.doesNotMatch(signing.run, /\$\{\{\s*(?:vars|secrets)\./);
  assert.equal(document.jobs['tunnel-sign'].environment, 'release');
  const steps = document.jobs.publish.steps;
  assert.ok(steps.findIndex((step) => step.name === 'Verify downloaded tunnel assets') < steps.findIndex((step) => step.name === 'Build and push nightly image'));
  assert.doesNotMatch(tunnelSource(tunnel), /--allow-unconfigured/);

  const packageJob = document.jobs.package;
  assert.ok(packageJob.needs.includes('tunnel-sign'));
  const publishJob = document.jobs.publish;
  assert.ok(publishJob.needs.includes('package'));
  const notifyJob = document.jobs.notify;
  assert.ok(notifyJob.needs.includes('tunnel'));
  assert.ok(notifyJob.needs.includes('tunnel-sign'));
  for (const [name, job] of Object.entries(document.jobs)) {
    for (const step of job.steps ?? []) {
      if (name === 'tunnel-sign' && step === signing) continue;
      assert.doesNotMatch(JSON.stringify(step.env ?? {}), /PRIVATE_KEY_PEM/);
    }
  }
  assert.match(source, /aether-tunnel-linux-musl-amd64\.tar\.gz/);
  assert.match(source, /aether-tunnel-linux-musl-arm64\.tar\.gz/);
  assert.match(source, /SHA256SUMS\.txt\.sig/);
  assert.match(source, /release-provenance\.json/);
});

function tunnelSource(tunnelJob) {
  return (tunnelJob.steps ?? []).map((step) => step.run ?? '').join('\n');
}

test('nightly release publish is draft-first and prerelease, never latest', () => {
  const document = workflow();
  const publish = findStep(document, 'publish', (step) => step.name === 'Publish GitHub Release');
  assert.match(publish.run, /gh release create "\$\{RELEASE_TAG\}"/);
  assert.match(publish.run, /--draft/);
  assert.match(publish.run, /gh release upload "\$\{RELEASE_TAG\}"/);
  assert.match(publish.run, /gh release edit "\$\{RELEASE_TAG\}"/);
  assert.match(publish.run, /--draft=false/);
  assert.match(publish.run, /--prerelease/);
  assert.match(publish.run, /--latest=false/);
  // Release notes link back to the exact main commit.
  assert.match(publish.run, /commit\/\$\{SOURCE_SHA\}/);
});

test('nightly cleanup only ever touches date-stamped nightly tags', () => {
  const document = workflow();
  const prune = findStep(document, 'publish', (step) => step.name === 'Prune old nightly releases');

  // The only selector is the anchored date-stamped nightly prefix.
  assert.ok(prune.run.includes('^aeris-token-nightly-[0-9]{8}$'));
  assert.doesNotMatch(prune.run, /aeris-token-v/);

  // Keeps the newest 7 and deletes release first, git tag second.
  assert.match(prune.run, /keep=7/);
  assert.match(prune.run, /NR > keep/);
  const releaseDelete = prune.run.indexOf('gh release delete');
  const tagDelete = prune.run.indexOf('git/refs/tags/');
  assert.ok(releaseDelete !== -1, 'cleanup must delete the release');
  assert.ok(tagDelete !== -1, 'cleanup must delete the git tag after the release');
  assert.ok(releaseDelete < tagDelete, 'release deletion must precede tag deletion');

  // Cleanup failure warns but never fails the round.
  assert.match(prune.run, /::warning::/);
  assert.doesNotMatch(prune.run, /set -e/);
});

test('nightly failure alert is idempotent per day', () => {
  const document = workflow();
  assert.equal(document.jobs.notify.if, 'always()');
  const alert = findStep(document, 'notify', (step) => /alert/i.test(step.name ?? ''));
  assert.match(alert.run, /\[nightly\] failure: \$\{date_stamp\}/);
  assert.match(alert.run, /gh issue list/);
  assert.match(alert.run, /gh issue create/);
  assert.match(alert.run, /gh issue comment/);
  // An exact-title match guards reuse; the raw outcome reaches the step summary.
  assert.match(alert.run, /select\(\.title == /);
  assert.match(alert.run, /GITHUB_STEP_SUMMARY/);
});
