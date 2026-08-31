import assert from 'node:assert/strict';
import { generateKeyPairSync } from 'node:crypto';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import {
  BootstrapCoordinator,
  BootstrapError,
  CapabilityGate,
  GhGitHubOperations,
  ReceiptStore,
  REPOSITORY,
  ROLE_NAMES,
  RULESET_NAMES,
  SessionTransitionLock,
  assertManifest,
  assertReceiptDocument,
  createAppJwt,
  dryRun,
  loadConfiguration,
  validateLoopbackRequest,
} from '../bootstrap.mjs';
import { probeActivation } from '../probe.mjs';

const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
const fixturePem = privateKey.export({ type: 'pkcs8', format: 'pem' });

function liveEnvironment(overrides = {}) {
  return {
    reviewer_count: 0,
    wait_timer: 0,
    deployment_branch_policy: { protected_branches: false, custom_branch_policies: true },
    branch_policies: [{ id: 1, name: 'main', type: 'branch' }],
    reviewers: [],
    protection_rule_types: [],
    raw: { name: 'fixture' },
    ...overrides,
  };
}

class FakeOperations {
  constructor(configuration, options = {}) {
    this.configuration = configuration;
    this.options = options;
    this.variables = new Map();
    this.secrets = new Map();
    this.environments = new Map(ROLE_NAMES.map((role) => [role, liveEnvironment()]));
    this.release = liveEnvironment({
      reviewer_count: 1,
      reviewers: ['maintainer'],
      protection_rule_types: ['required_reviewers'],
      raw: { name: 'release', reviewers: ['maintainer'] },
    });
    this.releaseReads = 0;
    this.rulesets = new Map();
    this.convertIndex = 0;
    this.counts = new Map();
  }

  maybeFail(method) {
    const count = (this.counts.get(method) ?? 0) + 1;
    this.counts.set(method, count);
    if (this.options.alwaysFail?.[method]) throw new BootstrapError(`${method}_fixture_failure`, method);
    const requested = this.options.fail?.[method];
    if (requested === true || requested === count) {
      delete this.options.fail[method];
      throw new BootstrapError(`${method}_fixture_failure`, method);
    }
  }

  async setVariable(name, value) {
    this.maybeFail('setVariable');
    this.variables.set(name, value);
  }

  async getVariable(name) {
    this.maybeFail('getVariable');
    if (this.options.variableMismatch === name) return 'mismatch';
    return this.variables.get(name) ?? '';
  }

  async setEnvironmentSecret(environment, name) {
    this.maybeFail('setEnvironmentSecret');
    const secrets = new Map(this.secrets.get(environment) ?? []);
    secrets.set(name, { name, updated_at: '2026-08-20T00:00:00.000Z' });
    this.secrets.set(environment, secrets);
  }

  async listEnvironmentSecretNames(environment) {
    this.maybeFail('listEnvironmentSecretNames');
    if (this.options.hideSecretFor === environment) return [];
    return [...(this.secrets.get(environment)?.keys() ?? [])];
  }

  async getEnvironmentSecret(environment, name) {
    this.maybeFail('getEnvironmentSecret');
    if (this.options.hideSecretFor === environment) throw new BootstrapError('private_key_secret_readback_missing', 'secret');
    const secret = this.secrets.get(environment)?.get(name);
    if (!secret) throw new BootstrapError('private_key_secret_readback_missing', 'secret');
    if (this.options.rotatedSecretFor === environment) return { ...secret, updated_at: '2026-08-21T00:00:00.000Z' };
    return structuredClone(secret);
  }

  async repository() {
    this.maybeFail('repository');
    if (this.options.wrongRepository) return { id: 77, full_name: 'JinPengGeng/not-aeris', owner: { id: 42, login: 'JinPengGeng' } };
    return { id: 77, full_name: REPOSITORY, owner: { id: 42, login: 'JinPengGeng' } };
  }

  async applyRuleset(payload) {
    this.maybeFail('applyRuleset');
    this.rulesets.set(payload.name, structuredClone(payload));
  }

  async readRuleset(name) {
    this.maybeFail('readRuleset');
    if (this.options.rulesetMissing === name) return undefined;
    const ruleset = structuredClone(this.rulesets.get(name));
    if (this.options.rulesetDrift === name && ruleset) ruleset.bypass_actors = [{ actor_id: 999, actor_type: 'Integration', bypass_mode: 'always' }];
    return ruleset;
  }

  async applyEnvironment(environment) {
    this.maybeFail('applyEnvironment');
    if (!this.environments.has(environment)) throw new Error('unexpected environment');
    this.environments.set(environment, liveEnvironment());
  }

  async readEnvironment(environment) {
    this.maybeFail('readEnvironment');
    if (environment === 'release') {
      this.releaseReads += 1;
      if (this.options.releaseDrift && this.releaseReads > 1) return liveEnvironment({ raw: { name: 'release', reviewers: [] } });
      if (this.options.releaseWithoutApproval) return liveEnvironment({ raw: { name: 'release', reviewers: [] } });
      return structuredClone(this.release);
    }
    if (this.options.invalidEnvironment === environment) return liveEnvironment({ reviewer_count: 1 });
    return structuredClone(this.environments.get(environment));
  }

  async convertManifest() {
    this.maybeFail('convertManifest');
    const role = ROLE_NAMES[this.convertIndex++];
    const mapping = this.configuration.mapping.roles[role];
    const override = this.options.conversion?.[role] ?? {};
    return {
      id: 1000 + this.convertIndex,
      name: mapping.app_name,
      slug: mapping.app_slug,
      owner: { id: 42, login: 'JinPengGeng' },
      pem: fixturePem,
      ...override,
    };
  }

