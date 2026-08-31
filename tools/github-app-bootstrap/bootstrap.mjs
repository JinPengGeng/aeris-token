#!/usr/bin/env node
import { createServer } from 'node:http';
import {
  createHash,
  createPrivateKey,
  createPublicKey,
  createSign,
  randomBytes,
} from 'node:crypto';
import { spawn } from 'node:child_process';
import {
  chmod,
  mkdir,
  open,
  readFile,
  rename,
  unlink,
} from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const OWNER = 'JinPengGeng';
export const REPOSITORY = `${OWNER}/aeris-token`;
export const ROLE_NAMES = Object.freeze(['writer', 'policy', 'merger', 'sync']);
export const RULESET_NAMES = Object.freeze(['agent_branches', 'merger_generation_tags']);

const here = path.dirname(fileURLToPath(import.meta.url));
const manifestEndpoint = 'https://github.com/settings/apps/new';
const apiVersion = '2022-11-28';
const secretValuePatterns = [
  /-----BEGIN [^-]*(?:PRIVATE KEY|CERTIFICATE)-----/i,
  /\bgithub_pat_[A-Za-z0-9_]{20,}\b/,
  /\bgh[pousr]_[A-Za-z0-9]{20,}\b/,
  /\bBearer\s+[A-Za-z0-9._~+\/-]{16,}/i,
  /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b/,
  /\bAKIA[0-9A-Z]{16}\b/,
  /\b[A-Za-z0-9+/=_-]{160,}\b/,
];
const forbiddenReceiptKeys = new Set([
  'authorization',
  'client_secret',
  'cookie',
  'jwt',
  'password',
  'pem',
  'private_key',
  'token',
  'webhook_secret',
]);

export class BootstrapError extends Error {
  constructor(code, stage, role = null, cause = undefined) {
    super(code, cause === undefined ? undefined : { cause });
    this.name = 'BootstrapError';
    this.code = code;
    this.stage = stage;
    this.role = role;
  }
}

function fail(code, stage, role = null, cause = undefined) {
  throw new BootstrapError(code, stage, role, cause);
}

function safeReason(value, fallback) {
  const normalized = String(value ?? fallback)
    .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
    .toLowerCase()
    .replace(/[^a-z0-9_]+/g, '_')
    .replace(/^_+|_+$/g, '')
    .slice(0, 80);
  return /^[a-z0-9_]{3,80}$/.test(normalized) ? normalized : fallback;
}

function base64url(value) {
  return Buffer.from(value).toString('base64url');
}

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonical(value[key])]));
  }
  return value;
}

const manifestKeys = Object.freeze([
  'default_events',
  'default_permissions',
  'description',
  'hook_attributes',
  'name',
  'public',
  'url',
]);

function configurationProjection(configuration) {
  return canonical({
    repository: configuration.mapping.repository,
    roles: configuration.mapping.roles,
    reviewer_security_identity: configuration.mapping.reviewer_security_identity,
    environment_policy: configuration.environmentPolicy,
    manifests: configuration.manifests,
    rulesets: Object.fromEntries(Object.entries(configuration.rulesets).map(([name, value]) => [name, rulesetProjection(value)])),
  });
}

export function configurationDigest(configuration) {
  return evidenceDigest(configurationProjection(configuration));
}

export function evidenceDigest(value) {
  return createHash('sha256').update(JSON.stringify(canonical(value))).digest('hex');
}

function publicKeyDigest(value) {
  try {
    const key = createPublicKey(String(value));
    if (key.asymmetricKeyType !== 'rsa') fail('writer_public_key_not_rsa', 'verification', 'writer');
    return evidenceDigest(key.export({ type: 'spki', format: 'pem' }).replace(/\r\n/g, '\n').trim());
  } catch (error) {
    if (error instanceof BootstrapError) throw error;
    fail('writer_public_key_invalid', 'verification', 'writer', error);
  }
}

function exactKeys(value, expected, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) fail(`${label}_not_object`, 'receipt');
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  if (JSON.stringify(actual) !== JSON.stringify(wanted)) fail(`${label}_keys_invalid`, 'receipt');
}

function assertSecretFree(value, key = null) {
  if (key && forbiddenReceiptKeys.has(key.toLowerCase())) fail('receipt_forbidden_key', 'receipt');
  if (typeof value === 'string') {
    if (secretValuePatterns.some((pattern) => pattern.test(value))) fail('receipt_secret_material', 'receipt');
    return;
  }
  if (Array.isArray(value)) {
    for (const item of value) assertSecretFree(item);
    return;
  }
  if (value && typeof value === 'object') {
    for (const [childKey, child] of Object.entries(value)) assertSecretFree(child, childKey);
  }
}

function nullableInteger(value) {
  return value === null || (Number.isSafeInteger(value) && value > 0);
}

function nullableSlug(value) {
  return value === null || (typeof value === 'string' && /^[a-z0-9][a-z0-9-]{0,99}$/.test(value));
}

function assertNamedReadback(value, expectedName, kind) {
  exactKeys(value, ['name', 'readback', 'value'], kind);
  if (value.name !== expectedName || typeof value.readback !== 'boolean') fail(`${kind}_invalid`, 'receipt');
  if (value.value !== null && typeof value.value !== 'string') fail(`${kind}_value_invalid`, 'receipt');
}

function assertRoleReceipt(role, value, mapping) {
  exactKeys(value, [
    'app_id',
    'app_slug',
    'environment',
    'installation',
    'pem_probe',
    'private_key_secret',
    'public_key_variable',
    'status',
    'switch',
    'variables',
  ], `${role}_receipt`);
  if (!['pending', 'conversion_started', 'reconciliation_required', 'converted', 'adoption_required', 'configuring', 'awaiting_install', 'verified', 'partial_failed'].includes(value.status)) fail(`${role}_status_invalid`, 'receipt');
  if (!nullableInteger(value.app_id) || !nullableSlug(value.app_slug)) fail(`${role}_identity_invalid`, 'receipt');

  exactKeys(value.switch, ['name', 'readback', 'value'], `${role}_switch`);
  if (value.switch.name !== mapping.enabled_variable || !['false', 'unknown'].includes(value.switch.value) || typeof value.switch.readback !== 'boolean') fail(`${role}_switch_invalid`, 'receipt');

  exactKeys(value.private_key_secret, ['name', 'present', 'updated_at'], `${role}_secret`);
  if (value.private_key_secret.name !== mapping.private_key_secret || typeof value.private_key_secret.present !== 'boolean' || (value.private_key_secret.updated_at !== null && !Number.isFinite(Date.parse(value.private_key_secret.updated_at)))) fail(`${role}_secret_invalid`, 'receipt');

  exactKeys(value.variables, ['app_id', 'app_slug'], `${role}_variables`);
  assertNamedReadback(value.variables.app_id, mapping.app_id_variable, `${role}_app_id_variable`);
  assertNamedReadback(value.variables.app_slug, mapping.app_slug_variable, `${role}_app_slug_variable`);

  if (role === 'writer') {
    assertNamedReadback(value.public_key_variable, 'AERIS_WRITER_PUBLIC_KEY', 'writer_public_key_variable');
  } else if (value.public_key_variable !== null) {
    fail(`${role}_public_key_variable_invalid`, 'receipt');
  }

  exactKeys(value.environment, ['evidence', 'name', 'verified'], `${role}_environment`);
  if (value.environment.name !== mapping.environment || typeof value.environment.verified !== 'boolean') fail(`${role}_environment_invalid`, 'receipt');
  if (value.environment.evidence !== null) {
    exactKeys(value.environment.evidence, ['branch_policies', 'reviewer_count', 'wait_timer'], `${role}_environment_evidence`);
    if (value.environment.evidence.reviewer_count !== 0 || value.environment.evidence.wait_timer !== 0 || JSON.stringify(value.environment.evidence.branch_policies) !== '["main"]') fail(`${role}_environment_evidence_invalid`, 'receipt');
  }

  if (value.installation !== null) {
    exactKeys(value.installation, ['account_id', 'account_login', 'app_updated_at', 'id', 'repository_full_name', 'repository_id', 'repository_selection'], `${role}_installation`);
    if (!Number.isSafeInteger(value.installation.id) || value.installation.id < 1 || !Number.isSafeInteger(value.installation.account_id) || value.installation.account_id < 1 || !Number.isSafeInteger(value.installation.repository_id) || value.installation.repository_id < 1 || value.installation.account_login !== OWNER || value.installation.repository_selection !== 'selected' || value.installation.repository_full_name !== REPOSITORY || !Number.isFinite(Date.parse(value.installation.app_updated_at))) fail(`${role}_installation_invalid`, 'receipt');
  }

  exactKeys(value.pem_probe, ['app_updated_at', 'verified'], `${role}_pem_probe`);
  if (typeof value.pem_probe.verified !== 'boolean' || (value.pem_probe.app_updated_at !== null && !Number.isFinite(Date.parse(value.pem_probe.app_updated_at)))) fail(`${role}_pem_probe_invalid`, 'receipt');

  if (value.status === 'verified') {
    if (!value.app_id || value.app_slug !== mapping.app_slug || value.switch.value !== 'false' || !value.switch.readback) fail(`${role}_verified_switch_invalid`, 'receipt');
    if (!value.private_key_secret.present || !value.private_key_secret.updated_at || !value.variables.app_id.readback || !value.variables.app_slug.readback) fail(`${role}_verified_storage_invalid`, 'receipt');
    if (value.variables.app_id.value !== String(value.app_id) || value.variables.app_slug.value !== mapping.app_slug) fail(`${role}_verified_variable_value_invalid`, 'receipt');
    if (!value.environment.verified || !value.installation) fail(`${role}_verified_live_evidence_missing`, 'receipt');
    if (!value.pem_probe.verified || value.pem_probe.app_updated_at !== value.installation.app_updated_at) fail(`${role}_verified_pem_probe_missing`, 'receipt');
    if (role === 'writer' && (!value.public_key_variable.readback || !/^[a-f0-9]{64}$/.test(value.public_key_variable.value))) fail('writer_verified_public_key_missing', 'receipt');
  }
  if (value.status === 'pending' && (value.app_id !== null || value.app_slug !== null)) fail(`${role}_pending_identity_invalid`, 'receipt');
  if (['conversion_started', 'reconciliation_required'].includes(value.status) && (value.app_id !== null || value.app_slug !== mapping.app_slug)) fail(`${role}_ambiguous_conversion_identity_invalid`, 'receipt');
  if (['converted', 'adoption_required', 'configuring', 'awaiting_install'].includes(value.status) && (!value.app_id || value.app_slug !== mapping.app_slug)) fail(`${role}_checkpoint_identity_invalid`, 'receipt');
}

