export const MANAGED_MARKER = '<!-- aeris-agent-managed -->';
const META_PREFIX = '<!-- aeris-agent-meta:';
const MAX_PERSISTED_REASON_CODES = 16;

function safe(value) {
  return String(value)
    .replaceAll('@', '&#64;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;');
}

function code(value) {
  return `\`${safe(value).replaceAll('`', "'")}\``;
}

function list(values) {
  return values.length === 0 ? '- None' : values.map((value) => `- ${safe(value)}`).join('\n');
}

export function encodeMetadata(metadata) {
  return Buffer.from(JSON.stringify(metadata), 'utf8').toString('base64url');
}

function fitMetadata(metadata, maximumCharacters, assemble) {
  let candidate = metadata;
  const reasons = Array.isArray(candidate?.reason_codes) ? candidate.reason_codes : [];
  if (reasons.length > MAX_PERSISTED_REASON_CODES) {
    candidate = { ...candidate, reason_codes: reasons.slice(-MAX_PERSISTED_REASON_CODES) };
  }
  while (true) {
    const body = assemble(candidate);
    if (body.length <= maximumCharacters) return body;
    const candidateReasons = Array.isArray(candidate?.reason_codes) ? candidate.reason_codes : [];
    if (candidateReasons.length > 1) {
      candidate = { ...candidate, reason_codes: candidateReasons.slice(1) };
      continue;
    }
    const processed = Array.isArray(candidate?.processed_identities)
      ? candidate.processed_identities
      : [];
    if (processed.length > 0) {
      candidate = { ...candidate, processed_identities: processed.slice(1) };
      continue;
    }
    throw new Error('managed comment exceeds the configured limit');
  }
}

export function decodeMetadata(body) {
  if (typeof body !== 'string') return null;
  const start = body.indexOf(META_PREFIX);
  if (start < 0) return null;
  const encodedStart = start + META_PREFIX.length;
  const end = body.indexOf(' -->', encodedStart);
  if (end < 0) return null;
  try {
    return JSON.parse(Buffer.from(body.slice(encodedStart, end), 'base64url').toString('utf8'));
  } catch {
    return null;
  }
}

export function findManagedComment(comments) {
  const matches = comments.filter(
    (comment) =>
      typeof comment.body === 'string' &&
      comment.body.startsWith(MANAGED_MARKER) &&
      ['github-actions[bot]', 'app/github-actions'].includes(comment.user?.login),
  );
  if (matches.length > 1) throw new Error('multiple bot-managed comments exist');
  return matches[0] ?? null;
}

function renderTriage(output) {
  return [
    `**摘要**\n\n${safe(output.summary)}`,
    `**风险**\n\n${code(output.risk)}`,
    `**建议标签**\n\n${list(output.proposed_labels.map(code))}`,
    `**缺失信息**\n\n${list(output.missing_information)}`,
    `**建议动作**\n\n${safe(output.recommended_action)}`,
  ].join('\n\n');
}

function renderPlanner(output) {
  return [
    `**摘要**\n\n${safe(output.summary)}`,
    `**验收标准**\n\n${list(output.acceptance_criteria)}`,
    `**实施步骤**\n\n${list(output.implementation_steps)}`,
    `**验证计划**\n\n${list(output.validation_plan)}`,
    `**风险**\n\n${list(output.risks)}`,
  ].join('\n\n');
}

function renderReviewer(output) {
  const findings =
    output.findings.length === 0
      ? '- None'
      : output.findings
          .map((finding) => {
            const location = finding.path
              ? ` (${code(finding.path)}${finding.line ? `:${finding.line}` : ''})`
              : '';
            return `- **${safe(finding.severity.toUpperCase())}: ${safe(finding.title)}**${location}\n  ${safe(finding.details)}`;
          })
          .join('\n');
  return [
    `**结论**\n\n${code(output.verdict)}`,
    `**摘要**\n\n${safe(output.summary)}`,
    `**发现**\n\n${findings}`,
    `**测试建议**\n\n${list(output.test_recommendations)}`,
  ].join('\n\n');
}

