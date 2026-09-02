import fs from 'node:fs';
import path from 'node:path';

import yaml from 'js-yaml';

import { ContractError } from './config.mjs';

export const CHANGE_FILTERS_PATH = '.github/change-filters.yml';

function hasControlCharacters(value) {
  for (const character of value) {
    const code = character.codePointAt(0);
    if (code < 32 || code === 127) return true;
  }
  return false;
}

// The CI workflows hand the shared filters file to dorny/paths-filter while
// this runtime matches the same groups itself. Both sides only agree because
// patterns stay inside the exact_or_directory_recursive subset (the
// aeris-glob-v1 shape used by the sync runtime): anything richer is rejected
// here so the shared file cannot silently drift into picomatch-only syntax.
function compilePattern(pattern) {
  if (
    typeof pattern !== 'string' ||
    pattern.length === 0 ||
    hasControlCharacters(pattern) ||
    pattern.startsWith('!') ||
    pattern.startsWith('/') ||
    pattern.endsWith('/') ||
    /[\\[\]?]/.test(pattern)
  ) {
    throw new ContractError(`unsupported change filter pattern: ${String(pattern)}`);
  }
  if (pattern.endsWith('/**')) {
    const prefix = pattern.slice(0, -'/**'.length);
    if (prefix.length === 0 || prefix.includes('*')) {
      throw new ContractError(`unsupported change filter pattern: ${pattern}`);
    }
    return (filePath) => filePath.startsWith(`${prefix}/`);
  }
  if (pattern.includes('*')) {
    throw new ContractError(`unsupported change filter pattern: ${pattern}`);
  }
  return (filePath) => filePath === pattern;
}

export function validateChangeFilters(value) {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new ContractError('change filters must be a mapping of filter groups');
  }
  const groups = {};
  for (const [name, patterns] of Object.entries(value)) {
    if (!Array.isArray(patterns) || patterns.length === 0) {
      throw new ContractError(`change filter group ${name} must be a non-empty list`);
    }
    groups[name] = Object.freeze(patterns.map(compilePattern));
  }
  if (Object.keys(groups).length === 0) {
    throw new ContractError('change filters must define at least one group');
  }
  return Object.freeze(groups);
}

export function loadChangeFilters(repoRoot) {
  const filePath = path.join(repoRoot, CHANGE_FILTERS_PATH);
  return validateChangeFilters(yaml.load(fs.readFileSync(filePath, 'utf8')));
}

export function matchedChangeFilterGroups(filters, filePaths) {
  return Object.entries(filters)
    .filter(([, matchers]) =>
      filePaths.some((filePath) => matchers.some((matches) => matches(filePath))))
    .map(([name]) => name);
}