export function assertReceiptDocument(document, configuration) {
  const mapping = configuration?.mapping;
  if (!mapping || !configuration?.environmentPolicy || !configuration?.manifests || !configuration?.rulesets) fail('receipt_expected_configuration_missing', 'receipt');
  exactKeys(document, ['configuration_digest', 'created_at', 'disable_failures', 'failure', 'release', 'repository', 'roles', 'rulesets', 'schema_version', 'status', 'updated_at'], 'receipt');
  if (document.schema_version !== 5 || document.repository !== REPOSITORY || document.configuration_digest !== configurationDigest(configuration)) fail('receipt_header_invalid', 'receipt');
  if (!['in_progress', 'partial_failed', 'verified'].includes(document.status)) fail('receipt_status_invalid', 'receipt');
  if (!Number.isFinite(Date.parse(document.created_at)) || !Number.isFinite(Date.parse(document.updated_at))) fail('receipt_timestamp_invalid', 'receipt');
  exactKeys(document.roles, ROLE_NAMES, 'receipt_roles');
  exactKeys(document.rulesets, RULESET_NAMES, 'receipt_rulesets');
  for (const name of RULESET_NAMES) {
    exactKeys(document.rulesets[name], ['digest', 'name', 'verified'], `${name}_ruleset`);
    const expectedRuleset = configuration.rulesets[name];
    const expectedRulesetDigest = evidenceDigest(rulesetProjection(expectedRuleset));
    if (document.rulesets[name].name !== expectedRuleset.name || typeof document.rulesets[name].verified !== 'boolean' || (document.rulesets[name].digest !== null && !/^[a-f0-9]{64}$/.test(document.rulesets[name].digest))) fail(`${name}_ruleset_invalid`, 'receipt');
    if (document.rulesets[name].verified && document.rulesets[name].digest !== expectedRulesetDigest) fail(`${name}_ruleset_expected_value_mismatch`, 'receipt');
  }
  for (const role of ROLE_NAMES) assertRoleReceipt(role, document.roles[role], mapping.roles[role]);
  let nonVerifiedRoleSeen = false;
  for (const role of ROLE_NAMES) {
    if (document.roles[role].status === 'verified') {
      if (nonVerifiedRoleSeen) fail('receipt_verified_roles_not_prefix', 'receipt');
    } else {
      nonVerifiedRoleSeen = true;
    }
  }
  if (document.status !== 'verified' && ROLE_NAMES.every((role) => document.roles[role].status === 'verified')) fail('receipt_status_role_mismatch', 'receipt');
  exactKeys(document.release, ['checked', 'end_digest', 'manual_approval_verified', 'reviewer_count', 'start_digest', 'unchanged'], 'release_evidence');
  for (const field of ['start_digest', 'end_digest']) {
    if (document.release[field] !== null && !/^[a-f0-9]{64}$/.test(document.release[field])) fail('release_digest_invalid', 'receipt');
  }
  if (typeof document.release.checked !== 'boolean' || typeof document.release.unchanged !== 'boolean' || typeof document.release.manual_approval_verified !== 'boolean' || !Number.isSafeInteger(document.release.reviewer_count) || document.release.reviewer_count < 0) fail('release_evidence_invalid', 'receipt');
  if (document.failure !== null) {
    exactKeys(document.failure, ['code', 'role', 'stage'], 'failure');
    if (!/^[a-z0-9_]{3,80}$/.test(document.failure.code) || !/^[a-z0-9_]{3,80}$/.test(document.failure.stage)) fail('failure_invalid', 'receipt');
    if (document.failure.role !== null && !ROLE_NAMES.includes(document.failure.role)) fail('failure_role_invalid', 'receipt');
  }
  if (!Array.isArray(document.disable_failures)) fail('disable_failures_invalid', 'receipt');
  const disableFailureRoles = new Set();
  for (const entry of document.disable_failures) {
    exactKeys(entry, ['code', 'role'], 'disable_failure');
    if (!ROLE_NAMES.includes(entry.role) || !/^[a-z0-9_]{3,80}$/.test(entry.code) || disableFailureRoles.has(entry.role)) fail('disable_failure_invalid', 'receipt');
    disableFailureRoles.add(entry.role);
  }
  if (document.status !== 'partial_failed' && document.disable_failures.length !== 0) fail('disable_failures_status_invalid', 'receipt');

  if (document.status === 'verified') {
    if (document.failure !== null || !document.release.manual_approval_verified || document.release.reviewer_count < 1 || !document.release.checked || !document.release.unchanged || document.release.start_digest !== document.release.end_digest) fail('verified_release_invalid', 'receipt');
    if (ROLE_NAMES.some((role) => document.roles[role].status !== 'verified') || RULESET_NAMES.some((name) => !document.rulesets[name].verified || !document.rulesets[name].digest)) fail('verified_live_evidence_incomplete', 'receipt');
    const ids = ROLE_NAMES.map((role) => document.roles[role].app_id);
    const slugs = ROLE_NAMES.map((role) => document.roles[role].app_slug);
    if (new Set(ids).size !== ROLE_NAMES.length || new Set(slugs).size !== ROLE_NAMES.length) fail('verified_app_identities_not_unique', 'receipt');
    const installations = ROLE_NAMES.map((role) => document.roles[role].installation);
    if (new Set(installations.map((value) => value.id)).size !== ROLE_NAMES.length) fail('verified_installation_identities_not_unique', 'receipt');
    if (new Set(installations.map((value) => value.account_id)).size !== 1 || new Set(installations.map((value) => value.repository_id)).size !== 1) fail('verified_installation_scope_inconsistent', 'receipt');
  }
  assertSecretFree(document);
  return document;
}

function makeRoleReceipt(mapping, role) {
  return {
    status: 'pending',
    app_id: null,
    app_slug: null,
    switch: { name: mapping.enabled_variable, value: 'unknown', readback: false },
    private_key_secret: { name: mapping.private_key_secret, present: false, updated_at: null },
    variables: {
      app_id: { name: mapping.app_id_variable, value: null, readback: false },
      app_slug: { name: mapping.app_slug_variable, value: null, readback: false },
    },
    public_key_variable: role === 'writer'
      ? { name: 'AERIS_WRITER_PUBLIC_KEY', value: null, readback: false }
      : null,
    pem_probe: { verified: false, app_updated_at: null },
    environment: { name: mapping.environment, verified: false, evidence: null },
    installation: null,
  };
}

function initialReceipt(configuration, now) {
  const mapping = configuration.mapping;
  const timestamp = new Date(now()).toISOString();
  return {
    schema_version: 5,
    repository: REPOSITORY,
    configuration_digest: configurationDigest(configuration),
    status: 'in_progress',
    created_at: timestamp,
    updated_at: timestamp,
    failure: null,
    disable_failures: [],
    release: { start_digest: null, end_digest: null, checked: false, unchanged: false, manual_approval_verified: false, reviewer_count: 0 },
    rulesets: {
      agent_branches: { name: 'aeris-writer-agent-branches', verified: false, digest: null },
      merger_generation_tags: { name: 'aeris-merger-generation-tags', verified: false, digest: null },
    },
    roles: Object.fromEntries(ROLE_NAMES.map((role) => [role, makeRoleReceipt(mapping.roles[role], role)])),
  };
}

export class ReceiptStore {
  constructor({ configuration, root = path.join(here, 'receipts'), filename = 'latest.json' }) {
    if (!configuration) fail('receipt_expected_configuration_missing', 'receipt');
    this.configuration = configuration;
    this.root = path.resolve(root);
    this.output = path.join(this.root, filename);
    this.lockPath = path.join(this.root, '.bootstrap.lock');
    this.lockHandle = null;
    if (path.resolve(this.output) !== this.output || path.dirname(this.output) !== this.root) fail('receipt_output_path_invalid', 'receipt');
  }

  async acquireExclusiveLock() {
    if (this.lockHandle) fail('bootstrap_lock_already_held', 'lock');
    await mkdir(this.root, { recursive: true, mode: 0o700 });
    await chmod(this.root, 0o700);
    try {
      this.lockHandle = await open(this.lockPath, 'wx', 0o600);
      await this.lockHandle.writeFile(`${JSON.stringify({ pid: process.pid, started_at: new Date().toISOString() })}\n`, 'utf8');
      await this.lockHandle.sync();
      await chmod(this.lockPath, 0o600);
    } catch (error) {
      if (this.lockHandle) await this.lockHandle.close().catch(() => {});
      this.lockHandle = null;
      fail(error?.code === 'EEXIST' ? 'bootstrap_lock_unavailable' : 'bootstrap_lock_acquire_failed', 'lock', null, error);
    }
  }

  async releaseExclusiveLock() {
    if (!this.lockHandle) return;
    const handle = this.lockHandle;
    this.lockHandle = null;
    try {
      await handle.close();
      await unlink(this.lockPath);
    } catch (error) {
      fail('bootstrap_lock_release_failed', 'lock', null, error);
    }
  }

  async load() {
    try {
      const document = JSON.parse(await readFile(this.output, 'utf8'));
      return assertReceiptDocument(document, this.configuration);
    } catch (error) {
      if (error?.code === 'ENOENT') return null;
      if (error instanceof BootstrapError) throw error;
      fail('existing_receipt_invalid', 'receipt', null, error);
    }
  }

  async write(document) {
    assertReceiptDocument(document, this.configuration);
    await mkdir(this.root, { recursive: true, mode: 0o700 });
    await chmod(this.root, 0o700);
    const temp = path.join(this.root, `.latest.${process.pid}.${randomBytes(12).toString('hex')}.tmp`);
    const serialized = `${JSON.stringify(document, null, 2)}\n`;
    let handle;
    try {
      handle = await open(temp, 'wx', 0o600);
      await handle.writeFile(serialized, 'utf8');
      await handle.sync();
      await handle.close();
      handle = null;
      await chmod(temp, 0o600);
      await rename(temp, this.output);
      await chmod(this.output, 0o600);
    } catch (error) {
      if (handle) await handle.close().catch(() => {});
      await unlink(temp).catch(() => {});
      fail('receipt_atomic_write_failed', 'receipt', null, error);
    }
    return this.output;
  }
}

