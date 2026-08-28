import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import {
  AutonomyFinalizerGitHubClient,
  validateBranchProtection,
  validateWriterGovernanceSnapshot,
} from './autonomy-finalizer.mjs';

const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const RFC3339_TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}:\d{2})$/;

export class WriterGovernanceCanaryError extends Error {
  constructor(message) {
    super(message);
    this.name = 'WriterGovernanceCanaryError';
  }
}

function reject(message) {
  throw new WriterGovernanceCanaryError(message);
}

function required(value, name, pattern = null) {
  if (typeof value !== 'string' || value.length === 0 || /[\u0000-\u001f\u007f]/.test(value)) {
    reject(`${name} is invalid`);
  }
  if (pattern && !pattern.test(value)) reject(`${name} format is invalid`);
  return value;
}

function positiveInteger(value, name) {
  const parsed = typeof value === 'string' && /^[1-9][0-9]*$/.test(value) ? Number(value) : value;
  if (!Number.isSafeInteger(parsed) || parsed <= 0) reject(`${name} must be a positive integer`);
  return parsed;
}

function timestamp(value, name) {
  const normalized = required(value, name, RFC3339_TIMESTAMP);
  if (!Number.isFinite(Date.parse(normalized))) reject(`${name} is invalid`);
  return normalized;
}

function exactSnapshot(value) {
  return JSON.stringify(value);
}

async function readValidatedSnapshot(client, context) {
  if (typeof client?.getBranchProtection !== 'function' ||
      typeof client?.readWriterGovernanceSnapshotOnce !== 'function') {
    reject('Writer governance canary client cannot read the full governance snapshot');
  }
  const [classicProof, writerSnapshot] = await Promise.all([
    client.getBranchProtection(),
    client.readWriterGovernanceSnapshotOnce(context.writerTrust),
  ]);
  const classic = validateBranchProtection(classicProof, context.trust.default_branch);
  const writer = validateWriterGovernanceSnapshot(writerSnapshot, {
    ...context,
    classicProtection: classic,
  });
  return Object.freeze({
    classic,
    snapshot: writer.snapshot,
    fence: writer.fence,
    secret_lane: writer.secret_lane,
  });
}

export async function proveWriterGovernanceCanary(client, context) {
  const initial = await readValidatedSnapshot(client, context);
  const confirmed = await readValidatedSnapshot(client, context);
  if (exactSnapshot(initial) !== exactSnapshot(confirmed)) {
    reject('Writer governance drifted between complete classic and REST reads');
  }
  return confirmed;
}

function publicSummary(proof, trust, writerTrust) {
  const snapshotJson = exactSnapshot(proof);
  return Object.freeze({
    schema_version: 1,
    repository: trust.repository,
    repository_id: trust.repository_id,
    default_branch: trust.default_branch,
    classic_profile: proof.classic.profile,
    required_contexts: proof.classic.contexts,
    ruleset_inventory_count: proof.classic.rulesets,
    governance_profile: proof.fence.profile,
    governance_fence_ruleset_id: proof.fence.ruleset_id,
    trusted_owner_login: proof.fence.trusted_owner_login,
    trusted_owner_database_id: proof.fence.trusted_owner_database_id,
    writer_app_id: proof.fence.app_id,
    writer_app_slug: proof.fence.app_slug,
    governance_fence_updated_at: writerTrust.governance_fence_updated_at,
    secret_lane_profile: proof.secret_lane.profile,
    writer_environment: proof.secret_lane.environment,
    snapshot_sha256: crypto.createHash('sha256').update(snapshotJson, 'utf8').digest('hex'),
  });
}

export async function runWriterGovernanceCanary(environment = process.env, dependencies = {}) {
  const repository = required(environment.GITHUB_REPOSITORY, 'GITHUB_REPOSITORY', REPOSITORY);
  const trust = Object.freeze({
    repository,
    repository_id: positiveInteger(environment.GITHUB_REPOSITORY_ID, 'GITHUB_REPOSITORY_ID'),
    default_branch: required(environment.AERIS_DEFAULT_BRANCH, 'AERIS_DEFAULT_BRANCH'),
  });
  const writerTrust = Object.freeze({
    proof_app_id: positiveInteger(environment.AERIS_WRITER_APP_ID, 'AERIS_WRITER_APP_ID'),
    proof_app_slug: required(environment.AERIS_WRITER_APP_SLUG, 'AERIS_WRITER_APP_SLUG'),
    proof_app_owner_login: required(
      environment.AERIS_WRITER_APP_OWNER_LOGIN,
      'AERIS_WRITER_APP_OWNER_LOGIN',
    ),
    proof_app_owner_database_id: positiveInteger(
      environment.AERIS_WRITER_APP_OWNER_DATABASE_ID,
      'AERIS_WRITER_APP_OWNER_DATABASE_ID',
    ),
    governance_fence_ruleset_id: positiveInteger(
      environment.AERIS_WRITER_GOVERNANCE_FENCE_RULESET_ID,
      'AERIS_WRITER_GOVERNANCE_FENCE_RULESET_ID',
    ),
    governance_fence_updated_at: timestamp(
      environment.AERIS_WRITER_GOVERNANCE_FENCE_UPDATED_AT,
      'AERIS_WRITER_GOVERNANCE_FENCE_UPDATED_AT',
    ),
  });
  const client = dependencies.client ?? new AutonomyFinalizerGitHubClient({
    token: required(environment.AERIS_WRITER_TOKEN, 'AERIS_WRITER_TOKEN'),
    repository,
    apiUrl: environment.GITHUB_API_URL,
  });
  const proof = await proveWriterGovernanceCanary(client, { trust, writerTrust });
  const result = publicSummary(proof, trust, writerTrust);
  if (environment.GITHUB_OUTPUT) {
    fs.appendFileSync(environment.GITHUB_OUTPUT, [
      `snapshot_sha256=${result.snapshot_sha256}`,
      `ruleset_id=${result.governance_fence_ruleset_id}`,
      `snapshot_summary=${JSON.stringify(result)}`,
      '',
    ].join('\n'));
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  await runWriterGovernanceCanary();
}