  async verifyInstallation(appId) {
    this.maybeFail('verifyInstallation');
    if (this.options.wrongInstalledRepository) {
      return { id: 500, account_id: 42, account_login: 'JinPengGeng', repository_selection: 'selected', repository_id: 88, repository_full_name: 'JinPengGeng/other' };
    }
    return { id: 500 + appId, app_updated_at: '2026-08-20T00:00:00.000Z', account_id: 42, account_login: 'JinPengGeng', repository_selection: 'selected', repository_id: 77, repository_full_name: REPOSITORY };
  }

  async verifyAppIdentity(appId, _pem, repository, expectedApp) {
    this.maybeFail('verifyAppIdentity');
    if (this.options.reconciliationMismatch) throw new BootstrapError('app_live_identity_invalid', 'reconciliation');
    if (repository.owner.id !== 42 || expectedApp.app_slug !== this.configuration.mapping.roles[ROLE_NAMES[this.convertIndex - 1] ?? 'writer']?.app_slug) {
      throw new BootstrapError('app_live_identity_invalid', 'reconciliation');
    }
    return { app_updated_at: '2026-08-20T00:00:00.000Z', app_id: appId };
  }

  async verifyExistingInstallation(appId) {
    this.maybeFail('verifyExistingInstallation');
    if (this.options.revokedInstallationForAppId === appId) throw new BootstrapError('verified_installation_count_invalid', 'verified_readback');
    return {
      id: 500 + appId,
      app_updated_at: this.options.rotatedAppKeyForAppId === appId ? '2026-08-21T00:00:00.000Z' : '2026-08-20T00:00:00.000Z',
      account_id: 42,
      account_login: 'JinPengGeng',
      repository_selection: 'selected',
      repository_id: 77,
      repository_full_name: REPOSITORY,
    };
  }
}

async function harness(options = {}) {
  const configuration = await loadConfiguration();
  const root = await mkdtemp(path.join(os.tmpdir(), 'aeris-bootstrap-test-'));
  const store = new ReceiptStore({ configuration, root });
  const operations = new FakeOperations(configuration, options);
  const coordinator = new BootstrapCoordinator({ configuration, operations, store });
  return { configuration, coordinator, operations, root, store };
}

async function configureCurrent(coordinator) {
  const pending = coordinator.begin();
  return coordinator.consume({ state: pending.state, code: 'abcdefgh1234' });
}

async function completeAll(coordinator) {
  for (const role of ROLE_NAMES) {
    const result = await configureCurrent(coordinator);
    assert.equal(result.role, role);
    assert.match(result.install_url, new RegExp(`/apps/aeris-${role}/installations/new/permissions\\?target_id=42$`));
    const verified = await coordinator.verifyCurrent();
    assert.equal(verified.role, role);
  }
  assert.ok(RULESET_NAMES.every((name) => coordinator.document.rulesets[name].verified));
}

function response(value, { status = 200, link = null } = {}) {
  const headers = { 'content-type': 'application/json' };
  if (link) headers.link = link;
  return new Response(status === 204 ? null : JSON.stringify(value), { status, headers });
}

function installationFetchQueue(items, requests = []) {
  const queue = [...items];
  return async (url, init = {}) => {
    assert.ok(queue.length > 0, 'unexpected fetch');
    requests.push({ url, init });
    return queue.shift();
  };
}

test('dry-run validates four private webhook-free least-privilege manifests', async () => {
  const result = await dryRun();
  assert.deepEqual(result.map((entry) => entry.role), ROLE_NAMES);
  assert.equal(result.find((entry) => entry.role === 'merger').permissions.checks, 'write');
  assert.equal(result.find((entry) => entry.role === 'merger').permissions.pull_requests, 'write');
  const configuration = await loadConfiguration();
  for (const role of ROLE_NAMES) {
    const manifest = configuration.manifests[role];
    assert.equal(manifest.public, false);
    assert.equal(manifest.hook_attributes.active, false);
    assert.deepEqual(manifest.default_events, []);
  }
});

test('manifest validator rejects every OAuth, webhook, callback, and unknown extra field', async () => {
  const { mapping, manifests } = await loadConfiguration();
  assert.throws(() => assertManifest('writer', { ...manifests.writer, hook_attributes: { active: true } }, mapping.roles.writer), /manifest_identity/);
  for (const field of ['redirect_url', 'callback_urls', 'setup_url', 'setup_on_update', 'request_oauth_on_install', 'webhook_url', 'webhook_secret', 'unknown']) {
    assert.throws(() => assertManifest('writer', { ...manifests.writer, [field]: 'unexpected' }, mapping.roles.writer), /manifest_keys_invalid/);
  }
  assert.throws(() => assertManifest('writer', { ...manifests.writer, hook_attributes: { active: false, url: 'https://example.test' } }, mapping.roles.writer), /hook_attributes_keys_invalid/);
  assert.throws(() => assertManifest('writer', { ...manifests.writer, default_permissions: {} }, mapping.roles.writer), /permissions/);
});

