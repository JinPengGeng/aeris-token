import fs from 'node:fs';
import path from 'node:path';

import { runWriterAnalyze, runWriterBuild, runWriterPreflight, runWriterPublish } from './writer-activation.mjs';

const [phase] = process.argv.slice(2);
if (!['preflight', 'analyze', 'build', 'publish'].includes(phase)) throw new Error('invalid Writer phase');
const repoRoot = process.env.GITHUB_WORKSPACE ?? process.cwd();
const read = () => JSON.parse(fs.readFileSync(process.env.AERIS_INPUT_PATH, 'utf8'));
let artifact;
if (phase === 'preflight') {
  artifact = await runWriterPreflight({ event: JSON.parse(fs.readFileSync(process.env.GITHUB_EVENT_PATH, 'utf8')), environment: process.env, repoRoot });
} else if (phase === 'analyze') {
  artifact = await runWriterAnalyze({ artifact: read(), environment: process.env, repoRoot });
} else if (phase === 'build') {
  artifact = runWriterBuild({ artifact: read(), repoRoot });
} else {
  artifact = await runWriterPublish({ artifact: read(), environment: process.env, repoRoot });
}
fs.mkdirSync(path.dirname(process.env.AERIS_OUTPUT_PATH), { recursive: true, mode: 0o700 });
fs.writeFileSync(process.env.AERIS_OUTPUT_PATH, `${JSON.stringify(artifact)}\n`, { encoding: 'utf8', mode: 0o600 });
