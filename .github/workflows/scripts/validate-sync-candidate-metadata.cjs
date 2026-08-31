#!/usr/bin/env node
'use strict';

const fs = require('node:fs');

function fail(message) {
  process.stderr.write(`invalid sync candidate metadata: ${message}\n`);
  process.exit(1);
}

function read(path, label) {
  try {
    return fs.readFileSync(path, 'utf8');
  } catch (error) {
    fail(`unable to read ${label}: ${error.message}`);
  }
}

function exactMatches(text, expression, label) {
  const matches = [...text.matchAll(expression)];
  if (matches.length !== 1) fail(`${label} must appear exactly once`);
  return matches[0][1] ?? '';
}

const [prPath, messagePath, expectedRepository, expectedBaseBranch, expectedHeadBranch,
  expectedHead, syncAppSlug, expectedAuthorId, expectedAuthorType,
  commitAuthor, commitCommitter, parentCount, actualParent] =
  process.argv.slice(2);

if (process.argv.length !== 15) fail('unexpected argument count');
if (!/^[A-Za-z0-9][A-Za-z0-9._-]*\/[A-Za-z0-9][A-Za-z0-9._-]*$/.test(expectedRepository)) {
  fail('expected repository is invalid');
}
for (const [label, value] of [['expected head', expectedHead], ['actual parent', actualParent]]) {
  if (!/^[0-9a-f]{40}$/.test(value)) fail(`${label} is not a full lowercase SHA`);
}
if (!/^[a-z0-9][a-z0-9-]{0,99}$/.test(syncAppSlug)) fail('Writer App slug is invalid');
if (!/^[1-9][0-9]*$/.test(expectedAuthorId) || expectedAuthorType !== 'Bot') {
  fail('trusted Writer App bot identity is invalid');
}

let pr;
try {
  pr = JSON.parse(read(prPath, 'pull request response'));
} catch (error) {
  fail(`pull request response is not valid JSON: ${error.message}`);
}
if (pr === null || Array.isArray(pr) || typeof pr !== 'object') {
  fail('pull request response must be one complete object');
}
if (pr.truncated === true || pr.incomplete_results === true) {
  fail('pull request response is truncated');
}

const required = [
  ['number', pr.number],
  ['state', pr.state],
  ['draft', pr.draft],
  ['author', pr.user?.login],
  ['author id', pr.user?.id],
  ['author type', pr.user?.type],
  ['base ref', pr.base?.ref],
  ['base SHA', pr.base?.sha],
  ['base repository', pr.base?.repo?.full_name],
  ['head ref', pr.head?.ref],
  ['head SHA', pr.head?.sha],
  ['head repository', pr.head?.repo?.full_name],
  ['body', pr.body],
];
for (const [label, value] of required) {
  if (value === undefined || value === null) fail(`pull request ${label} is missing`);
}
if (!Number.isSafeInteger(pr.number) || pr.number < 1) fail('pull request number is invalid');
if (pr.state !== 'open' || pr.draft !== false) fail('pull request must be open and non-draft');
if (pr.base.ref !== expectedBaseBranch || pr.base.repo.full_name !== expectedRepository) {
  fail('pull request base identity is untrusted');
}
if (pr.head.ref !== expectedHeadBranch || pr.head.repo.full_name !== expectedRepository) {
  fail('pull request head identity is untrusted');
}
if (pr.head.sha !== expectedHead) fail('pull request head drifted from the expected candidate');
if (!/^[0-9a-f]{40}$/.test(pr.base.sha)) fail('pull request base SHA is invalid');

if (pr.user.login !== `${syncAppSlug}[bot]` ||
    String(pr.user.id) !== expectedAuthorId || pr.user.type !== expectedAuthorType) {
  fail('pull request author is not the exact trusted Writer App bot identity');
}

const body = pr.body;
const managedCount = [...body.matchAll(/^<!-- upstream-sync-managed -->\r?$/gm)].length;
if (managedCount !== 1) fail('managed marker must appear exactly once');
if ([...body.matchAll(/^<!-- upstream-sync-owned-tip:.*-->\r?$/gm)].length !== 1) {
  fail('owned-tip marker must appear exactly once');
}
const ownedTip = exactMatches(
  body,
  /^<!-- upstream-sync-owned-tip:([0-9a-f]{40}) -->\r?$/gm,
  'owned-tip marker',
);
if ([...body.matchAll(/^<!-- upstream-sync-source:.*-->\r?$/gm)].length !== 1) {
  fail('source marker must appear exactly once');
}
const bodySource = exactMatches(
  body,
  /^<!-- upstream-sync-source:([A-Za-z0-9][A-Za-z0-9._-]*\/[A-Za-z0-9][A-Za-z0-9._-]*@[0-9a-f]{40}) -->\r?$/gm,
  'source marker',
);
if (ownedTip !== expectedHead) fail('owned-tip marker does not match the PR head');

const message = read(messagePath, 'commit message');
const trailerNames = ['Automation', 'Source', 'Checkpoint', 'Base'];
const trailers = {};
for (const name of trailerNames) {
  const expression = new RegExp(`^Sync-Upstream-${name}: (.*)\\r?$`, 'gmi');
  const matches = [...message.matchAll(expression)];
  if (matches.length !== 1 || matches[0][0].slice(0, `Sync-Upstream-${name}:`.length) !==
      `Sync-Upstream-${name}:`) {
    fail(`Sync-Upstream-${name} trailer must appear exactly once with canonical spelling`);
  }
  trailers[name] = matches[0][1];
}

if (trailers.Automation !== 'true') fail('automation trailer is invalid');
const sourceMatch = trailers.Source.match(
  /^([A-Za-z0-9][A-Za-z0-9._-]*\/[A-Za-z0-9][A-Za-z0-9._-]*)@([0-9a-f]{40})$/,
);
if (!sourceMatch) fail('source trailer is invalid');
const checkpointMatch = trailers.Checkpoint.match(/^([0-9a-f]{40})->([0-9a-f]{40})$/);
if (!checkpointMatch) fail('checkpoint trailer is invalid');
if (!/^[0-9a-f]{40}$/.test(trailers.Base)) fail('base trailer is invalid');
if (trailers.Source !== bodySource) fail('PR source marker conflicts with the commit trailer');
if (trailers.Base !== pr.base.sha || trailers.Base !== actualParent || parentCount !== '1') {
  fail('candidate must have exactly one parent equal to its advertised PR base');
}
if (commitAuthor !== '41898282+github-actions[bot]@users.noreply.github.com' ||
    commitCommitter !== commitAuthor) {
  fail('candidate commit author or committer is untrusted');
}

const [sourceRepository, upstreamTip] = sourceMatch.slice(1);
const [checkpoint, checkpointTip] = checkpointMatch.slice(1);
if (checkpointTip !== upstreamTip) fail('source and checkpoint trailers disagree on U1');
const subject = message.split(/\r?\n/, 1)[0];
if (subject !== `chore: sync ${trailers.Source}`) fail('candidate subject does not match its source');

process.stdout.write(`${JSON.stringify({
  number: pr.number,
  baseSha: pr.base.sha,
  headSha: pr.head.sha,
  sourceRepository,
  checkpoint,
  upstreamTip,
})}\n`);