test('full staged path verifies four distinct Apps and emits strict evidence', async () => {
  const { coordinator, root, configuration } = await harness();
  await coordinator.prepare();
  await completeAll(coordinator);
  assert.equal(coordinator.document.status, 'verified');
  assertReceiptDocument(coordinator.document, configuration);
  assert.equal(new Set(ROLE_NAMES.map((role) => coordinator.document.roles[role].app_id)).size, 4);
  for (const role of ROLE_NAMES) {
    const receipt = coordinator.document.roles[role];
    assert.equal(receipt.switch.value, 'false');
    assert.equal(receipt.switch.readback, true);
    assert.equal(receipt.private_key_secret.present, true);
    assert.equal(receipt.installation.repository_full_name, REPOSITORY);
    assert.equal(receipt.environment.evidence.reviewer_count, 0);
  }
  const saved = JSON.parse(await readFile(path.join(root, 'latest.json'), 'utf8'));
  assert.equal(saved.status, 'verified');
  assert.doesNotMatch(JSON.stringify(saved), /BEGIN PRIVATE KEY|Bearer |github_pat_|eyJ/);
});

test('only one callback state may be outstanding and replay is rejected', async () => {
  const { coordinator } = await harness();
  await coordinator.prepare();
  const pending = coordinator.begin();
  assert.throws(() => coordinator.begin(), /already_outstanding/);
  await assert.rejects(coordinator.consume({ state: 'wrong', code: 'abcdefgh1234' }), /callback_state_invalid/);
  await coordinator.consume({ state: pending.state, code: 'abcdefgh1234' });
  await assert.rejects(coordinator.consume({ state: pending.state, code: 'abcdefgh1234' }), /callback_state_invalid/);
});

test('session transitions fail closed while a conversion is in flight', async () => {
  const { coordinator, operations, store } = await harness();
  await coordinator.prepare();
  const pending = coordinator.begin();
  const lock = new SessionTransitionLock();
  let release;
  let conversionStarted;
  const started = new Promise((resolve) => { conversionStarted = resolve; });
  operations.convertManifest = () => new Promise((resolve) => {
    release = resolve;
    conversionStarted();
  });
  const first = lock.run(() => coordinator.consume({ state: pending.state, code: 'abcdefgh1234' }));
  await started;
  const duringConversion = await store.load();
  assert.equal(duringConversion.roles.writer.status, 'conversion_started');
  assert.equal(coordinator.canRestartCurrent(), false);
  await assert.rejects(lock.run(() => coordinator.consume({ state: pending.state, code: 'abcdefgh1234' })), /bootstrap_transition_busy/);
  release({
    id: 1001,
    name: 'aeris-writer',
    slug: 'aeris-writer',
    owner: { id: 42, login: 'JinPengGeng' },
    pem: fixturePem,
  });
  await first;
  await assert.doesNotReject(lock.run(async () => {}));
});

test('expired callback state is cleared for a safe retry', async () => {
  let now = 1000;
  const { configuration, operations, store } = await harness();
  const coordinator = new BootstrapCoordinator({ configuration, operations, store, now: () => now });
  await coordinator.prepare();
  const expired = coordinator.begin(10);
  now = 1011;
  await assert.rejects(coordinator.consume({ state: expired.state, code: 'abcdefgh1234' }), /callback_state_expired/);
  assert.doesNotThrow(() => coordinator.begin());
});

test('conversion validates owner, slug, App ID uniqueness, and RSA key before storage', async () => {
  for (const [field, override, expected] of [
    ['owner', { owner: { id: 42, login: 'attacker' } }, 'conversion_owner_invalid'],
    ['slug', { slug: 'wrong-app' }, 'conversion_slug_invalid'],
    ['pem', { pem: 'not a key' }, 'conversion_private_key_invalid'],
  ]) {
    const { coordinator, operations } = await harness({ conversion: { writer: override } });
    await coordinator.prepare();
    await assert.rejects(configureCurrent(coordinator), new RegExp(expected), field);
    assert.equal(operations.secrets.size, 0);
    assert.equal(coordinator.document.roles.writer.switch.value, 'false');
    assert.equal(coordinator.document.status, 'partial_failed');
  }

  const { coordinator, operations } = await harness();
  await coordinator.prepare();
  await configureCurrent(coordinator);
  await coordinator.verifyCurrent();
  operations.options.conversion = { policy: { id: coordinator.document.roles.writer.app_id } };
  await assert.rejects(configureCurrent(coordinator), /conversion_app_id_invalid/);
});

test('all mutation and readback classes fail closed with a partial_failed receipt', async () => {
  for (const method of ['setVariable', 'getVariable', 'applyEnvironment', 'readEnvironment', 'repository', 'applyRuleset', 'readRuleset']) {
    const { coordinator } = await harness({ fail: { [method]: true } });
    await assert.rejects(coordinator.prepare(), /fixture_failure/);
    assert.equal(coordinator.document.status, 'partial_failed');
    assert.ok(ROLE_NAMES.every((role) => coordinator.document.roles[role].switch.value !== 'true'));
  }

  for (const method of ['convertManifest', 'setEnvironmentSecret', 'getEnvironmentSecret', 'verifyInstallation']) {
    const { coordinator } = await harness({ fail: { [method]: true } });
    await coordinator.prepare();
    if (method === 'verifyInstallation') {
      await configureCurrent(coordinator);
      await assert.rejects(coordinator.verifyCurrent(), /fixture_failure/);
    } else {
      await assert.rejects(configureCurrent(coordinator), /fixture_failure/);
    }
    assert.equal(coordinator.document.status, 'partial_failed');
    assert.ok(ROLE_NAMES.every((role) => coordinator.document.roles[role].switch.value !== 'true'));
  }
});

