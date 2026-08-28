import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import { AutonomyPolicyGitHubClient, evaluateAutonomyPolicy } from './autonomy-policy-runtime.mjs';

const SHA = /^[0-9a-f]{40}$/;
const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const APP_SLUG = /^[a-z0-9][a-z0-9-]{0,99}$/;

export class AutonomyPolicyGateError extends Error {
  constructor(message) {
    super(message);
    this.name = 'AutonomyPolicyGateError';
  }
}

function reject(message) {
  throw new AutonomyPolicyGateError(message);
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

function enabled(value, name) {
  if (value === undefined || value === '' || value === '0' || value === 'false') return false;
  if (value === '1' || value === 'true') return true;
  reject(`${name} must be true, false, 1, or 0`);
}

export function policyConfigFromEnvironment(environment) {
  const writerEnabled = enabled(environment.AERIS_WRITER_ENABLED, 'AERIS_WRITER_ENABLED');
  const slug = environment.AERIS_WRITER_APP_SLUG ?? '';
  if (writerEnabled && !APP_SLUG.test(slug)) reject('AERIS_WRITER_APP_SLUG is required while Writer is enabled');
  if (slug !== '' && !APP_SLUG.test(slug)) reject('AERIS_WRITER_APP_SLUG format is invalid');
  return Object.freeze({
    repository: required(environment.GITHUB_REPOSITORY, 'GITHUB_REPOSITORY', REPOSITORY),
    base_ref: required(environment.AERIS_DEFAULT_BRANCH, 'AERIS_DEFAULT_BRANCH'),
    writer_login: `${slug || 'aeris-disabled-writer'}[bot]`,
    branch_prefix: 'agent/issue-',
    maximum_files: 20,
    maximum_changes: 2000,
  });
}

export async function runAutonomyPolicyGate(environment = process.env, dependencies = {}) {
  const config = policyConfigFromEnvironment(environment);
  if (config.base_ref !== 'main') reject('AERIS_DEFAULT_BRANCH must be main');
  const trigger = Object.freeze({
    pull_number: positiveInteger(environment.AERIS_PULL_NUMBER, 'AERIS_PULL_NUMBER'),
    head_sha: required(environment.AERIS_HEAD_SHA, 'AERIS_HEAD_SHA', SHA),
  });
  const trust = Object.freeze({
    repository: config.repository,
    repository_id: positiveInteger(environment.GITHUB_REPOSITORY_ID, 'GITHUB_REPOSITORY_ID'),
    default_branch: config.base_ref,
    policy_ref: required(environment.AERIS_POLICY_REF, 'AERIS_POLICY_REF'),
    policy_sha: required(environment.AERIS_POLICY_SHA, 'AERIS_POLICY_SHA', SHA),
  });
  const client = dependencies.client ?? new AutonomyPolicyGitHubClient({
    token: required(environment.GITHUB_TOKEN, 'GITHUB_TOKEN'),
    repository: config.repository,
    apiUrl: environment.GITHUB_API_URL,
  });
  const evaluatePolicy = dependencies.evaluatePolicy ?? evaluateAutonomyPolicy;
  const result = await evaluatePolicy({ client, trigger, trust, config });
  const reasons = result.decision.reasons.join(',');
  if (environment.GITHUB_OUTPUT) {
    fs.appendFileSync(environment.GITHUB_OUTPUT, `classification=${result.decision.classification}\nreasons=${reasons}\n`);
  }
  if (environment.GITHUB_STEP_SUMMARY) {
    fs.appendFileSync(
      environment.GITHUB_STEP_SUMMARY,
      `### Automation Policy\n\n- Classification: \`${result.decision.classification}\`\n- Head: \`${result.snapshot.head.sha}\`\n- Reasons: \`${reasons || 'none'}\`\n`,
    );
  }
  if (result.decision.classification === 'deny') reject(`autonomy policy denied the pull request: ${reasons || 'unspecified'}`);
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  await runAutonomyPolicyGate();
}