export function assertManifest(role, manifest, mapping) {
  exactKeys(manifest, manifestKeys, `${role}_manifest`);
  exactKeys(manifest.hook_attributes, ['active'], `${role}_manifest_hook_attributes`);
  if (manifest.name !== mapping.app_name || manifest.public !== false || manifest.hook_attributes?.active !== false) fail(`${role}_manifest_identity_invalid`, 'manifest', role);
  if (!Array.isArray(manifest.default_events) || manifest.default_events.length !== 0) fail(`${role}_manifest_events_invalid`, 'manifest', role);
  if (JSON.stringify(canonical(manifest.default_permissions)) !== JSON.stringify(canonical(mapping.permissions))) fail(`${role}_manifest_permissions_invalid`, 'manifest', role);
}

export async function loadConfiguration() {
  const mapping = JSON.parse(await readFile(path.join(here, 'roles.json'), 'utf8'));
  const environmentPolicy = JSON.parse(await readFile(path.join(here, 'environment-policy.json'), 'utf8'));
  const rulesets = {
    agent_branches: JSON.parse(await readFile(path.join(here, 'agent-branch-ruleset.json'), 'utf8')),
    merger_generation_tags: JSON.parse(await readFile(path.join(here, 'merger-tag-ruleset.json'), 'utf8')),
  };
  if (mapping.schema_version !== 1 || mapping.repository !== REPOSITORY) fail('role_mapping_invalid', 'configuration');
  if (environmentPolicy.schema_version !== 1 || environmentPolicy.repository !== REPOSITORY || environmentPolicy.default_branch !== 'main') fail('environment_policy_invalid', 'configuration');
  exactKeys(mapping.roles, ROLE_NAMES, 'mapped_roles');
  exactKeys(environmentPolicy.environments, ROLE_NAMES, 'environment_roles');
  const manifests = {};
  for (const role of ROLE_NAMES) {
    const roleMapping = mapping.roles[role];
    if (roleMapping.environment !== role || roleMapping.private_key_secret.includes('RELEASE')) fail(`${role}_mapping_isolation_invalid`, 'configuration', role);
    const policy = environmentPolicy.environments[role];
    if (JSON.stringify(policy.required_reviewers) !== '[]' || policy.wait_timer !== 0 || JSON.stringify(policy.deployment_branches) !== '["main"]') fail(`${role}_environment_policy_invalid`, 'configuration', role);
    manifests[role] = JSON.parse(await readFile(path.join(here, 'manifests', `${role}.json`), 'utf8'));
    assertManifest(role, manifests[role], roleMapping);
  }
  assertRuleset(rulesets.agent_branches, {
    name: 'aeris-writer-agent-branches',
    target: 'branch',
    include: ['refs/heads/agent/**'],
    types: ['deletion', 'non_fast_forward'],
  });
  assertRuleset(rulesets.merger_generation_tags, {
    name: 'aeris-merger-generation-tags',
    target: 'tag',
    include: ['refs/tags/aeris-merger-attempt-*'],
    types: ['deletion', 'non_fast_forward', 'update'],
  });
  return { mapping, environmentPolicy, manifests, rulesets };
}

function rulesetProjection(value) {
  return {
    name: value?.name,
    target: value?.target,
    enforcement: value?.enforcement,
    conditions: canonical(value?.conditions),
    rules: (value?.rules ?? []).map(canonical).sort((left, right) => left.type.localeCompare(right.type)),
    bypass_actors: canonical(value?.bypass_actors ?? []),
  };
}

export function assertRuleset(value, contract) {
  const projection = rulesetProjection(value);
  if (projection.name !== contract.name || projection.target !== contract.target || projection.enforcement !== 'active') fail(`${contract.name}_ruleset_header_invalid`, 'ruleset');
  if (JSON.stringify(projection.conditions) !== JSON.stringify(canonical({ ref_name: { include: contract.include, exclude: [] } }))) fail(`${contract.name}_ruleset_conditions_invalid`, 'ruleset');
  const types = projection.rules.map((rule) => rule.type).sort();
  if (JSON.stringify(types) !== JSON.stringify([...contract.types].sort())) fail(`${contract.name}_ruleset_rules_invalid`, 'ruleset');
  for (const rule of projection.rules) {
    const keys = Object.keys(rule).sort();
    if (rule.type === 'update') {
      if (JSON.stringify(keys) !== JSON.stringify(['parameters', 'type']) || JSON.stringify(rule.parameters) !== JSON.stringify({ update_allows_fetch_and_merge: false })) fail(`${contract.name}_ruleset_update_invalid`, 'ruleset');
    } else if (JSON.stringify(keys) !== JSON.stringify(['type'])) {
      fail(`${contract.name}_ruleset_rule_shape_invalid`, 'ruleset');
    }
  }
  if (projection.bypass_actors.length !== 0) fail(`${contract.name}_ruleset_bypass_invalid`, 'ruleset');
  return projection;
}

function runProcess(command, args, { input = null, secretInput = false, timeoutMs = 60_000 } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
    const stdout = [];
    const stderr = [];
    let stdoutBytes = 0;
    let stderrBytes = 0;
    let settled = false;
    const maximumBytes = 4 * 1024 * 1024;
    const finish = (callback) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      callback();
    };
    const timer = setTimeout(() => {
      finish(() => {
        child.kill();
        reject(new BootstrapError(secretInput ? 'gh_secret_command_timeout' : 'gh_command_timeout', 'github_cli'));
      });
    }, timeoutMs);
    timer.unref();
    child.stdout.on('data', (chunk) => {
      stdoutBytes += chunk.length;
      if (stdoutBytes <= maximumBytes) stdout.push(chunk);
    });
    child.stderr.on('data', (chunk) => {
      stderrBytes += chunk.length;
      if (stderrBytes <= maximumBytes) stderr.push(chunk);
    });
    child.once('error', () => finish(() => reject(new BootstrapError('gh_process_start_failed', 'github_cli'))));
    child.once('close', (code) => {
      finish(() => {
        if (code !== 0 || stdoutBytes > maximumBytes || stderrBytes > maximumBytes) {
          reject(new BootstrapError(secretInput ? 'gh_secret_command_failed' : 'gh_command_failed', 'github_cli'));
          return;
        }
        resolve(Buffer.concat(stdout).toString('utf8'));
      });
    });
    child.stdin.end(input ?? undefined);
  });
}

async function ghJson(args, body = undefined) {
  const fullArgs = ['api', ...args, '-H', 'Accept: application/vnd.github+json', '-H', `X-GitHub-Api-Version: ${apiVersion}`];
  if (body !== undefined) fullArgs.push('--input', '-');
  const output = await runProcess('gh', fullArgs, { input: body === undefined ? null : JSON.stringify(body) });
  if (!output.trim()) return null;
  try {
    return JSON.parse(output);
  } catch (error) {
    fail('github_api_invalid_json', 'github_api', null, error);
  }
}

function normalizeEnvironment(environment, branchPolicies) {
  const rules = Array.isArray(environment?.protection_rules) ? environment.protection_rules : [];
  const reviewerRule = rules.find((rule) => rule.type === 'required_reviewers');
  const waitRule = rules.find((rule) => rule.type === 'wait_timer');
  const reviewers = Array.isArray(reviewerRule?.reviewers)
    ? reviewerRule.reviewers.map((entry) => entry?.reviewer?.login ?? entry?.reviewer?.slug ?? entry?.login ?? entry?.slug).filter((login) => typeof login === 'string').sort()
    : [];
  return {
    reviewer_count: reviewers.length,
    reviewers,
    wait_timer: Number(waitRule?.wait_timer ?? environment?.wait_timer ?? 0),
    deployment_branch_policy: environment?.deployment_branch_policy ?? null,
    branch_policies: branchPolicies.map((entry) => ({ id: entry.id, name: entry.name, type: entry.type })).sort((a, b) => a.id - b.id),
    protection_rule_types: rules.map((rule) => rule.type).sort(),
    raw: canonical(environment),
  };
}

export function assertAgentEnvironment(snapshot, role) {
  if (snapshot.reviewer_count !== 0 || snapshot.wait_timer !== 0) fail(`${role}_environment_protection_invalid`, 'environment', role);
  if (snapshot.protection_rule_types.some((type) => !['branch_policy', 'required_reviewers', 'wait_timer'].includes(type))) fail(`${role}_environment_custom_rule_invalid`, 'environment', role);
  if (snapshot.deployment_branch_policy?.protected_branches !== false || snapshot.deployment_branch_policy?.custom_branch_policies !== true) fail(`${role}_environment_branch_mode_invalid`, 'environment', role);
  if (snapshot.branch_policies.length !== 1 || snapshot.branch_policies[0].name !== 'main' || snapshot.branch_policies[0].type !== 'branch') fail(`${role}_environment_branch_policy_invalid`, 'environment', role);
  return {
    reviewer_count: 0,
    wait_timer: 0,
    branch_policies: ['main'],
  };
}

export function assertReleaseEnvironment(snapshot) {
  if (!snapshot || snapshot.reviewer_count < 1 || snapshot.reviewers.length !== snapshot.reviewer_count) fail('release_manual_approval_missing', 'release');
  if (!snapshot.protection_rule_types.includes('required_reviewers')) fail('release_required_reviewer_rule_missing', 'release');
  return { reviewer_count: snapshot.reviewer_count };
}

export function createAppJwt(appId, pem, now = Date.now()) {
  if (!Number.isSafeInteger(appId) || appId < 1 || typeof pem !== 'string') fail('jwt_input_invalid', 'jwt');
  let key;
  try {
    key = createPrivateKey(pem);
  } catch (error) {
    fail('jwt_private_key_invalid', 'jwt', null, error);
  }
  if (key.type !== 'private' || key.asymmetricKeyType !== 'rsa') fail('jwt_private_key_not_rsa', 'jwt');
  const issuedAt = Math.floor(now / 1000) - 60;
  const header = base64url(JSON.stringify({ alg: 'RS256', typ: 'JWT' }));
  const payload = base64url(JSON.stringify({ iat: issuedAt, exp: issuedAt + 540, iss: String(appId) }));
  const signer = createSign('RSA-SHA256');
  signer.update(`${header}.${payload}`);
  signer.end();
  return `${header}.${payload}.${signer.sign(key).toString('base64url')}`;
}

