import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const yaml = require('js-yaml');
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');

test('release Docker Hub classifier receives its repository variable under nounset', () => {
  const workflow = yaml.load(fs.readFileSync(path.join(root, '.github/workflows/release.yml'), 'utf8'));
  const step = workflow.jobs.publish.steps.find(
    (candidate) => candidate.name === 'Classify optional Docker Hub publication',
  );
  assert.ok(step);
  assert.equal(step.env.DOCKERHUB_IMAGE, '${{ vars.DOCKERHUB_IMAGE }}');
  assert.match(step.run, /\$\{DOCKERHUB_IMAGE\}/);
});
