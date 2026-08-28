const EXECUTOR_ID = /^[a-z][a-z0-9-]{2,63}$/;
const PROTOCOL = /^[a-z][a-z0-9-]{2,63}$/;
const ROUTE = /^[a-z][a-z0-9_]{2,63}$/;
const ACTION_SHA = /^[0-9a-f]{40}$/;
const TOOL_VERSION = /^[0-9]+(?:\.[0-9]+){1,3}$/;
const COMPLETION_KIND = 'completion';
const WORKSPACE_CANDIDATE_KIND = 'workspace_candidate';
const ROUTE_KINDS = Object.freeze({
  agent_analysis: COMPLETION_KIND,
  sync_conflict_resolver: COMPLETION_KIND,
  sync_conflict_reviewer: COMPLETION_KIND,
  candidate: WORKSPACE_CANDIDATE_KIND,
});

function fail(message) {
  throw new Error(`AI executor contract: ${message}`);
}

function exactKeys(value, keys, name) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) fail(`${name} must be an object`);
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index])) {
    fail(`${name} has invalid fields`);
  }
}

export function validateExecutorIdentity(value, name = 'executor identity') {
  exactKeys(value, ['id', 'protocol'], name);
  return normalizeExecutorIdentity(value, name);
}

function normalizeExecutorIdentity(value, name) {
  if (typeof value.id !== 'string' || !EXECUTOR_ID.test(value.id)) fail(`${name} ID is invalid`);
  if (typeof value.protocol !== 'string' || !PROTOCOL.test(value.protocol)) fail(`${name} protocol is invalid`);
  return Object.freeze({ id: value.id, protocol: value.protocol });
}

function validateExecutorDescriptor(value, name) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) fail(`${name} must be an object`);
  const kind = value.kind;
  if (kind === COMPLETION_KIND) {
    exactKeys(value, ['id', 'kind', 'protocol'], name);
    return Object.freeze({ ...normalizeExecutorIdentity(value, name), kind });
  }
  if (kind === WORKSPACE_CANDIDATE_KIND) {
    exactKeys(value, ['action_sha', 'id', 'kind', 'protocol', 'tool_version'], name);
    const identity = normalizeExecutorIdentity(value, name);
    if (typeof value.action_sha !== 'string' || !ACTION_SHA.test(value.action_sha)) fail(`${name} action SHA is invalid`);
    if (typeof value.tool_version !== 'string' || !TOOL_VERSION.test(value.tool_version)) fail(`${name} tool version is invalid`);
    return Object.freeze({
      ...identity,
      kind,
      action_sha: value.action_sha,
      tool_version: value.tool_version,
    });
  }
  fail(`${name} kind is invalid`);
}

export function validateExecutorRegistry(value) {
  exactKeys(value, ['schema_version', 'executors', 'routes'], 'registry');
  if (value.schema_version !== 1) fail('registry schema version is invalid');
  if (!Array.isArray(value.executors) || value.executors.length < 2 || value.executors.length > 16) {
    fail('registry executor count is invalid');
  }
  const executors = value.executors.map((entry, index) => validateExecutorDescriptor(entry, `executor ${index}`));
  if (new Set(executors.map((entry) => entry.id)).size !== executors.length) fail('registry executor IDs are not unique');
  const routes = value.routes;
  if (!routes || typeof routes !== 'object' || Array.isArray(routes)) fail('registry routes are invalid');
  const expectedRoutes = Object.keys(ROUTE_KINDS);
  exactKeys(routes, expectedRoutes, 'registry routes');
  const byId = new Map(executors.map((entry) => [entry.id, entry]));
  const resolvedRoutes = {};
  for (const route of expectedRoutes) {
    if (!ROUTE.test(route) || typeof routes[route] !== 'string' || !byId.has(routes[route])) {
      fail(`route ${route} is not bound to a trusted executor`);
    }
    if (byId.get(routes[route]).kind !== ROUTE_KINDS[route]) fail(`route ${route} has the wrong executor kind`);
    resolvedRoutes[route] = routes[route];
  }
  return Object.freeze({ schema_version: 1, executors: Object.freeze(executors), routes: Object.freeze(resolvedRoutes) });
}

export function executorForRoute(registry, route) {
  const descriptor = executorDescriptorForRoute(registry, route);
  return Object.freeze({ id: descriptor.id, protocol: descriptor.protocol });
}

export function executorDescriptorForRoute(registry, route) {
  const normalized = validateExecutorRegistry(registry);
  if (typeof route !== 'string' || !ROUTE.test(route) || !Object.hasOwn(normalized.routes, route)) {
    fail(`route ${route} is not approved`);
  }
  return normalized.executors.find((entry) => entry.id === normalized.routes[route]);
}

export function validateWorkspaceCandidateExecutor(value, name = 'workspace candidate executor') {
  const descriptor = validateExecutorDescriptor(value, name);
  if (descriptor.kind !== WORKSPACE_CANDIDATE_KIND) fail(`${name} kind is invalid`);
  return descriptor;
}