test('conversion is durably checkpointed before configuration and restart requires safe adoption', async () => {
  const { coordinator, operations, store, configuration } = await harness({ fail: { setEnvironmentSecret: true } });
  await coordinator.prepare();
  await assert.rejects(configureCurrent(coordinator), /setEnvironmentSecret_fixture_failure/);
  const checkpoint = await store.load();
  assert.equal(checkpoint.roles.writer.app_id, 1001);
  assert.equal(checkpoint.roles.writer.app_slug, 'aeris-writer');
  assert.equal(coordinator.canRecover(), true);
  const recovered = await coordinator.recover();
  assert.equal(recovered.role, 'writer');
  await coordinator.verifyCurrent();

  operations.options.fail = { setEnvironmentSecret: true };
  await assert.rejects(configureCurrent(coordinator), /setEnvironmentSecret_fixture_failure/);
  const restarted = new BootstrapCoordinator({ configuration, operations, store });
  await restarted.prepare();
  assert.equal(restarted.document.roles.policy.status, 'adoption_required');
  assert.throws(() => restarted.begin(), /checkpoint_requires_adoption/);
  await assert.rejects(restarted.adoptCurrent({ appId: 9999, appSlug: 'aeris-policy', pem: fixturePem }), /adoption_checkpoint_mismatch/);

  const secondRestart = new BootstrapCoordinator({ configuration, operations, store });
  await secondRestart.prepare();
  const adopted = await secondRestart.adoptCurrent({ appId: checkpoint.roles.writer.app_id + 1, appSlug: 'aeris-policy', pem: fixturePem });
  assert.equal(adopted.role, 'policy');
});

test('crash immediately after conversion checkpoint cannot create a duplicate App', async () => {
  const { coordinator, operations, store, configuration } = await harness({ fail: { setEnvironmentSecret: true } });
  await coordinator.prepare();
  await assert.rejects(configureCurrent(coordinator), /setEnvironmentSecret_fixture_failure/);
  assert.equal(operations.convertIndex, 1);
  const restarted = new BootstrapCoordinator({ configuration, operations, store });
  await restarted.prepare();
  assert.throws(() => restarted.begin(), /checkpoint_requires_adoption/);
  assert.equal(operations.convertIndex, 1);
});

test('result-unknown conversion is checkpointed before the API call and requires authenticated reconciliation', async () => {
  const { coordinator, operations, store, configuration } = await harness({ fail: { convertManifest: true } });
  await coordinator.prepare();
  await assert.rejects(configureCurrent(coordinator), /convertManifest_fixture_failure/);
  const ambiguous = await store.load();
  assert.equal(ambiguous.roles.writer.status, 'conversion_started');
  assert.equal(ambiguous.roles.writer.app_id, null);
  assert.equal(ambiguous.roles.writer.app_slug, 'aeris-writer');

  const restarted = new BootstrapCoordinator({ configuration, operations, store });
  await restarted.prepare();
  assert.equal(restarted.document.roles.writer.status, 'reconciliation_required');
  assert.equal(restarted.canRestartCurrent(), false);
  assert.throws(() => restarted.begin(), /checkpoint_requires_adoption/);

  operations.options.reconciliationMismatch = true;
  await assert.rejects(restarted.adoptCurrent({ appId: 1001, appSlug: 'aeris-writer', pem: fixturePem }), /app_live_identity_invalid/);
  assert.equal(restarted.canRecover(), false);
  assert.equal(operations.secrets.size, 0);
  const stillAmbiguous = await store.load();
  assert.equal(stillAmbiguous.roles.writer.status, 'reconciliation_required');
  assert.equal(stillAmbiguous.roles.writer.app_id, null);

  delete operations.options.reconciliationMismatch;
  const secondRestart = new BootstrapCoordinator({ configuration, operations, store });
  await secondRestart.prepare();
  const reconciled = await secondRestart.adoptCurrent({ appId: 1001, appSlug: 'aeris-writer', pem: fixturePem });
  assert.equal(reconciled.role, 'writer');
  assert.equal(secondRestart.document.roles.writer.app_id, 1001);
  assert.equal(operations.convertIndex, 0);
});

test('resume preserves the durable App checkpoint when fail-closed disabling fails', async () => {
  const { coordinator, operations, store, configuration } = await harness({ fail: { setEnvironmentSecret: true } });
  await coordinator.prepare();
  await assert.rejects(configureCurrent(coordinator), /setEnvironmentSecret_fixture_failure/);
  const before = await store.load();
  assert.equal(before.roles.writer.app_id, 1001);

  operations.options.fail = { setVariable: true };
  const restarted = new BootstrapCoordinator({ configuration, operations, store });
  await assert.rejects(restarted.prepare(), /setVariable_fixture_failure/);
  const after = await store.load();
  assert.equal(after.roles.writer.app_id, before.roles.writer.app_id);
  assert.equal(after.roles.writer.app_slug, before.roles.writer.app_slug);
  assert.equal(operations.convertIndex, 1);
});

test('persistent fail-closed disabling errors remain writable receipt evidence', async () => {
  const { coordinator, operations, store } = await harness();
  await coordinator.prepare();
  await configureCurrent(coordinator);
  await coordinator.verifyCurrent();

  operations.options.alwaysFail = { setVariable: true };
  await assert.rejects(configureCurrent(coordinator), /setVariable_fixture_failure/);
  const receipt = await store.load();
  assert.equal(receipt.status, 'partial_failed');
  assert.equal(receipt.roles.writer.status, 'verified');
  assert.equal(receipt.roles.writer.switch.value, 'false');
  assert.equal(receipt.roles.writer.switch.readback, true);
  assert.deepEqual(receipt.disable_failures.map((entry) => entry.role), ROLE_NAMES);
  assert.ok(receipt.disable_failures.every((entry) => entry.code === 'set_variable_fixture_failure'));
});