function renderCompactAnalysis(agent, output, maximumSummaryCharacters) {
  const characters = Array.from(output.summary);
  const truncated = characters.length > maximumSummaryCharacters;
  const summary = characters.slice(0, maximumSummaryCharacters).join('');
  const result = agent === 'triage'
    ? `**风险**\n\n${code(output.risk)}`
    : agent === 'reviewer'
      ? `**结论**\n\n${code(output.verdict)}`
      : null;
  return [
    result,
    `**摘要${truncated ? '（截断）' : ''}**\n\n${safe(summary) || 'None'}`,
    '> 详细结果超过 managed comment 大小上限，仅显示紧凑摘要。',
  ].filter(Boolean).join('\n\n');
}

export function renderAnalysisComment(agent, output, metadata, maximumCharacters) {
  const content =
    agent === 'triage'
      ? renderTriage(output)
      : agent === 'planner'
        ? renderPlanner(output)
        : renderReviewer(output);
  const nextAgent = output.next_agent ? code(output.next_agent) : 'None';
  const assemble = (fittedMetadata) => `${MANAGED_MARKER}
## Aeris Agent: ${safe(agent)}

${content}

**建议下一 Agent**: ${nextAgent}

> 只读建议。此评论不会自动修改标签、代码、审批状态或合并状态。

${META_PREFIX}${encodeMetadata(fittedMetadata)} -->`;
  try {
    return fitMetadata(metadata, maximumCharacters, assemble);
  } catch (error) {
    if (error.message !== 'managed comment exceeds the configured limit') throw error;
    for (const maximumSummaryCharacters of [600, 300, 120, 0]) {
      try {
        const compact = renderCompactAnalysis(agent, output, maximumSummaryCharacters);
        return fitMetadata(metadata, maximumCharacters, (fittedMetadata) => `${MANAGED_MARKER}
## Aeris Agent: ${safe(agent)}

${compact}

> 只读建议。此评论不会自动修改标签、代码、审批状态或合并状态。

${META_PREFIX}${encodeMetadata(fittedMetadata)} -->`);
      } catch (compactError) {
        if (compactError.message !== 'managed comment exceeds the configured limit') throw compactError;
      }
    }
    throw error;
  }
}

export function renderStatusComment(status, metadata, maximumCharacters, existingBody = null) {
  const statusBlock = `<!-- aeris-agent-status:start -->
## Aeris Agent 状态

- 状态: ${code(status)}
- 对象 generation: ${code(metadata.object_generation)}
- Policy SHA: ${code(metadata.policy_sha)}
<!-- aeris-agent-status:end -->`;
  const preserved =
    typeof existingBody === 'string' && existingBody.startsWith(MANAGED_MARKER)
      ? existingBody
          .slice(MANAGED_MARKER.length)
          .replace(/<!-- aeris-agent-status:start -->[\s\S]*?<!-- aeris-agent-status:end -->/g, '')
          .replace(/<!-- aeris-agent-meta:[A-Za-z0-9_-]+ -->/g, '')
          .trim()
      : '';
  const assemble = (preservedContent, fittedMetadata) => `${MANAGED_MARKER}
${statusBlock}${preservedContent ? `\n\n${preservedContent}` : ''}

${META_PREFIX}${encodeMetadata(fittedMetadata)} -->`;
  try {
    return fitMetadata(metadata, maximumCharacters, (fittedMetadata) =>
      assemble(preserved, fittedMetadata));
  } catch (error) {
    if (error.message !== 'managed comment exceeds the configured limit') throw error;
    return fitMetadata(metadata, maximumCharacters, (fittedMetadata) =>
      assemble('', fittedMetadata));
  }
}

export function metadataMatches(metadata, identity) {
  return (
    metadata?.source_key === identity.sourceKey &&
    metadata?.agent === identity.agent &&
    metadata?.input_sha === identity.inputSha &&
    metadata?.policy_sha === identity.policySha
  );
}

export function metadataHasProcessedIdentity(metadata, identity) {
  const processed = Array.isArray(metadata?.processed_identities)
    ? metadata.processed_identities
    : [];
  return processed.some(
    (entry) =>
      entry?.source_key === identity.sourceKey &&
      entry?.agent === identity.agent &&
      entry?.input_sha === identity.inputSha &&
      entry?.policy_sha === identity.policySha,
  );
}