function assertLiveAppIdentity(app, appId, expectedApp, ownerId, stage) {
  if (app?.id !== appId || app.slug !== expectedApp.app_slug || app.name !== expectedApp.app_name || app.owner?.id !== ownerId || app.owner?.login !== OWNER) fail('app_live_identity_invalid', stage);
  if (!Number.isFinite(Date.parse(app.updated_at))) fail('app_live_version_invalid', stage);
  if (JSON.stringify(canonical(app.permissions)) !== JSON.stringify(canonical(expectedApp.permissions)) || JSON.stringify(app.events ?? []) !== '[]') fail('app_live_permissions_invalid', stage);
  return app;
}

async function fetchJson(fetchImpl, url, init, code, timeoutMs = 60_000) {
  let response;
  try {
    response = await fetchImpl(url, { ...init, signal: init.signal ?? AbortSignal.timeout(timeoutMs) });
  } catch (error) {
    fail(`${code}_transport_failed`, code, null, error);
  }
  if (!response.ok) fail(`${code}_http_${response.status}`, code);
  try {
    return { value: await response.json(), headers: response.headers };
  } catch (error) {
    fail(`${code}_invalid_json`, code, null, error);
  }
}

async function fetchNoContent(fetchImpl, url, init, code, timeoutMs = 60_000) {
  let response;
  try {
    response = await fetchImpl(url, { ...init, signal: init.signal ?? AbortSignal.timeout(timeoutMs) });
  } catch (error) {
    fail(`${code}_transport_failed`, code, null, error);
  }
  if (!response.ok) fail(`${code}_http_${response.status}`, code);
}

export class GhGitHubOperations {
  constructor({ fetchImpl = fetch, now = Date.now, timeoutMs = 60_000 } = {}) {
    this.fetchImpl = fetchImpl;
    this.now = now;
    this.timeoutMs = timeoutMs;
  }

  async setVariable(name, value) {
    await runProcess('gh', ['variable', 'set', name, '--repo', REPOSITORY, '--body', value]);
  }

  async getVariable(name) {
    return (await runProcess('gh', ['variable', 'get', name, '--repo', REPOSITORY, '--json', 'value', '--jq', '.value'])).replace(/\r?\n$/, '');
  }

  async setEnvironmentSecret(environment, name, value) {
    if (environment === 'release') fail('release_secret_mutation_forbidden', 'secret');
    await runProcess('gh', ['secret', 'set', name, '--env', environment, '--repo', REPOSITORY], { input: value, secretInput: true });
  }

  async listEnvironmentSecretNames(environment) {
    const output = await runProcess('gh', ['secret', 'list', '--env', environment, '--repo', REPOSITORY, '--json', 'name']);
    try {
      return JSON.parse(output).map((entry) => entry.name);
    } catch (error) {
      fail('secret_list_invalid_json', 'secret', null, error);
    }
  }

  async getEnvironmentSecret(environment, name) {
    const secret = await ghJson([`repos/${REPOSITORY}/environments/${environment}/secrets/${name}`]);
    if (secret?.name !== name || !Number.isFinite(Date.parse(secret?.updated_at))) fail('environment_secret_metadata_invalid', 'secret');
    return { name: secret.name, updated_at: secret.updated_at };
  }

  async repository() {
    return ghJson([`repos/${REPOSITORY}`]);
  }

  async listRulesets() {
    const rulesets = [];
    for (let page = 1; page <= 100; page += 1) {
      const entries = await ghJson([`repos/${REPOSITORY}/rulesets?includes_parents=false&per_page=100&page=${page}`]);
      if (!Array.isArray(entries)) fail('ruleset_list_invalid', 'ruleset');
      rulesets.push(...entries);
      if (entries.length < 100) return rulesets;
    }
    fail('ruleset_pagination_exceeded', 'ruleset');
  }

  async readRuleset(name) {
    const matches = (await this.listRulesets()).filter((entry) => entry.name === name && entry.source_type === 'Repository');
    if (matches.length !== 1 || !Number.isSafeInteger(matches[0].id)) fail('ruleset_identity_count_invalid', 'ruleset');
    return ghJson([`repos/${REPOSITORY}/rulesets/${matches[0].id}`]);
  }

  async applyRuleset(payload) {
    const matches = (await this.listRulesets()).filter((entry) => entry.name === payload.name && entry.source_type === 'Repository');
    if (matches.length > 1) fail('ruleset_identity_count_invalid', 'ruleset');
    if (matches.length === 1) {
      await ghJson([`repos/${REPOSITORY}/rulesets/${matches[0].id}`, '--method', 'PUT'], payload);
    } else {
      await ghJson([`repos/${REPOSITORY}/rulesets`, '--method', 'POST'], payload);
    }
  }

  async deploymentPolicies(environment) {
    const policies = [];
    for (let page = 1; page <= 100; page += 1) {
      const response = await ghJson([`repos/${REPOSITORY}/environments/${environment}/deployment-branch-policies?per_page=100&page=${page}`]);
      const entries = response?.branch_policies ?? [];
      policies.push(...entries);
      if (entries.length < 100) {
        if (Number(response?.total_count ?? policies.length) !== policies.length) fail('environment_policy_pagination_incomplete', 'environment');
        return policies;
      }
    }
    fail('environment_policy_pagination_exceeded', 'environment');
  }

  async readEnvironment(environment) {
    const live = await ghJson([`repos/${REPOSITORY}/environments/${environment}`]);
    const policies = live?.deployment_branch_policy?.custom_branch_policies
      ? await this.deploymentPolicies(environment)
      : [];
    return normalizeEnvironment(live, policies);
  }

  async applyEnvironment(environment) {
    if (!ROLE_NAMES.includes(environment)) fail('environment_mutation_target_forbidden', 'environment');
    await ghJson([`repos/${REPOSITORY}/environments/${environment}`, '--method', 'PUT'], {
      wait_timer: 0,
      reviewers: [],
      deployment_branch_policy: { protected_branches: false, custom_branch_policies: true },
    });
    const policies = await this.deploymentPolicies(environment);
    let mainPresent = false;
    for (const policy of policies) {
      if (policy.name === 'main' && policy.type === 'branch' && !mainPresent) {
        mainPresent = true;
      } else {
        await ghJson([`repos/${REPOSITORY}/environments/${environment}/deployment-branch-policies/${policy.id}`, '--method', 'DELETE']);
      }
    }
    if (!mainPresent) {
      await ghJson([`repos/${REPOSITORY}/environments/${environment}/deployment-branch-policies`, '--method', 'POST'], { name: 'main', type: 'branch' });
    }
  }

  async convertManifest(code) {
    const result = await fetchJson(this.fetchImpl, `https://api.github.com/app-manifests/${encodeURIComponent(code)}/conversions`, {
      method: 'POST',
      headers: { Accept: 'application/vnd.github+json', 'X-GitHub-Api-Version': apiVersion },
    }, 'manifest_conversion', this.timeoutMs);
    return result.value;
  }

  async verifyAppIdentity(appId, pem, repository, expectedApp) {
    const jwt = createAppJwt(appId, pem, this.now());
    const result = await fetchJson(this.fetchImpl, 'https://api.github.com/app', {
      headers: { Accept: 'application/vnd.github+json', Authorization: `Bearer ${jwt}`, 'X-GitHub-Api-Version': apiVersion },
    }, 'app_reconciliation', this.timeoutMs);
    const app = assertLiveAppIdentity(result.value, appId, expectedApp, repository.owner.id, 'reconciliation');
    return { app_updated_at: app.updated_at };
  }

  async verifyInstallation(appId, pem, repository, expectedApp) {
    const jwt = createAppJwt(appId, pem, this.now());
    const appHeaders = { Accept: 'application/vnd.github+json', Authorization: `Bearer ${jwt}`, 'X-GitHub-Api-Version': apiVersion };
    const appResult = await fetchJson(this.fetchImpl, 'https://api.github.com/app', { headers: appHeaders }, 'app_identity', this.timeoutMs);
    const app = appResult.value;
    assertLiveAppIdentity(app, appId, expectedApp, repository.owner.id, 'installation');
    const installationsResult = await fetchJson(this.fetchImpl, 'https://api.github.com/app/installations?per_page=100', { headers: appHeaders }, 'installation_list', this.timeoutMs);
    if (installationsResult.headers?.get?.('link')?.includes('rel="next"')) fail('extra_installation_page', 'installation');
    const installations = installationsResult.value;
    if (!Array.isArray(installations) || installations.length !== 1) fail('installation_count_invalid', 'installation');
    const installation = installations[0];
    if (installation.app_id !== appId || installation.account?.login !== OWNER || installation.account?.id !== repository.owner.id || installation.target_type !== 'User' || installation.repository_selection !== 'selected') fail('installation_identity_invalid', 'installation');
    if (JSON.stringify(canonical(installation.permissions)) !== JSON.stringify(canonical(expectedApp.permissions)) || JSON.stringify(installation.events ?? []) !== '[]') fail('installation_permissions_invalid', 'installation');

    let installationToken = null;
    try {
      const tokenResult = await fetchJson(this.fetchImpl, `https://api.github.com/app/installations/${installation.id}/access_tokens`, {
        method: 'POST',
        headers: { ...appHeaders, 'Content-Type': 'application/json' },
        body: JSON.stringify({ repository_ids: [repository.id], permissions: expectedApp.permissions }),
      }, 'installation_token', this.timeoutMs);
      installationToken = tokenResult.value?.token;
      if (typeof installationToken !== 'string' || installationToken.length < 20) fail('installation_token_invalid', 'installation');
      if (tokenResult.value?.repository_selection !== 'selected' || JSON.stringify(canonical(tokenResult.value?.permissions)) !== JSON.stringify(canonical(expectedApp.permissions))) fail('installation_token_scope_invalid', 'installation');
      const repositoriesResult = await fetchJson(this.fetchImpl, 'https://api.github.com/installation/repositories?per_page=100', {
        headers: { Accept: 'application/vnd.github+json', Authorization: `Bearer ${installationToken}`, 'X-GitHub-Api-Version': apiVersion },
      }, 'installation_repositories', this.timeoutMs);
      if (repositoriesResult.headers?.get?.('link')?.includes('rel="next"')) fail('extra_repository_page', 'installation');
      const repositories = repositoriesResult.value?.repositories;
      if (repositoriesResult.value?.total_count !== 1 || !Array.isArray(repositories) || repositories.length !== 1) fail('installation_repository_count_invalid', 'installation');
      const installed = repositories[0];
      if (installed.id !== repository.id || installed.full_name !== REPOSITORY) fail('installation_repository_identity_invalid', 'installation');
      return {
        id: installation.id,
        app_updated_at: app.updated_at,
        account_id: installation.account.id,
        account_login: installation.account.login,
        repository_selection: installation.repository_selection,
        repository_id: installed.id,
        repository_full_name: installed.full_name,
      };
    } finally {
      if (installationToken) {
        const tokenHeaders = { Accept: 'application/vnd.github+json', Authorization: `Bearer ${installationToken}`, 'X-GitHub-Api-Version': apiVersion };
        try {
          await fetchNoContent(this.fetchImpl, 'https://api.github.com/installation/token', { method: 'DELETE', headers: tokenHeaders }, 'installation_token_revoke', this.timeoutMs);
        } finally {
          installationToken = null;
        }
      }
    }
  }