test('partial resume live-verifies every prior verified role before mutation', async () => {
  const { coordinator, operations, store, configuration } = await harness();
  await coordinator.prepare();
  await configureCurrent(coordinator);
  await coordinator.verifyCurrent();
  operations.options.fail = { setEnvironmentSecret: true };
  await assert.rejects(configureCurrent(coordinator), /setEnvironmentSecret_fixture_failure/);
  const original = await readFile(store.output, 'utf8');

  const forged = JSON.parse(original);
  forged.roles.writer.app_id = 9001;
  forged.roles.writer.variables.app_id.value = '9001';
  forged.roles.writer.installation.id = 9501;
  assert.doesNotThrow(() => assertReceiptDocument(forged, configuration));
  const forgedBytes = `${JSON.stringify(forged, null, 2)}\n`;
  await writeFile(store.output, forgedBytes, 'utf8');
  const forgedResume = new BootstrapCoordinator({ configuration, operations, store });
  await assert.rejects(forgedResume.prepare(), /writer_verified_app_id_live_drift/);
  assert.equal(await readFile(store.output, 'utf8'), forgedBytes);

  await writeFile(store.output, original, 'utf8');
  operations.options.rotatedSecretFor = 'writer';
  const rotatedResume = new BootstrapCoordinator({ configuration, operations, store });
  await assert.rejects(rotatedResume.prepare(), /writer_verified_secret_rotated_or_revoked/);
  assert.equal(await readFile(store.output, 'utf8'), original);
});

test('receipt store holds an exclusive fail-closed apply lock', async () => {
  const { configuration, root, store } = await harness();
  const competing = new ReceiptStore({ configuration, root });
  await store.acquireExclusiveLock();
  try {
    await assert.rejects(competing.acquireExclusiveLock(), /bootstrap_lock_unavailable/);
  } finally {
    await store.releaseExclusiveLock();
  }
  await competing.acquireExclusiveLock();
  await competing.releaseExclusiveLock();
});

test('wrong repository, invalid Environment, and release drift are rejected', async () => {
  const wrongRepo = await harness({ wrongRepository: true });
  await assert.rejects(wrongRepo.coordinator.prepare(), /repository_identity_invalid/);

  const invalidEnvironment = await harness({ invalidEnvironment: 'policy' });
  await assert.rejects(invalidEnvironment.coordinator.prepare(), /policy_environment_protection_invalid/);

  const wrongInstall = await harness({ wrongInstalledRepository: true });
  await wrongInstall.coordinator.prepare();
  await configureCurrent(wrongInstall.coordinator);
  await assert.rejects(wrongInstall.coordinator.verifyCurrent(), /installation_evidence_invalid/);

  const drift = await harness({ releaseDrift: true });
  await drift.coordinator.prepare();
  await configureCurrent(drift.coordinator);
  await assert.rejects(drift.coordinator.verifyCurrent(), /release_environment_drift/);
  assert.equal(drift.coordinator.document.release.unchanged, false);
});

test('release must start with a maintainer approval baseline before any mutation', async () => {
  const blocked = await harness({ releaseWithoutApproval: true });
  await assert.rejects(blocked.coordinator.prepare(), /release_manual_approval_missing/);
  assert.equal(blocked.operations.variables.size, 0);
  assert.equal(blocked.operations.rulesets.size, 0);
  assert.equal(blocked.operations.counts.get('setVariable') ?? 0, 0);
});

test('Writer and Merger rulesets are separate, exact, and live drift blocks finalize', async () => {
  const { coordinator, operations } = await harness();
  await coordinator.prepare();
  const writer = operations.rulesets.get('aeris-writer-agent-branches');
  const merger = operations.rulesets.get('aeris-merger-generation-tags');
  assert.equal(writer.target, 'branch');
  assert.deepEqual(writer.conditions.ref_name.include, ['refs/heads/agent/**']);
  assert.deepEqual(writer.rules.map((rule) => rule.type).sort(), ['deletion', 'non_fast_forward']);
  assert.equal(merger.target, 'tag');
  assert.deepEqual(merger.conditions.ref_name.include, ['refs/tags/aeris-merger-attempt-*']);
  assert.deepEqual(merger.rules.map((rule) => rule.type).sort(), ['deletion', 'non_fast_forward', 'update']);
  assert.deepEqual(merger.rules.find((rule) => rule.type === 'update').parameters, { update_allows_fetch_and_merge: false });
  assert.equal(merger.rules.some((rule) => rule.type === 'creation'), false);
  assert.deepEqual(merger.bypass_actors, []);

  await configureCurrent(coordinator);
  operations.options.rulesetDrift = 'aeris-merger-generation-tags';
  await assert.rejects(coordinator.verifyCurrent(), /merger_generation_tags_ruleset_live_drift/);
  assert.equal(coordinator.document.status, 'partial_failed');
  assert.equal(coordinator.document.rulesets.merger_generation_tags.verified, false);

  const missing = await harness();
  await missing.coordinator.prepare();
  await configureCurrent(missing.coordinator);
  missing.operations.options.rulesetMissing = 'aeris-merger-generation-tags';
  await assert.rejects(missing.coordinator.verifyCurrent(), /merger_generation_tags_ruleset_live_drift/);
});

