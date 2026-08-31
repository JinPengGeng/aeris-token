#!/usr/bin/env node
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const payload = JSON.parse(await readFile(new URL('./agent-branch-ruleset.json', import.meta.url), 'utf8'));
const tagPayload = JSON.parse(await readFile(new URL('./merger-tag-ruleset.json', import.meta.url), 'utf8'));
const types = new Set(payload.rules.map((rule) => rule.type));
const tagTypes = new Set(tagPayload.rules.map((rule) => rule.type));
if (payload.target !== 'branch' || payload.enforcement !== 'active') throw new Error('ruleset must actively target branches');
if (JSON.stringify(payload.conditions?.ref_name?.include) !== JSON.stringify(['refs/heads/agent/**'])) throw new Error('ruleset must only target agent branches');
if (!types.has('deletion') || !types.has('non_fast_forward') || types.has('update')) throw new Error('ruleset must deny deletion and force push while preserving fast-forward updates');
if ((payload.bypass_actors ?? []).length !== 0) throw new Error('ruleset must not grant bypass actors');
if (tagPayload.target !== 'tag' || tagPayload.enforcement !== 'active') throw new Error('merger tag ruleset must actively target tags');
if (JSON.stringify(tagPayload.conditions?.ref_name?.include) !== JSON.stringify(['refs/tags/aeris-merger-attempt-*'])) throw new Error('merger tag ruleset prefix drift');
if (tagTypes.has('creation') || !tagTypes.has('deletion') || !tagTypes.has('non_fast_forward') || !tagTypes.has('update')) throw new Error('merger tag ruleset must allow first create and fence later mutations');
if (tagPayload.rules.find((rule) => rule.type === 'update')?.parameters?.update_allows_fetch_and_merge !== false) throw new Error('merger tag updates must not allow fetch-and-merge');
if ((tagPayload.bypass_actors ?? []).length !== 0) throw new Error('merger tag ruleset must not grant bypass actors');
process.stdout.write('agent branch and merger generation tag rulesets are valid\n');