  async probeInstallation(appId, pem, expectedApp) {
    const jwt = createAppJwt(appId, pem, this.now());
    const appHeaders = { Accept: 'application/vnd.github+json', Authorization: `Bearer ${jwt}`, 'X-GitHub-Api-Version': apiVersion };
    const appResult = await fetchJson(this.fetchImpl, 'https://api.github.com/app', { headers: appHeaders }, 'app_probe_identity', this.timeoutMs);
    const app = appResult.value;
    if (app?.owner?.login !== OWNER || !Number.isSafeInteger(app.owner?.id) || app.owner.id < 1) fail('app_probe_owner_invalid', 'activation_probe');
    assertLiveAppIdentity(app, appId, expectedApp, app.owner.id, 'activation_probe');

    const installationsResult = await fetchJson(this.fetchImpl, 'https://api.github.com/app/installations?per_page=100', { headers: appHeaders }, 'app_probe_installations', this.timeoutMs);
    if (installationsResult.headers?.get?.('link')?.includes('rel="next"')) fail('app_probe_extra_installation_page', 'activation_probe');
    const installations = installationsResult.value;
    if (!Array.isArray(installations) || installations.length !== 1) fail('app_probe_installation_count_invalid', 'activation_probe');
    const installation = installations[0];
    if (installation.app_id !== appId || installation.account?.login !== OWNER || installation.account?.id !== app.owner.id || installation.target_type !== 'User' || installation.repository_selection !== 'selected') fail('app_probe_installation_identity_invalid', 'activation_probe');
    if (JSON.stringify(canonical(installation.permissions)) !== JSON.stringify(canonical(expectedApp.permissions)) || JSON.stringify(installation.events ?? []) !== '[]') fail('app_probe_installation_permissions_invalid', 'activation_probe');

    let installationToken = null;
    try {
      const tokenResult = await fetchJson(this.fetchImpl, `https://api.github.com/app/installations/${installation.id}/access_tokens`, {
        method: 'POST',
        headers: { ...appHeaders, 'Content-Type': 'application/json' },
        body: JSON.stringify({ permissions: expectedApp.permissions }),
      }, 'app_probe_token', this.timeoutMs);
      installationToken = tokenResult.value?.token;
      if (typeof installationToken !== 'string' || installationToken.length < 20) fail('app_probe_token_invalid', 'activation_probe');
      if (tokenResult.value?.repository_selection !== 'selected' || JSON.stringify(canonical(tokenResult.value?.permissions)) !== JSON.stringify(canonical(expectedApp.permissions))) fail('app_probe_token_scope_invalid', 'activation_probe');
      const repositoriesResult = await fetchJson(this.fetchImpl, 'https://api.github.com/installation/repositories?per_page=100', {
        headers: { Accept: 'application/vnd.github+json', Authorization: `Bearer ${installationToken}`, 'X-GitHub-Api-Version': apiVersion },
      }, 'app_probe_repositories', this.timeoutMs);
      if (repositoriesResult.headers?.get?.('link')?.includes('rel="next"')) fail('app_probe_extra_repository_page', 'activation_probe');
      const repositories = repositoriesResult.value?.repositories;
      if (repositoriesResult.value?.total_count !== 1 || !Array.isArray(repositories) || repositories.length !== 1) fail('app_probe_repository_count_invalid', 'activation_probe');
      const repository = repositories[0];
      if (repository.full_name !== REPOSITORY || !Number.isSafeInteger(repository.id) || repository.id < 1) fail('app_probe_repository_identity_invalid', 'activation_probe');
      return {
        app_id: app.id,
        app_slug: app.slug,
        app_updated_at: app.updated_at,
        installation_id: installation.id,
        repository_id: repository.id,
        repository_full_name: repository.full_name,
      };
    } finally {
      if (installationToken) {
        const tokenHeaders = { Accept: 'application/vnd.github+json', Authorization: `Bearer ${installationToken}`, 'X-GitHub-Api-Version': apiVersion };
        try {
          await fetchNoContent(this.fetchImpl, 'https://api.github.com/installation/token', { method: 'DELETE', headers: tokenHeaders }, 'app_probe_token_revoke', this.timeoutMs);
        } finally {
          installationToken = null;
        }
      }
    }
  }

  async verifyExistingInstallation(appId, repository, expectedApp) {
    const app = await ghJson([`apps/${expectedApp.app_slug}`]);
    if (app?.id !== appId || app.slug !== expectedApp.app_slug || app.name !== expectedApp.app_name || app.owner?.id !== repository.owner.id || app.owner?.login !== OWNER) fail('app_live_identity_invalid', 'verified_readback');
    if (!Number.isFinite(Date.parse(app.updated_at))) fail('app_live_version_invalid', 'verified_readback');
    if (JSON.stringify(canonical(app.permissions)) !== JSON.stringify(canonical(expectedApp.permissions)) || JSON.stringify(app.events ?? []) !== '[]') fail('app_live_permissions_invalid', 'verified_readback');

    const page = await ghJson(['user/installations?per_page=100']);
    if (!Array.isArray(page?.installations) || page.total_count !== page.installations.length) fail('verified_installation_pagination_incomplete', 'verified_readback');
    const matches = page.installations.filter((entry) => entry.app_id === appId && entry.account?.id === repository.owner.id && entry.account?.login === OWNER);
    if (matches.length !== 1) fail('verified_installation_count_invalid', 'verified_readback');
    const installation = matches[0];
    if (installation.target_type !== 'User' || installation.repository_selection !== 'selected') fail('verified_installation_identity_invalid', 'verified_readback');
    if (JSON.stringify(canonical(installation.permissions)) !== JSON.stringify(canonical(expectedApp.permissions)) || JSON.stringify(installation.events ?? []) !== '[]') fail('verified_installation_permissions_invalid', 'verified_readback');

    const repositories = await ghJson([`user/installations/${installation.id}/repositories?per_page=100`]);
    if (repositories?.total_count !== 1 || !Array.isArray(repositories?.repositories) || repositories.repositories.length !== 1) fail('verified_installation_repository_count_invalid', 'verified_readback');
    const installed = repositories.repositories[0];
    if (installed.id !== repository.id || installed.full_name !== REPOSITORY) fail('verified_installation_repository_identity_invalid', 'verified_readback');
    return {
      id: installation.id,
      app_updated_at: app.updated_at,
      account_id: installation.account.id,
      account_login: installation.account.login,
      repository_selection: installation.repository_selection,
      repository_id: installed.id,
      repository_full_name: installed.full_name,
    };
  }
}

function validateConversion(converted, mapping, repository, existingIds) {
  if (!Number.isSafeInteger(converted?.id) || converted.id < 1 || existingIds.has(converted.id)) fail('conversion_app_id_invalid', 'conversion');
  if (converted.slug !== mapping.app_slug || converted.name !== mapping.app_name) fail('conversion_slug_invalid', 'conversion');
  if (converted.owner?.login !== OWNER || converted.owner?.id !== repository.owner.id) fail('conversion_owner_invalid', 'conversion');
  let privateKey;
  try {
    privateKey = createPrivateKey(converted.pem);
  } catch (error) {
    fail('conversion_private_key_invalid', 'conversion', null, error);
  }
  if (privateKey.type !== 'private' || privateKey.asymmetricKeyType !== 'rsa') fail('conversion_private_key_not_rsa', 'conversion');
  const publicPem = createPublicKey(privateKey).export({ type: 'spki', format: 'pem' });
  return { id: converted.id, slug: converted.slug, pem: converted.pem, publicPem };
}

function installUrl(slug, ownerId) {
  return `https://github.com/apps/${encodeURIComponent(slug)}/installations/new/permissions?target_id=${ownerId}`;
}

export class BootstrapCoordinator {
  constructor({ configuration, operations, store, now = Date.now, random = (size) => randomBytes(size) }) {
    this.configuration = configuration;
    this.operations = operations;
    this.store = store;
    this.now = now;
    this.random = random;
    this.document = initialReceipt(configuration, now);
    this.releaseBaseline = null;
    this.repository = null;
    this.index = 0;
    this.pending = null;
    this.current = null;
    this.prepared = false;
    this.mutationsStarted = false;
    this.verifiedNoop = false;
  }

  currentRole() {
    return ROLE_NAMES[this.index] ?? null;
  }

  canRecover() {
    return Boolean(this.current?.pem);
  }

  canRestartCurrent() {
    const receipt = this.document.roles[this.currentRole()];
    return this.prepared && Boolean(this.currentRole()) && receipt?.status === 'pending' && receipt.app_id === null && !this.pending && !this.current;
  }

  canAdoptCurrent() {
    return this.prepared && ['adoption_required', 'reconciliation_required'].includes(this.document.roles[this.currentRole()]?.status);
  }