test('App JWT rejects malformed and non-RSA keys', () => {
  assert.throws(() => createAppJwt(1, 'not a key'), /jwt_private_key_invalid/);
  const { privateKey: ecKey } = generateKeyPairSync('ec', { namedCurve: 'P-256' });
  assert.throws(() => createAppJwt(1, ecKey.export({ type: 'pkcs8', format: 'pem' })), /jwt_private_key_not_rsa/);
  const jwt = createAppJwt(123, fixturePem, Date.UTC(2026, 0, 1));
  assert.equal(jwt.split('.').length, 3);
});

test('REST operations have a bounded fail-closed timeout', async () => {
  const fetchImpl = async (_url, init) => new Promise((_resolve, reject) => {
    init.signal.addEventListener('abort', () => reject(init.signal.reason), { once: true });
  });
  const operations = new GhGitHubOperations({ fetchImpl, timeoutMs: 5 });
  await assert.rejects(operations.convertManifest('abcdefgh1234'), /manifest_conversion_transport_failed/);
});

test('App installation verification scopes and revokes tokens on success and failure', async () => {
  const repo = { id: 77, full_name: REPOSITORY, owner: { id: 42, login: 'JinPengGeng' } };
  const expectedApp = { app_slug: 'aeris-merger', app_name: 'aeris-merger', permissions: { metadata: 'read', contents: 'write', pull_requests: 'write', checks: 'write' } };
  const app = { id: 123, slug: expectedApp.app_slug, name: expectedApp.app_name, updated_at: '2026-08-20T00:00:00.000Z', owner: { id: 42, login: 'JinPengGeng' }, permissions: expectedApp.permissions, events: [] };
  const install = { id: 5, app_id: 123, target_type: 'User', repository_selection: 'selected', account: { id: 42, login: 'JinPengGeng' }, permissions: expectedApp.permissions, events: [] };

  const extraInstallations = new GhGitHubOperations({ fetchImpl: installationFetchQueue([
    response(app),
    response([install, { ...install, id: 6 }]),
  ]) });
  await assert.rejects(extraInstallations.verifyInstallation(123, fixturePem, repo, expectedApp), /installation_count_invalid/);

  const scopedToken = { token: 'installation-token-fixture-value', repository_selection: 'selected', permissions: expectedApp.permissions };
  const extraRequests = [];
  const extraRepositories = new GhGitHubOperations({ fetchImpl: installationFetchQueue([
    response(app),
    response([install]),
    response(scopedToken),
    response({ total_count: 2, repositories: [{ id: 77, full_name: REPOSITORY }, { id: 78, full_name: 'JinPengGeng/other' }] }),
    response(null, { status: 204 }),
  ], extraRequests) });
  await assert.rejects(extraRepositories.verifyInstallation(123, fixturePem, repo, expectedApp), /installation_repository_count_invalid/);
  assert.equal(extraRequests.at(-1).url, 'https://api.github.com/installation/token');
  assert.equal(extraRequests.at(-1).init.method, 'DELETE');
  assert.deepEqual(JSON.parse(extraRequests[2].init.body), { repository_ids: [77], permissions: expectedApp.permissions });

  const wrongRepository = new GhGitHubOperations({ fetchImpl: installationFetchQueue([
    response(app),
    response([install]),
    response(scopedToken),
    response({ total_count: 1, repositories: [{ id: 78, full_name: 'JinPengGeng/other' }] }),
    response(null, { status: 204 }),
  ]) });
  await assert.rejects(wrongRepository.verifyInstallation(123, fixturePem, repo, expectedApp), /installation_repository_identity_invalid/);

  const wrongPermissions = new GhGitHubOperations({ fetchImpl: installationFetchQueue([
    response({ ...app, permissions: { metadata: 'read', contents: 'write', issues: 'write' } }),
  ]) });
  await assert.rejects(wrongPermissions.verifyInstallation(123, fixturePem, repo, expectedApp), /app_live_permissions_invalid/);

  const validRequests = [];
  const valid = new GhGitHubOperations({ fetchImpl: installationFetchQueue([
    response(app),
    response([install]),
    response(scopedToken),
    response({ total_count: 1, repositories: [{ id: 77, full_name: REPOSITORY }] }),
    response(null, { status: 204 }),
  ], validRequests) });
  assert.deepEqual(await valid.verifyInstallation(123, fixturePem, repo, expectedApp), {
    id: 5,
    app_updated_at: '2026-08-20T00:00:00.000Z',
    account_id: 42,
    account_login: 'JinPengGeng',
    repository_selection: 'selected',
    repository_id: 77,
    repository_full_name: REPOSITORY,
  });
  assert.equal(validRequests.at(-1).url, 'https://api.github.com/installation/token');

  const scopeMismatch = new GhGitHubOperations({ fetchImpl: installationFetchQueue([
    response(app),
    response([install]),
    response({ ...scopedToken, permissions: { metadata: 'read' } }),
    response(null, { status: 204 }),
  ]) });
  await assert.rejects(scopeMismatch.verifyInstallation(123, fixturePem, repo, expectedApp), /installation_token_scope_invalid/);

  const revokeFailure = new GhGitHubOperations({ fetchImpl: installationFetchQueue([
    response(app),
    response([install]),
    response(scopedToken),
    response({ total_count: 1, repositories: [{ id: 77, full_name: REPOSITORY }] }),
    response({ message: 'failure' }, { status: 500 }),
  ]) });
  await assert.rejects(revokeFailure.verifyInstallation(123, fixturePem, repo, expectedApp), /installation_token_revoke_http_500/);
});

