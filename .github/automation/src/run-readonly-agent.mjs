import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { runAutomation } from './engine.mjs';

const sourceDirectory = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(sourceDirectory, '..', '..', '..');
const event = JSON.parse(fs.readFileSync(process.env.GITHUB_EVENT_PATH, 'utf8'));
const kind = process.env.AERIS_OBJECT_KIND;

if (!['issue', 'pull_request'].includes(kind)) {
  throw new Error('AERIS_OBJECT_KIND must be issue or pull_request');
}

await runAutomation({
  kind: kind === 'pull_request' ? 'pull' : kind,
  eventName: process.env.GITHUB_EVENT_NAME,
  event,
  environment: process.env,
  repoRoot,
});