  async updateReceipt() {
    this.document.updated_at = new Date(this.now()).toISOString();
    await this.store.write(this.document);
  }

  async setSwitchFalse(role) {
    const mapping = this.configuration.mapping.roles[role];
    await this.operations.setVariable(mapping.enabled_variable, 'false');
    const value = await this.operations.getVariable(mapping.enabled_variable);
    if (value !== 'false') fail('switch_readback_not_false', 'disable', role);
    this.document.roles[role].switch = { name: mapping.enabled_variable, value: 'false', readback: true };
  }

  async disableAll({ bestEffort = false } = {}) {
    let firstError = null;
    const failures = [];
    for (const role of ROLE_NAMES) {
      try {
        await this.setSwitchFalse(role);
      } catch (error) {
        if (this.document.roles[role].status !== 'verified') {
          this.document.roles[role].switch = { name: this.configuration.mapping.roles[role].enabled_variable, value: 'unknown', readback: false };
        }
        failures.push({ role, code: safeReason(error?.code, 'disable_failed') });
        firstError ??= error;
      }
    }
    if (firstError && !bestEffort) throw firstError;
    return failures;
  }

  async verifyRulesets({ apply = false } = {}) {
    for (const name of RULESET_NAMES) {
      this.document.rulesets[name].verified = false;
      this.document.rulesets[name].digest = null;
    }
    for (const name of RULESET_NAMES) {
      const expected = this.configuration.rulesets[name];
      if (apply) await this.operations.applyRuleset(expected);
      const live = await this.operations.readRuleset(expected.name);
      const expectedProjection = rulesetProjection(expected);
      if (JSON.stringify(rulesetProjection(live)) !== JSON.stringify(expectedProjection)) fail(`${name}_ruleset_live_drift`, 'ruleset');
      this.document.rulesets[name].verified = true;
      this.document.rulesets[name].digest = evidenceDigest(expectedProjection);
    }
  }

  async verifyVerifiedReceiptLive(document, { roles = ROLE_NAMES, requireVerifiedRelease = true } = {}) {
    const repository = await this.operations.repository();
    if (repository?.full_name !== REPOSITORY || !Number.isSafeInteger(repository?.id) || repository.id < 1 || repository.owner?.login !== OWNER || !Number.isSafeInteger(repository.owner?.id)) fail('repository_identity_invalid', 'verified_readback');

    const release = await this.operations.readEnvironment('release');
    const releaseEvidence = assertReleaseEnvironment(release);
    const releaseDigest = evidenceDigest(release);
    if (requireVerifiedRelease && (releaseEvidence.reviewer_count !== document.release.reviewer_count || releaseDigest !== document.release.start_digest || releaseDigest !== document.release.end_digest)) fail('verified_release_live_drift', 'verified_readback');

    for (const name of RULESET_NAMES) {
      const expected = this.configuration.rulesets[name];
      const live = await this.operations.readRuleset(expected.name);
      const projection = rulesetProjection(expected);
      if (JSON.stringify(rulesetProjection(live)) !== JSON.stringify(projection) || (requireVerifiedRelease && document.rulesets[name].digest !== evidenceDigest(projection))) fail(`${name}_verified_live_drift`, 'verified_readback');
    }

    for (const role of roles) {
      const mapping = this.configuration.mapping.roles[role];
      const receipt = document.roles[role];
      if (await this.operations.getVariable(mapping.enabled_variable) !== 'false') fail(`${role}_verified_switch_live_drift`, 'verified_readback', role);
      if (await this.operations.getVariable(mapping.app_id_variable) !== String(receipt.app_id)) fail(`${role}_verified_app_id_live_drift`, 'verified_readback', role);
      if (await this.operations.getVariable(mapping.app_slug_variable) !== mapping.app_slug) fail(`${role}_verified_app_slug_live_drift`, 'verified_readback', role);
      if (role === 'writer') {
        const livePublicKey = await this.operations.getVariable('AERIS_WRITER_PUBLIC_KEY');
        if (publicKeyDigest(livePublicKey) !== receipt.public_key_variable.value) fail('writer_verified_public_key_live_drift', 'verified_readback', role);
      }

      const secret = await this.operations.getEnvironmentSecret(mapping.environment, mapping.private_key_secret);
      if (secret.name !== receipt.private_key_secret.name || secret.updated_at !== receipt.private_key_secret.updated_at) fail(`${role}_verified_secret_rotated_or_revoked`, 'verified_readback', role);
      const environment = assertAgentEnvironment(await this.operations.readEnvironment(mapping.environment), role);
      if (JSON.stringify(environment) !== JSON.stringify(receipt.environment.evidence)) fail(`${role}_verified_environment_live_drift`, 'verified_readback', role);
      const installation = await this.operations.verifyExistingInstallation(receipt.app_id, repository, mapping);
      if (JSON.stringify(canonical(installation)) !== JSON.stringify(canonical(receipt.installation))) fail(`${role}_verified_installation_live_drift`, 'verified_readback', role);
    }
    this.repository = repository;
  }

  async readReleaseEnd() {
    if (!this.releaseBaseline) return;
    try {
      const end = await this.operations.readEnvironment('release');
      const digest = evidenceDigest(end);
      this.document.release.end_digest = digest;
      this.document.release.checked = true;
      this.document.release.unchanged = digest === this.document.release.start_digest;
    } catch {
      this.document.release.end_digest = null;
      this.document.release.checked = false;
      this.document.release.unchanged = false;
    }
  }

  async recordFailure(error, stage = error?.stage ?? 'bootstrap', role = error?.role ?? this.currentRole()) {
    const normalized = error instanceof BootstrapError ? error : new BootstrapError('unexpected_failure', stage, role, error);
    this.document.disable_failures = this.mutationsStarted ? await this.disableAll({ bestEffort: true }) : [];
    await this.readReleaseEnd();
    this.document.status = 'partial_failed';
    this.document.failure = {
      code: safeReason(normalized.code, 'unexpected_failure'),
      stage: safeReason(normalized.stage ?? stage, 'bootstrap'),
      role: normalized.role ?? role ?? null,
    };
    const failedRole = normalized.role ?? role;
    if (failedRole && this.document.roles[failedRole].status !== 'verified' && !['conversion_started', 'reconciliation_required'].includes(this.document.roles[failedRole].status)) {
      this.document.roles[failedRole].status = 'partial_failed';
    }
    await this.updateReceipt();
    throw normalized;
  }

  async prepare() {
    if (this.prepared) fail('bootstrap_already_prepared', 'prepare');
    // An invalid or configuration-mismatched receipt is evidence: never overwrite it via failure handling.
    const existing = await this.store.load();
    if (existing) this.document = existing;
    if (existing?.status === 'verified') {
      await this.verifyVerifiedReceiptLive(existing);
      this.index = ROLE_NAMES.length;
      this.prepared = true;
      this.verifiedNoop = true;
      return this.document;
    }
    if (existing) {
      const verifiedRoles = ROLE_NAMES.filter((role) => existing.roles[role].status === 'verified');
      await this.verifyVerifiedReceiptLive(existing, { roles: verifiedRoles, requireVerifiedRelease: false });
    }
    try {
      this.releaseBaseline = await this.operations.readEnvironment('release');
      const releaseEvidence = assertReleaseEnvironment(this.releaseBaseline);
      this.document.release.start_digest = evidenceDigest(this.releaseBaseline);
      this.document.release.manual_approval_verified = true;
      this.document.release.reviewer_count = releaseEvidence.reviewer_count;
      this.mutationsStarted = true;
      await this.disableAll();
      this.document.disable_failures = [];

      if (existing) {
        this.document.release = {
          start_digest: evidenceDigest(this.releaseBaseline),
          end_digest: null,
          checked: false,
          unchanged: false,
          manual_approval_verified: true,
          reviewer_count: releaseEvidence.reviewer_count,
        };
        this.index = ROLE_NAMES.findIndex((role) => this.document.roles[role].status !== 'verified');
        if (this.index < 0) fail('receipt_verified_state_inconsistent', 'resume');
      }

      this.repository = await this.operations.repository();
      if (this.repository?.full_name !== REPOSITORY || !Number.isSafeInteger(this.repository?.id) || this.repository.id < 1 || this.repository.owner?.login !== OWNER || !Number.isSafeInteger(this.repository.owner?.id)) fail('repository_identity_invalid', 'prepare');

      await this.verifyRulesets({ apply: true });

      for (const role of ROLE_NAMES) {
        const environment = this.configuration.mapping.roles[role].environment;
        await this.operations.applyEnvironment(environment);
        const live = await this.operations.readEnvironment(environment);
        const evidence = assertAgentEnvironment(live, role);
        this.document.roles[role].environment = { name: environment, verified: true, evidence };
      }
      this.prepared = true;
      const currentReceipt = this.document.roles[this.currentRole()];
      if (currentReceipt?.app_id !== null) {
        currentReceipt.status = 'adoption_required';
        this.document.status = 'in_progress';
        this.document.failure = null;
      } else if (['conversion_started', 'reconciliation_required'].includes(currentReceipt?.status)) {
        currentReceipt.status = 'reconciliation_required';
        currentReceipt.app_slug = this.configuration.mapping.roles[this.currentRole()].app_slug;
        this.document.status = 'in_progress';
        this.document.failure = null;
      } else if (currentReceipt) {
        currentReceipt.status = 'pending';
        currentReceipt.app_slug = null;
        this.document.status = 'in_progress';
        this.document.failure = null;
      }
      await this.updateReceipt();
      return this.document;
    } catch (error) {
      return this.recordFailure(error, 'prepare', null);
    }
  }

  begin(timeoutMs = 10 * 60_000) {
    if (!this.prepared) fail('bootstrap_not_prepared', 'callback');
    if (!this.currentRole()) return null;
    const receipt = this.document.roles[this.currentRole()];
    if (receipt.status !== 'pending' || receipt.app_id !== null) fail('checkpoint_requires_adoption', 'callback', this.currentRole());
    if (this.pending) fail('callback_state_already_outstanding', 'callback', this.currentRole());
    if (this.current) fail('current_role_requires_verification', 'callback', this.currentRole());
    const state = this.random(32).toString('base64url');
    this.pending = { role: this.currentRole(), state, expiresAt: this.now() + timeoutMs };
    const manifest = structuredClone(this.configuration.manifests[this.currentRole()]);
    return { role: this.currentRole(), state, manifest };
  }