test('activation probe proves the PEM and global single-installation scope', async () => {
  const expectedApp = { app_slug: 'aeris-merger', app_name: 'aeris-merger', permissions: { metadata: 'read', contents: 'write', pull_requests: 'write', checks: 'write' } };
  const app = { id: 123, slug: expectedApp.app_slug, name: expectedApp.app_name, updated_at: '2026-08-20T00:00:00.000Z', owner: { id: 42, login: 'JinPengGeng' }, permissions: expectedApp.permissions, events: [] };
  const install = { id: 5, app_id: 123, target_type: 'User', repository_selection: 'selected', account: { id: 42, login: 'JinPengGeng' }, permissions: expectedApp.permissions, events: [] };
  const token = { token: 'installation-token-fixture-value', repository_selection: 'selected', permissions: expectedApp.permissions };
  const requests = [];
  const operations = new GhGitHubOperations({ fetchImpl: installationFetchQueue([
    response(app),
    response([install]),
    response(token),
    response({ total_count: 1, repositories: [{ id: 77, full_name: REPOSITORY }] }),
    response(null, { status: 204 }),
  ], requests) });
  assert.deepEqual(await operations.probeInstallation(123, fixturePem, expectedApp), {
    app_id: 123,
    app_slug: 'aeris-merger',
    app_updated_at: '2026-08-20T00:00:00.000Z',
    installation_id: 5,
    repository_id: 77,
    repository_full_name: REPOSITORY,
  });
  assert.deepEqual(JSON.parse(requests[2].init.body), { permissions: expectedApp.permissions });
  assert.equal(requests.at(-1).url, 'https://api.github.com/installation/token');
  assert.equal(requests.at(-1).init.method, 'DELETE');

  const extra = new GhGitHubOperations({ fetchImpl: installationFetchQueue([
    response(app),
    response([install, { ...install, id: 6 }]),
  ]) });
  await assert.rejects(extra.probeInstallation(123, fixturePem, expectedApp), /app_probe_installation_count_invalid/);

  const mismatchRequests = [];
  const repositoryMismatch = new GhGitHubOperations({ fetchImpl: installationFetchQueue([
    response(app),
    response([install]),
    response(token),
    response({ total_count: 1, repositories: [{ id: 78, full_name: 'JinPengGeng/other' }] }),
    response(null, { status: 204 }),
  ], mismatchRequests) });
  await assert.rejects(repositoryMismatch.probeInstallation(123, fixturePem, expectedApp), /app_probe_repository_identity_invalid/);
  assert.equal(mismatchRequests.at(-1).url, 'https://api.github.com/installation/token');
  assert.equal(mismatchRequests.at(-1).init.method, 'DELETE');

  const revokeFailure = new GhGitHubOperations({ fetchImpl: installationFetchQueue([
    response(app),
    response([install]),
    response(token),
    response({ total_count: 1, repositories: [{ id: 77, full_name: REPOSITORY }] }),
    response({ message: 'failure' }, { status: 500 }),
  ]) });
  await assert.rejects(revokeFailure.probeInstallation(123, fixturePem, expectedApp), /app_probe_token_revoke_http_500/);

  const configuration = await loadConfiguration();
  const evidence = await probeActivation({
    role: 'merger',
    appId: 123,
    pem: fixturePem,
    configuration,
    operations: { probeInstallation: async () => ({ app_id: 123, app_slug: 'aeris-merger' }) },
  });
  assert.deepEqual(evidence, { role: 'merger', app_id: 123, app_slug: 'aeris-merger' });
});

test('readback mismatches and missing secret-name evidence fail closed', async () => {
  const slugMismatch = await harness({ variableMismatch: 'AERIS_WRITER_APP_SLUG' });
  await slugMismatch.coordinator.prepare();
  await assert.rejects(configureCurrent(slugMismatch.coordinator), /app_slug_readback_mismatch/);
  assert.equal(slugMismatch.coordinator.document.status, 'partial_failed');

  const missingSecret = await harness({ hideSecretFor: 'writer' });
  await missingSecret.coordinator.prepare();
  await assert.rejects(configureCurrent(missingSecret.coordinator), /private_key_secret_readback_missing/);
  assert.equal(missingSecret.coordinator.document.status, 'partial_failed');
});

test('capabilities are one-time and loopback Host, Origin, and Fetch-Site are enforced', () => {
  const gate = new CapabilityGate({ random: () => Buffer.alloc(32, 7), now: () => 1000 });
  const capability = gate.issue('init');
  gate.consume(capability, 'init');
  assert.throws(() => gate.consume(capability, 'init'), /capability_invalid/);

  const request = { headers: { host: '127.0.0.1:8791', origin: 'http://127.0.0.1:8791', 'sec-fetch-site': 'same-origin' } };
  assert.doesNotThrow(() => validateLoopbackRequest(request, '127.0.0.1:8791', 'local'));
  assert.throws(() => validateLoopbackRequest({ headers: { ...request.headers, host: 'evil.test' } }, '127.0.0.1:8791', 'local'), /host_invalid/);
  assert.throws(() => validateLoopbackRequest({ headers: { ...request.headers, origin: 'https://evil.test' } }, '127.0.0.1:8791', 'local'), /origin_invalid/);
  assert.throws(() => validateLoopbackRequest({ headers: { ...request.headers, 'sec-fetch-site': 'cross-site' } }, '127.0.0.1:8791', 'local'), /fetch_site_invalid/);
});

