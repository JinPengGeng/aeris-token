function timestamp(entry) {
  for (const value of [entry.started_at, entry.created_at, entry.updated_at, entry.completed_at]) {
    const parsed = Date.parse(value ?? '');
    if (Number.isFinite(parsed)) return parsed;
  }
  return 0;
}

function sequence(entry) {
  return Number.isSafeInteger(entry.id) ? entry.id : 0;
}

function isNewer(candidate, current) {
  const candidateSequence = sequence(candidate);
  const currentSequence = sequence(current);
  if (candidateSequence > 0 && currentSequence > 0 && candidateSequence !== currentSequence) {
    return candidateSequence > currentSequence;
  }
  const timeDifference = timestamp(candidate) - timestamp(current);
  return timeDifference > 0 || (timeDifference === 0 && candidateSequence > currentSequence);
}

function checkRunResult(checkRun) {
  return checkRun.status === 'completed' && checkRun.conclusion === 'success' ? 'success' : 'not_successful';
}

function commitStatusResult(status) {
  return status.state === 'success' ? 'success' : 'not_successful';
}

export function evaluateRequiredChecks(requiredChecks, checkRuns, commitStatuses) {
  const latestCheckRunByContext = new Map();
  const latestStatusByContext = new Map();
  const record = (signals, context, entry, result) => {
    if (typeof context !== 'string' || context.length === 0) return;
    const current = signals.get(context);
    if (!current || isNewer(entry, current.entry)) signals.set(context, { entry, result });
  };

  for (const checkRun of checkRuns) {
    record(latestCheckRunByContext, checkRun.name, checkRun, checkRunResult(checkRun));
  }
  for (const status of commitStatuses) {
    record(latestStatusByContext, status.context, status, commitStatusResult(status));
  }

  const unsuccessful = requiredChecks.filter((context) => {
    const signal = latestCheckRunByContext.get(context) ?? latestStatusByContext.get(context);
    return signal?.result !== 'success';
  });
  return { ready: unsuccessful.length === 0, unsuccessful };
}