  manifestFor(pending, callbackUrl) {
    if (!pending || pending !== this.pending) fail('callback_state_not_current', 'callback');
    const manifest = structuredClone(pending.manifest);
    manifest.redirect_url = callbackUrl;
    return manifest;
  }

  async configureCurrent() {
    const role = this.currentRole();
    const mapping = this.configuration.mapping.roles[role];
    const receipt = this.document.roles[role];
    if (!this.current?.pem) fail('current_private_key_unavailable', 'configuration', role);
    receipt.status = 'configuring';
    await this.setSwitchFalse(role);

    await this.operations.setEnvironmentSecret(mapping.environment, mapping.private_key_secret, this.current.pem);
    const secret = await this.operations.getEnvironmentSecret(mapping.environment, mapping.private_key_secret);
    receipt.private_key_secret = { name: secret.name, present: true, updated_at: secret.updated_at };

    await this.operations.setVariable(mapping.app_id_variable, String(this.current.id));
    const appIdReadback = await this.operations.getVariable(mapping.app_id_variable);
    if (appIdReadback !== String(this.current.id)) fail('app_id_readback_mismatch', 'configuration', role);
    receipt.variables.app_id = { name: mapping.app_id_variable, value: appIdReadback, readback: true };

    await this.operations.setVariable(mapping.app_slug_variable, this.current.slug);
    const slugReadback = await this.operations.getVariable(mapping.app_slug_variable);
    if (slugReadback !== this.current.slug) fail('app_slug_readback_mismatch', 'configuration', role);
    receipt.variables.app_slug = { name: mapping.app_slug_variable, value: slugReadback, readback: true };

    if (role === 'writer') {
      await this.operations.setVariable('AERIS_WRITER_PUBLIC_KEY', this.current.publicPem);
      const publicKeyReadback = await this.operations.getVariable('AERIS_WRITER_PUBLIC_KEY');
      if (publicKeyReadback.replace(/\r\n/g, '\n').trim() !== this.current.publicPem.replace(/\r\n/g, '\n').trim()) fail('writer_public_key_readback_mismatch', 'configuration', role);
      receipt.public_key_variable = { name: 'AERIS_WRITER_PUBLIC_KEY', value: publicKeyDigest(publicKeyReadback), readback: true };
    }

    const liveEnvironment = await this.operations.readEnvironment(mapping.environment);
    receipt.environment = { name: mapping.environment, verified: true, evidence: assertAgentEnvironment(liveEnvironment, role) };
    await this.setSwitchFalse(role);
    receipt.status = 'awaiting_install';
    this.document.status = 'in_progress';
    this.document.failure = null;
    this.document.disable_failures = [];
    await this.updateReceipt();
  }

  async consume({ state, code }) {
    const pending = this.pending;
    if (pending && pending.expiresAt < this.now()) {
      this.pending = null;
      fail('callback_state_expired', 'callback', this.currentRole());
    }
    if (!pending || typeof state !== 'string' || state !== pending.state) fail('callback_state_invalid', 'callback', this.currentRole());
    if (typeof code !== 'string' || !/^[A-Za-z0-9_-]{8,512}$/.test(code)) fail('callback_code_invalid', 'callback', pending.role);
    this.pending = null;
    const role = pending.role;
    try {
      const mapping = this.configuration.mapping.roles[role];
      this.document.roles[role].status = 'conversion_started';
      this.document.roles[role].app_id = null;
      this.document.roles[role].app_slug = mapping.app_slug;
      this.document.status = 'in_progress';
      this.document.failure = null;
      this.document.disable_failures = [];
      await this.updateReceipt();
      const converted = await this.operations.convertManifest(code);
      const existingIds = new Set(ROLE_NAMES.map((name) => this.document.roles[name].app_id).filter(Boolean));
      this.current = validateConversion(converted, mapping, this.repository, existingIds);
      this.document.roles[role].app_id = this.current.id;
      this.document.roles[role].app_slug = this.current.slug;
      this.document.roles[role].status = 'converted';
      this.document.status = 'in_progress';
      this.document.failure = null;
      await this.updateReceipt();
      await this.configureCurrent();
      return { role, install_url: installUrl(this.current.slug, this.repository.owner.id), repository: REPOSITORY };
    } catch (error) {
      return this.recordFailure(error, error?.stage ?? 'conversion', role);
    }
  }

  async recover() {
    const role = this.currentRole();
    if (!this.current?.pem) return this.recordFailure(new BootstrapError('cross_process_pem_unavailable', 'resume', role), 'resume', role);
    try {
      await this.configureCurrent();
      return { role, install_url: installUrl(this.current.slug, this.repository.owner.id), repository: REPOSITORY };
    } catch (error) {
      return this.recordFailure(error, error?.stage ?? 'configuration', role);
    }
  }

  async adoptCurrent({ appId, appSlug, pem }) {
    const role = this.currentRole();
    const receipt = this.document.roles[role];
    if (!this.prepared || !role || !['adoption_required', 'reconciliation_required'].includes(receipt.status)) fail('role_not_ready_for_adoption', 'adoption', role);
    try {
      const reconciliation = receipt.status === 'reconciliation_required';
      if (!Number.isSafeInteger(appId) || appId < 1 || appSlug !== this.configuration.mapping.roles[role].app_slug) fail('adoption_checkpoint_mismatch', 'adoption', role);
      if (!reconciliation && appId !== receipt.app_id) fail('adoption_checkpoint_mismatch', 'adoption', role);
      const existingIds = new Set(ROLE_NAMES.filter((name) => name !== role).map((name) => this.document.roles[name].app_id).filter(Boolean));
      const candidate = validateConversion({
        id: appId,
        slug: appSlug,
        name: this.configuration.mapping.roles[role].app_name,
        owner: { id: this.repository.owner.id, login: OWNER },
        pem,
      }, this.configuration.mapping.roles[role], this.repository, existingIds);
      await this.operations.verifyAppIdentity(candidate.id, candidate.pem, this.repository, this.configuration.mapping.roles[role]);
      this.current = candidate;
      receipt.app_id = this.current.id;
      receipt.app_slug = this.current.slug;
      receipt.status = 'converted';
      this.document.status = 'in_progress';
      this.document.failure = null;
      this.document.disable_failures = [];
      await this.updateReceipt();
      await this.configureCurrent();
      return { role, install_url: installUrl(this.current.slug, this.repository.owner.id), repository: REPOSITORY };
    } catch (error) {
      return this.recordFailure(error, error?.stage ?? 'adoption', role);
    }
  }

  async verifyCurrent() {
    const role = this.currentRole();
    if (!this.current?.pem) return this.recordFailure(new BootstrapError('cross_process_pem_unavailable', 'verification', role), 'verification', role);
    if (this.document.roles[role].status !== 'awaiting_install') fail('role_not_ready_for_installation_verification', 'verification', role);
    try {
      const mapping = this.configuration.mapping.roles[role];
      const receipt = this.document.roles[role];
      const installation = await this.operations.verifyInstallation(this.current.id, this.current.pem, this.repository, mapping);
      if (installation.account_id !== this.repository.owner.id || installation.account_login !== OWNER || installation.repository_selection !== 'selected' || installation.repository_id !== this.repository.id || installation.repository_full_name !== REPOSITORY) fail('installation_evidence_invalid', 'verification', role);
      receipt.installation = installation;
      receipt.pem_probe = { verified: true, app_updated_at: installation.app_updated_at };

      await this.setSwitchFalse(role);
      const secret = await this.operations.getEnvironmentSecret(mapping.environment, mapping.private_key_secret);
      if (secret.name !== receipt.private_key_secret.name || secret.updated_at !== receipt.private_key_secret.updated_at) fail('private_key_secret_rotated_during_bootstrap', 'verification', role);
      if (await this.operations.getVariable(mapping.app_id_variable) !== String(this.current.id)) fail('app_id_readback_mismatch', 'verification', role);
      if (await this.operations.getVariable(mapping.app_slug_variable) !== this.current.slug) fail('app_slug_readback_mismatch', 'verification', role);
      receipt.variables.app_id.readback = true;
      receipt.variables.app_slug.readback = true;
      const liveEnvironment = await this.operations.readEnvironment(mapping.environment);
      receipt.environment = { name: mapping.environment, verified: true, evidence: assertAgentEnvironment(liveEnvironment, role) };

      await this.verifyRulesets();

      await this.readReleaseEnd();
      if (!this.document.release.checked || !this.document.release.unchanged) fail('release_environment_drift', 'release', role);

      receipt.status = 'verified';
      this.current.pem = null;
      this.current.publicPem = null;
      this.current = null;
      this.index += 1;
      this.document.failure = null;
      if (this.index === ROLE_NAMES.length) {
        this.document.status = 'verified';
        assertReceiptDocument(this.document, this.configuration);
      } else {
        this.document.status = 'in_progress';
      }
      await this.updateReceipt();
      return { role, complete: this.index === ROLE_NAMES.length, next_role: this.currentRole() };
    } catch (error) {
      return this.recordFailure(error, error?.stage ?? 'verification', role);
    }
  }
}

export class CapabilityGate {
  constructor({ random = (size) => randomBytes(size), now = Date.now, ttlMs = 10 * 60_000 } = {}) {
    this.random = random;
    this.now = now;
    this.ttlMs = ttlMs;
    this.pending = null;
  }

  issue(purpose) {
    if (this.pending) fail('capability_already_outstanding', 'csrf');
    const capability = this.random(32).toString('base64url');
    this.pending = { digest: evidenceDigest(capability), purpose, expiresAt: this.now() + this.ttlMs };
    return capability;
  }

  consume(capability, purpose) {
    const pending = this.pending;
    if (!pending || pending.purpose !== purpose || pending.expiresAt < this.now() || typeof capability !== 'string' || evidenceDigest(capability) !== pending.digest) fail('capability_invalid', 'csrf');
    this.pending = null;
  }
}

export class SessionTransitionLock {
  constructor() {
    this.busy = false;
  }

