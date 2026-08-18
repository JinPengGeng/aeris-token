import { createHash } from 'node:crypto';

function truncate(value, maximumLength) {
  const text = typeof value === 'string' ? value : '';
  if (text.length <= maximumLength) return { value: text, truncated: false };
  return { value: text.slice(0, maximumLength), truncated: true };
}

function labels(item) {
  return (item.labels ?? [])
    .map((label) => (typeof label === 'string' ? label : label.name))
    .filter((name) => typeof name === 'string')
    .sort();
}

function canonicalize(value) {
  if (Array.isArray(value)) return value.map(canonicalize);
  if (value === null || typeof value !== 'object') return value;
  return Object.fromEntries(
    Object.keys(value)
      .sort()
      .filter((key) => value[key] !== undefined)
      .map((key) => [key, canonicalize(value[key])]),
  );
}

export function buildIssueInput(issue, { maximumCharacters, repositoryLabels }) {
  const title = truncate(issue.title, 512);
  const bodyBudget = Math.max(1000, maximumCharacters - 2500 - repositoryLabels.join('').length);
  const body = truncate(issue.body, bodyBudget);
  return {
    object: 'issue',
    number: issue.number,
    url: issue.html_url,
    title: title.value,
    body: body.value,
    labels: labels(issue),
    author_association: issue.author_association,
    available_labels: repositoryLabels,
    truncated: title.truncated || body.truncated,
  };
}

function fittingPrefix(value, maximumLength, fits) {
  if (value.length <= maximumLength && fits(value)) return value;
  let low = 0;
  let high = Math.min(value.length, maximumLength);
  while (low < high) {
    const middle = Math.ceil((low + high) / 2);
    if (fits(value.slice(0, middle))) low = middle;
    else high = middle - 1;
  }
  return value.slice(0, low);
}

export function buildPullInput(pull, pullFiles, {
  maximumCharacters,
  maximumPatchCharactersPerFile = 5000,
}) {
  if (pullFiles.truncated || pullFiles.files.length !== pull.changed_files) {
    throw new Error('Reviewer pull file list is incomplete');
  }
  const title = truncate(pull.title, 512);
  const body = truncate(pull.body, 5000);
  const input = {
    object: 'pull_request',
    number: pull.number,
    url: pull.html_url,
    title: title.value,
    body: body.value,
    author_association: pull.author_association,
    labels: labels(pull),
    base: { ref: pull.base.ref, sha: pull.base.sha },
    head: { ref: pull.head.ref, sha: pull.head.sha },
    changed_files: pull.changed_files,
    files: [],
    truncated: title.truncated || body.truncated || pullFiles.truncated,
  };

  for (const file of pullFiles.files) {
    input.files.push({
      path: file.filename,
      status: file.status,
      additions: file.additions,
      deletions: file.deletions,
      changes: file.changes,
      patch: null,
      patch_truncated: false,
    });
  }

  if (JSON.stringify(input).length > maximumCharacters) {
    throw new Error('Reviewer pull metadata exceeds the maximum input size');
  }

  for (const [index, file] of input.files.entries()) {
    const sourcePatch = pullFiles.files[index].patch;
    if (typeof sourcePatch !== 'string' || sourcePatch.length === 0) continue;
    const cappedPatch = truncate(sourcePatch, maximumPatchCharactersPerFile);
    const patch = fittingPrefix(
      cappedPatch.value,
      maximumPatchCharactersPerFile,
      (value) => {
        file.patch = value || null;
        return JSON.stringify(input).length <= maximumCharacters;
      },
    );
    file.patch = patch || null;
    file.patch_truncated = patch.length !== sourcePatch.length;
    if (file.patch_truncated) input.truncated = true;
  }
  return input;
}

export function canonicalInput(input) {
  return JSON.stringify(canonicalize(input));
}

export function inputFingerprint(input) {
  return createHash('sha256').update(canonicalInput(input)).digest('hex');
}

export function hashInput(input) {
  return inputFingerprint(input);
}

export function sourceKey(eventName, event, object, environment) {
  if (eventName === 'issue_comment') return `comment:${event.comment.id}`;
  if (eventName === 'issues') {
    return `issue:${event.action}:${object.id}:${event.issue?.updated_at ?? object.updated_at}`;
  }
  if (eventName === 'workflow_run') return `pull:${object.number}:${object.head.sha}`;
  return `dispatch:${environment.GITHUB_RUN_ID}:${environment.GITHUB_RUN_ATTEMPT ?? '1'}`;
}
