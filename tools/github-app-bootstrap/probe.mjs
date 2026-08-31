#!/usr/bin/env node
import { fileURLToPath } from 'node:url';
import {
  BootstrapError,
  GhGitHubOperations,
  ROLE_NAMES,
  loadConfiguration,
} from './bootstrap.mjs';

export async function probeActivation({ role, appId, pem, configuration, operations }) {
  if (!ROLE_NAMES.includes(role)) throw new BootstrapError('activation_probe_role_invalid', 'activation_probe');
  if (!Number.isSafeInteger(appId) || appId < 1) throw new BootstrapError('activation_probe_app_id_invalid', 'activation_probe', role);
  if (typeof pem !== 'string' || pem.length === 0) throw new BootstrapError('activation_probe_private_key_missing', 'activation_probe', role);
  const expected = configuration.mapping.roles[role];
  const evidence = await operations.probeInstallation(appId, pem, expected);
  return { role, ...evidence };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  if (args.length !== 2 || args[0] !== '--role' || !ROLE_NAMES.includes(args[1])) {
    throw new BootstrapError('usage_role_required', 'activation_probe');
  }
  const configuration = await loadConfiguration();
  const result = await probeActivation({
    role: args[1],
    appId: Number(process.env.AERIS_APP_ID),
    pem: process.env.AERIS_APP_PRIVATE_KEY,
    configuration,
    operations: new GhGitHubOperations(),
  });
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
}
