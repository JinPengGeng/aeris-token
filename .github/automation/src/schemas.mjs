const CONTROL_LABELS = new Set(['agent-analyze', 'agent-ready', 'automerge-approved']);

function requireCondition(condition, message) {
  if (!condition) throw new Error(message);
}

function exactKeys(value, keys, name) {
  requireCondition(value && typeof value === 'object' && !Array.isArray(value), `${name} must be object`);
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  requireCondition(JSON.stringify(actual) === JSON.stringify(expected), `${name} has unexpected keys`);
}

function cleanString(value, name, maximumLength = 1200) {
  requireCondition(typeof value === 'string', `${name} must be string`);
  const cleaned = value
    .replace(/<!--[\s\S]*?-->/g, '')
    .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, '')
    .trim();
  requireCondition(cleaned.length > 0 && cleaned.length <= maximumLength, `${name} length is invalid`);
  return cleaned;
}

function stringArray(value, name, maximumItems = 12, maximumLength = 500) {
  requireCondition(Array.isArray(value) && value.length <= maximumItems, `${name} must be bounded array`);
  return value.map((entry, index) => cleanString(entry, `${name}[${index}]`, maximumLength));
}

function common(value, agent, keys) {
  exactKeys(value, ['schema_version', 'agent', ...keys], `${agent} output`);
  requireCondition(value.schema_version === 1, 'output schema_version must be 1');
  requireCondition(value.agent === agent, `output agent must be ${agent}`);
}

export function parseModelJson(content) {
  const trimmed = content.trim();
  requireCondition(trimmed.startsWith('{') && trimmed.endsWith('}'), 'model output must be one JSON object');
  try {
    return JSON.parse(trimmed);
  } catch {
    throw new Error('model output is not valid JSON');
  }
}

export function validateAgentOutput(agent, value, repositoryLabels = []) {
  if (agent === 'triage') {
    common(value, agent, [
      'summary',
      'risk',
      'proposed_labels',
      'missing_information',
      'recommended_action',
      'next_agent',
    ]);
    requireCondition(['low', 'medium', 'high'].includes(value.risk), 'triage risk is invalid');
    const knownLabels = new Set(repositoryLabels);
    requireCondition(Array.isArray(value.proposed_labels) && value.proposed_labels.length <= 8, 'labels invalid');
    const proposedLabels = value.proposed_labels.map((label) => {
      const cleaned = cleanString(label, 'proposed label', 80);
      requireCondition(knownLabels.has(cleaned), `unknown proposed label: ${cleaned}`);
      requireCondition(!CONTROL_LABELS.has(cleaned), `control label cannot be proposed: ${cleaned}`);
      return cleaned;
    });
    requireCondition(value.next_agent === null || value.next_agent === 'planner', 'triage next_agent invalid');
    return {
      ...value,
      summary: cleanString(value.summary, 'summary'),
      proposed_labels: proposedLabels,
      missing_information: stringArray(value.missing_information, 'missing_information'),
      recommended_action: cleanString(value.recommended_action, 'recommended_action'),
    };
  }

  if (agent === 'planner') {
    common(value, agent, [
      'summary',
      'acceptance_criteria',
      'implementation_steps',
      'validation_plan',
      'risks',
      'next_agent',
    ]);
    requireCondition(value.next_agent === null || value.next_agent === 'reviewer', 'planner next_agent invalid');
    return {
      ...value,
      summary: cleanString(value.summary, 'summary'),
      acceptance_criteria: stringArray(value.acceptance_criteria, 'acceptance_criteria'),
      implementation_steps: stringArray(value.implementation_steps, 'implementation_steps'),
      validation_plan: stringArray(value.validation_plan, 'validation_plan'),
      risks: stringArray(value.risks, 'risks'),
    };
  }

  if (agent === 'reviewer') {
    common(value, agent, [
      'summary',
      'verdict',
      'findings',
      'test_recommendations',
      'next_agent',
    ]);
    requireCondition(
      ['ready_for_human_review', 'changes_requested', 'needs_human_decision'].includes(value.verdict),
      'review verdict invalid',
    );
    requireCondition(value.next_agent === null || value.next_agent === 'security', 'reviewer next_agent invalid');
    requireCondition(Array.isArray(value.findings) && value.findings.length <= 20, 'findings invalid');
    const findings = value.findings.map((finding, index) => {
      exactKeys(finding, ['severity', 'title', 'details', 'path', 'line'], `finding ${index}`);
      requireCondition(['critical', 'high', 'medium', 'low'].includes(finding.severity), 'severity invalid');
      requireCondition(finding.path === null || typeof finding.path === 'string', 'finding path invalid');
      requireCondition(
        finding.line === null || (Number.isInteger(finding.line) && finding.line > 0),
        'finding line invalid',
      );
      return {
        ...finding,
        title: cleanString(finding.title, `finding ${index} title`, 200),
        details: cleanString(finding.details, `finding ${index} details`, 1000),
        path: finding.path === null ? null : cleanString(finding.path, `finding ${index} path`, 300),
      };
    });
    return {
      ...value,
      summary: cleanString(value.summary, 'summary'),
      findings,
      test_recommendations: stringArray(value.test_recommendations, 'test_recommendations'),
    };
  }

  throw new Error(`unsupported output agent: ${agent}`);
}