test('receipt validation rejects extra fields and secret-shaped material', async () => {
  const { coordinator, configuration } = await harness();
  await coordinator.prepare();
  const extra = structuredClone(coordinator.document);
  extra.roles.writer.unexpected = true;
  assert.throws(() => assertReceiptDocument(extra, configuration), /keys_invalid/);
  const secret = structuredClone(coordinator.document);
  secret.failure = { code: 'github_pat_abcdefghijklmnopqrstuvwxyz123456', stage: 'receipt', role: null };
  secret.status = 'partial_failed';
  assert.throws(() => assertReceiptDocument(secret, configuration), /failure_invalid|secret_material/);

  await completeAll(coordinator);
  const missingProbe = structuredClone(coordinator.document);
  missingProbe.roles.writer.pem_probe = { verified: false, app_updated_at: null };
  assert.throws(() => assertReceiptDocument(missingProbe, configuration), /writer_verified_pem_probe_missing/);
  const outOfOrder = structuredClone(coordinator.document);
  outOfOrder.status = 'partial_failed';
  outOfOrder.roles.writer.status = 'partial_failed';
  assert.throws(() => assertReceiptDocument(outOfOrder, configuration), /receipt_verified_roles_not_prefix/);
});

test('verified rerun requires bounded live readback and preserves receipt bytes', async () => {
  const { coordinator, operations, store, configuration } = await harness();
  await coordinator.prepare();
  await completeAll(coordinator);
  const before = await readFile(store.output, 'utf8');
  const mutationMethods = ['setVariable', 'setEnvironmentSecret', 'applyEnvironment', 'applyRuleset', 'convertManifest'];
  const mutationsBefore = Object.fromEntries(mutationMethods.map((method) => [method, operations.counts.get(method) ?? 0]));
  const rerun = new BootstrapCoordinator({ configuration, operations, store });
  const result = await rerun.prepare();
  assert.equal(result.status, 'verified');
  assert.equal(rerun.verifiedNoop, true);
  assert.equal(await readFile(store.output, 'utf8'), before);
  assert.deepEqual(Object.fromEntries(mutationMethods.map((method) => [method, operations.counts.get(method) ?? 0])), mutationsBefore);
  assert.ok((operations.counts.get('repository') ?? 0) > 1);
  const countsAfterReadback = new Map(operations.counts);

  const drifted = structuredClone(configuration);
  drifted.mapping.roles.merger.permissions.pull_requests = 'read';
  const driftStore = new ReceiptStore({ configuration: drifted, root: store.root });
  await assert.rejects(driftStore.load(), /receipt_header_invalid/);
  const driftRerun = new BootstrapCoordinator({ configuration: drifted, operations, store: driftStore });
  await assert.rejects(driftRerun.prepare(), /receipt_header_invalid/);
  assert.equal(await readFile(store.output, 'utf8'), before);
  assert.deepEqual(operations.counts, countsAfterReadback);

  const forged = JSON.parse(before);
  forged.roles.merger.variables.app_slug.value = 'aeris-policy';
  assert.throws(() => assertReceiptDocument(forged, configuration), /verified_variable_value_invalid/);
});

test('self-consistent forged mismatched-live and replayed receipts cannot bypass live trust', async () => {
  const { coordinator, operations, store, configuration } = await harness();
  await coordinator.prepare();
  await completeAll(coordinator);
  const original = await readFile(store.output, 'utf8');
  const forged = JSON.parse(original);
  forged.roles.writer.app_id = 9001;
  forged.roles.writer.variables.app_id.value = '9001';
  forged.roles.writer.installation.id = 9501;
  assert.doesNotThrow(() => assertReceiptDocument(forged, configuration));
  const forgedBytes = `${JSON.stringify(forged, null, 2)}\n`;
  await writeFile(store.output, forgedBytes, 'utf8');
  const forgedRerun = new BootstrapCoordinator({ configuration, operations, store });
  await assert.rejects(forgedRerun.prepare(), /writer_verified_app_id_live_drift/);
  assert.equal(await readFile(store.output, 'utf8'), forgedBytes);

  await writeFile(store.output, original, 'utf8');
  operations.options.rotatedSecretFor = 'writer';
  const rotated = new BootstrapCoordinator({ configuration, operations, store });
  await assert.rejects(rotated.prepare(), /writer_verified_secret_rotated_or_revoked/);
  assert.equal(await readFile(store.output, 'utf8'), original);
  delete operations.options.rotatedSecretFor;

  operations.options.rotatedAppKeyForAppId = coordinator.document.roles.writer.app_id;
  const keyRevoked = new BootstrapCoordinator({ configuration, operations, store });
  await assert.rejects(keyRevoked.prepare(), /writer_verified_installation_live_drift/);
  assert.equal(await readFile(store.output, 'utf8'), original);
  delete operations.options.rotatedAppKeyForAppId;

  operations.options.revokedInstallationForAppId = coordinator.document.roles.writer.app_id;
  const revoked = new BootstrapCoordinator({ configuration, operations, store });
  await assert.rejects(revoked.prepare(), /verified_installation_count_invalid/);
  assert.equal(await readFile(store.output, 'utf8'), original);
  delete operations.options.revokedInstallationForAppId;

  operations.options.fail = { repository: true };
  const offline = new BootstrapCoordinator({ configuration, operations, store });
  await assert.rejects(offline.prepare(), /repository_fixture_failure/);
  assert.equal(await readFile(store.output, 'utf8'), original);
});