  async run(callback) {
    if (this.busy) fail('bootstrap_transition_busy', 'session_lock');
    this.busy = true;
    try {
      return await callback();
    } finally {
      this.busy = false;
    }
  }
}

export function validateLoopbackRequest(request, expectedHost, kind) {
  if (request.headers.host !== expectedHost) fail('loopback_host_invalid', 'csrf');
  const origin = request.headers.origin;
  if (origin && origin !== `http://${expectedHost}`) fail('loopback_origin_invalid', 'csrf');
  const site = request.headers['sec-fetch-site'];
  if (kind === 'local' && site && !['none', 'same-origin'].includes(site)) fail('loopback_fetch_site_invalid', 'csrf');
  if (kind === 'callback' && site && !['cross-site', 'none', 'same-origin'].includes(site)) fail('callback_fetch_site_invalid', 'csrf');
}

function htmlEscape(value) {
  return String(value).replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;').replaceAll('"', '&quot;').replaceAll("'", '&#39;');
}

function manifestPage(manifest, state) {
  const escapedManifest = htmlEscape(JSON.stringify(manifest).replace(/</g, '\\u003c'));
  return `<!doctype html><meta charset="utf-8"><title>Aeris App bootstrap</title><form id="manifest" method="post" action="${manifestEndpoint}"><input type="hidden" name="manifest" value='${escapedManifest}'><input type="hidden" name="state" value="${htmlEscape(state)}"></form><script>document.getElementById('manifest').submit()</script>`;
}

function linkPage(message, links) {
  return `<!doctype html><meta charset="utf-8"><title>Aeris App bootstrap</title><p>${htmlEscape(message)}</p>${links.map(({ href, label }) => `<p><a href="${htmlEscape(href)}">${htmlEscape(label)}</a></p>`).join('')}`;
}

function adoptionPage(role, appId, appSlug, capability) {
  const description = appId === null
    ? `Reconcile the result-unknown ${htmlEscape(role)} conversion for expected slug ${htmlEscape(appSlug)}.`
    : `Adopt checkpointed ${htmlEscape(role)} App ${htmlEscape(appSlug)} (${htmlEscape(appId)}).`;
  const appIdInput = appId === null
    ? '<p><label>Created App ID <input name="app_id" required inputmode="numeric" pattern="[1-9][0-9]*"></label></p>'
    : '';
  return `<!doctype html><meta charset="utf-8"><title>Aeris App adoption</title><p>${description}</p><form method="post" action="/adopt"><input type="hidden" name="cap" value="${htmlEscape(capability)}">${appIdInput}<label>Downloaded private key (PEM)<br><textarea name="pem" required rows="18" cols="80"></textarea></label><p><button type="submit">Reconcile App</button></p></form>`;
}

async function readForm(request, maximumBytes = 128 * 1024) {
  if (!String(request.headers['content-type'] ?? '').toLowerCase().startsWith('application/x-www-form-urlencoded')) fail('adoption_content_type_invalid', 'adoption');
  const chunks = [];
  let total = 0;
  for await (const chunk of request) {
    total += chunk.length;
    if (total > maximumBytes) fail('adoption_body_too_large', 'adoption');
    chunks.push(chunk);
  }
  return new URLSearchParams(Buffer.concat(chunks).toString('utf8'));
}

export async function serve({ port, configuration, operations, store }) {
  if (!Number.isInteger(port) || port < 1024 || port > 65535) fail('bootstrap_port_invalid', 'server');
  await store.acquireExclusiveLock();
  let retainLockForServer = false;
  try {
    const coordinator = new BootstrapCoordinator({ configuration, operations, store });
    await coordinator.prepare();
    if (coordinator.verifiedNoop) {
      process.stdout.write('Bootstrap live readback verified the exact current configuration; no GitHub mutation performed.\n');
      return null;
    }
    const callbackUrl = `http://127.0.0.1:${port}/callback`;
    const expectedHost = `127.0.0.1:${port}`;
    const gate = new CapabilityGate();
    const transitionLock = new SessionTransitionLock();
    const initialPurpose = coordinator.canAdoptCurrent() ? 'adopt' : 'init';
    const initialCapability = gate.issue(initialPurpose);
    const server = createServer(async (request, response) => {
    const url = new URL(request.url, callbackUrl);
    response.setHeader('cache-control', 'no-store');
    response.setHeader('content-security-policy', "default-src 'none'; form-action 'self' https://github.com; script-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'");
    response.setHeader('referrer-policy', 'no-referrer');
    try {
      await transitionLock.run(async () => {
      if (request.method === 'GET' && url.pathname === '/init') {
        validateLoopbackRequest(request, expectedHost, 'local');
        gate.consume(url.searchParams.get('cap'), 'init');
        const pending = coordinator.begin();
        const manifest = coordinator.manifestFor(pending, callbackUrl);
        response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
        response.end(manifestPage(manifest, pending.state));
        return;
      }
      if (request.method === 'GET' && url.pathname === '/adopt') {
        validateLoopbackRequest(request, expectedHost, 'local');
        gate.consume(url.searchParams.get('cap'), 'adopt');
        if (!coordinator.canAdoptCurrent()) fail('role_not_ready_for_adoption', 'adoption', coordinator.currentRole());
        const role = coordinator.currentRole();
        const receipt = coordinator.document.roles[role];
        const submitCapability = gate.issue('adopt-submit');
        response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
        response.end(adoptionPage(role, receipt.app_id, receipt.app_slug, submitCapability));
        return;
      }
      if (request.method === 'POST' && url.pathname === '/adopt') {
        validateLoopbackRequest(request, expectedHost, 'local');
        const form = await readForm(request);
        gate.consume(form.get('cap'), 'adopt-submit');
        const role = coordinator.currentRole();
        const receipt = coordinator.document.roles[role];
        const suppliedAppId = receipt.app_id ?? Number(form.get('app_id'));
        const result = await coordinator.adoptCurrent({ appId: suppliedAppId, appSlug: receipt.app_slug, pem: form.get('pem') });
        const verifyCapability = gate.issue('verify');
        response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
        response.end(linkPage(`Adopted ${result.role} configuration.`, [
          { href: result.install_url, label: 'Install App' },
          { href: `/verify?cap=${verifyCapability}`, label: 'Verify installation' },
        ]));
        return;
      }
      if (request.method === 'GET' && url.pathname === '/callback') {
        validateLoopbackRequest(request, expectedHost, 'callback');
        const result = await coordinator.consume({ state: url.searchParams.get('state'), code: url.searchParams.get('code') });
        const verifyCapability = gate.issue('verify');
        response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
        response.end(linkPage(`Install ${result.role} for selected repository ${result.repository} only.`, [
          { href: result.install_url, label: 'Install App' },
          { href: `/verify?cap=${verifyCapability}`, label: 'Verify installation' },
        ]));
        return;
      }
      if (request.method === 'GET' && url.pathname === '/recover') {
        validateLoopbackRequest(request, expectedHost, 'local');
        gate.consume(url.searchParams.get('cap'), 'recover');
        const result = await coordinator.recover();
        const verifyCapability = gate.issue('verify');
        response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
        response.end(linkPage(`Recovered ${result.role} configuration.`, [
          { href: result.install_url, label: 'Install App' },
          { href: `/verify?cap=${verifyCapability}`, label: 'Verify installation' },
        ]));
        return;
      }
      if (request.method === 'GET' && url.pathname === '/verify') {
        validateLoopbackRequest(request, expectedHost, 'local');
        gate.consume(url.searchParams.get('cap'), 'verify');
        const result = await coordinator.verifyCurrent();
        const links = [];
        if (!result.complete) {
          const nextCapability = gate.issue('init');
          links.push({ href: `/init?cap=${nextCapability}`, label: `Create ${result.next_role}` });
        }
        response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
        response.end(linkPage(result.complete ? 'Bootstrap verified and complete.' : `${result.role} verified.`, links));
        return;
      }
      response.writeHead(404).end();
      });
    } catch (error) {
      const links = [];
      const transitionBusy = error instanceof BootstrapError && error.code === 'bootstrap_transition_busy';
      if (!transitionBusy && coordinator.canRecover() && !gate.pending) {
        const recoveryCapability = gate.issue('recover');
        links.push({ href: `/recover?cap=${recoveryCapability}`, label: 'Retry configuration in this process' });
      } else if (!transitionBusy && coordinator.canRestartCurrent() && !gate.pending) {
        const retryCapability = gate.issue('init');
        links.push({ href: `/init?cap=${retryCapability}`, label: `Retry ${coordinator.currentRole()} creation` });
      }
      response.writeHead(transitionBusy ? 409 : 400, { 'content-type': 'text/html; charset=utf-8' });
      response.end(linkPage(`Bootstrap stopped safely: ${error instanceof BootstrapError ? error.code : 'unexpected_failure'}.`, links));
    }
    });
    await new Promise((resolve, reject) => {
      server.once('error', reject);
      server.listen(port, '127.0.0.1', resolve);
    });
    server.once('close', () => {
      void store.releaseExclusiveLock().catch((error) => {
        process.stderr.write(`Bootstrap lock release failed closed: ${safeReason(error?.code, 'unexpected_failure')}.\n`);
      });
    });
    retainLockForServer = true;
    process.stdout.write(`Apply mode ready. Open once: http://127.0.0.1:${port}/${initialPurpose}?cap=${initialCapability}\n`);
    return server;
  } finally {
    if (!retainLockForServer) await store.releaseExclusiveLock();
  }
}

export async function dryRun() {
  const configuration = await loadConfiguration();
  return ROLE_NAMES.map((role) => ({ role, ...configuration.mapping.roles[role] }));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = new Set(process.argv.slice(2));
  if (args.size === 1 && args.has('--dry-run')) {
    process.stdout.write(`${JSON.stringify(await dryRun(), null, 2)}\n`);
  } else if (args.size === 2 && args.has('--serve') && args.has('--apply')) {
    const configuration = await loadConfiguration();
    const operations = new GhGitHubOperations();
    const store = new ReceiptStore({ configuration });
    await serve({ port: Number(process.env.AERIS_BOOTSTRAP_PORT ?? 8791), configuration, operations, store });
  } else {
    fail('usage_dry_run_or_explicit_apply_required', 'cli');
  }
}
