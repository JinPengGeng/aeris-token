import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  AutonomyPublisherError,
  LocalGitPublisher,
  trustedCandidateExecutorForBase,
  WriterGitHubClient,
  publishCandidate,
} from '../src/autonomy-publisher.mjs';

const repository = 'JinPengGeng/aeris-token';
const writerLogin = 'aeris-writer[bot]';
const baseSha = 'a'.repeat(40);
const commitSha = 'b'.repeat(40);
const branch = 'agent/issue-123';
const taskId = 'issue:123';
const candidateExecutor = Object.freeze({
  id: 'codex-action-v1',
  protocol: 'aeris-workspace-candidate-v1',
  kind: 'workspace_candidate',
  action_sha: '52fe01ec70a42f454c9d2ebd47598f9fd6893d56',
  tool_version: '0.148.0',
});

const expected = Object.freeze({
  repository,
  repository_id: 1310462380,
  task_id: taskId,
  issue_number: 123,
  base_ref: 'refs/heads/main',
  base_sha: baseSha,
  trigger_run_id: '456',
  trigger_run_attempt: 1,
  executor: candidateExecutor,
});

const manifest = Object.freeze({
  ...expected,
  schema_version: 2,
  patch_sha256: 'c'.repeat(64),
  patch_bytes: 100,
  created_at: '2026-08-20T00:00:00.000Z',
});

const artifact = Object.freeze({
  patchPath: 'candidate.patch',
  verified: Object.freeze({ manifest, paths: Object.freeze(['docs/automation-canary/example.md']) }),
});

function ref(sha = commitSha, refBranch = branch) {
  return { ref: `refs/heads/${refBranch}`, object: { type: 'commit', sha } };
}

function pull({
  number = 17,
  state = 'open',
  sha = commitSha,
  title = 'old title',
  body = '<!-- aeris-autonomy-managed -->\n<!-- aeris-autonomy-task:issue:123 -->\nold body',
  login = writerLogin,
  base = 'main',
  head = branch,
  headRepository = repository,
  draft = true,
  autoMerge = null,
  htmlUrl = `https://github.com/${repository}/pull/${number}`,
} = {}) {
  return {
    number,
    state,
    title,
    body,
    html_url: htmlUrl,
    user: { login },
    base: { ref: base },
    head: { ref: head, sha, repo: { full_name: headRepository } },
    draft,
    auto_merge: autoMerge,
  };
}

function commitMessage(sourceManifest = manifest) {
  return [
    `chore(autonomy): update issue #${sourceManifest.issue_number}`,
    '',
    'Aeris-Autonomy-Managed: true',
    `Aeris-Autonomy-Task: ${sourceManifest.task_id}`,
    `Aeris-Autonomy-Patch: ${sourceManifest.patch_sha256}`,
    `Aeris-Autonomy-Base: ${sourceManifest.base_sha}`,
    `Aeris-Autonomy-Run: ${sourceManifest.trigger_run_id}/${sourceManifest.trigger_run_attempt}`,
    `Aeris-Autonomy-Executor-ID: ${sourceManifest.executor.id}`,
    `Aeris-Autonomy-Executor-Protocol: ${sourceManifest.executor.protocol}`,
    `Aeris-Autonomy-Executor-Action-SHA: ${sourceManifest.executor.action_sha}`,
    `Aeris-Autonomy-Executor-Tool-Version: ${sourceManifest.executor.tool_version}`,
  ].join('\n');
}

function harness({
  initialSha = null,
  pulls = [],
  pushedSha = commitSha,
  persisted = {},
  persistedCommit = {},
  baseShas = [baseSha],
} = {}) {
  const calls = [];
  let mutationBody = null;
  let branchReads = 0;
  const client = {
    async getBranch(requestedBranch) {
      if (requestedBranch === 'main') {
        const baseRead = calls.filter((entry) => entry[0] === 'getBase').length;
        const sha = baseShas[Math.min(baseRead, baseShas.length - 1)];
        calls.push(['getBase', sha]);
        return ref(sha, 'main');
      }
      branchReads += 1;
      calls.push(['getBranch', branchReads]);
      if (branchReads === 1) return initialSha === null ? null : ref(initialSha);
      return ref(pushedSha);
    },
    async listBranchPulls(owner, requestedBranch) {
      calls.push(['listBranchPulls', owner, requestedBranch]);
      return pulls;
    },
    async createPull(body) {
      calls.push(['createPull', body]);
      mutationBody = body;
      return { number: persisted.number ?? 17 };
    },
    async updatePull(number, body) {
      calls.push(['updatePull', number, body]);
      mutationBody = body;
      return { number: persisted.number ?? number };
    },
    async getPull(number) {
      calls.push(['getPull', number]);
      return pull({
        number,
        title: mutationBody?.title,
        body: mutationBody?.body,
        ...persisted,
      });
    },
    async getCommit(sha) {
      calls.push(['getCommit', sha]);
      return {
        sha,
        tree: { sha: 'd'.repeat(40) },
        message: commitMessage(),
        ...persistedCommit,
      };
    },
  };
  const gitPublisher = {
    prepareCommit(input) {
      calls.push(['prepareCommit', input]);
      return { sha: commitSha, tree: 'd'.repeat(40) };
    },
    push(requestedBranch, oldSha) {
      calls.push(['push', requestedBranch, oldSha]);
    },
  };
  return { calls, client, gitPublisher };
}

async function publish(overrides = {}) {
  const fixture = harness(overrides);
  const result = await publishCandidate({
    artifact,
    expected,
    client: fixture.client,
    gitPublisher: fixture.gitPublisher,
    writerLogin,
    runUrl: 'https://github.com/JinPengGeng/aeris-token/actions/runs/456',
  });
  return { ...fixture, result };
}

test('new publication uses an exact nonexistence lease and confirms branch and PR state', async () => {
  const { calls, result } = await publish();

  assert.deepEqual(result, {
    branch,
    head_sha: commitSha,
    pull_number: 17,
    pull_url: 'https://github.com/JinPengGeng/aeris-token/pull/17',
    action: 'created',
  });
  assert.ok(calls.some((entry) => entry[0] === 'push' && entry[1] === branch && entry[2] === null));
  const create = calls.find((entry) => entry[0] === 'createPull')[1];
  assert.equal(create.head, branch);
  assert.equal(create.base, 'main');
  assert.equal(create.draft, true);
  assert.match(create.body, /aeris-autonomy-patch:c{64}/);
  assert.match(create.body, /aeris-autonomy-executor-id:codex-action-v1/);
  assert.match(create.body, /aeris-autonomy-executor-action-sha:52fe01ec70a42f454c9d2ebd47598f9fd6893d56/);
  assert.ok(calls.some((entry) => entry[0] === 'getCommit' && entry[1] === commitSha));
  assert.equal(calls.filter((entry) => entry[0] === 'getBranch').length, 3);
  assert.equal(calls.filter((entry) => entry[0] === 'getBase').length, 3);
});

test('retry after a push but before PR creation reuses the exact remote commit', async () => {
  const { calls, result } = await publish({ initialSha: commitSha });
  assert.equal(result.action, 'created');
  assert.equal(calls.some((entry) => entry[0] === 'push'), false);
  assert.equal(calls.filter((entry) => entry[0] === 'getBranch').length, 2);
});

test('retry after PR creation updates the single owned draft PR idempotently', async () => {
  const existing = pull();
  const { calls, result } = await publish({ initialSha: commitSha, pulls: [existing] });
  assert.equal(result.action, 'updated');
  assert.equal(calls.some((entry) => entry[0] === 'push'), false);
  assert.ok(calls.some((entry) => entry[0] === 'updatePull' && entry[1] === 17));
  assert.equal(calls.some((entry) => entry[0] === 'createPull'), false);
});

test('an existing owned PR is updated only after an exact old-SHA lease', async () => {
  const oldSha = 'e'.repeat(40);
  const existing = pull({ sha: oldSha });
  const { calls } = await publish({ initialSha: oldSha, pulls: [existing] });
  assert.ok(calls.some((entry) => entry[0] === 'push' && entry[2] === oldSha));
});

test('any closed same-branch PR is a fail-closed tombstone even if its marker was removed', async () => {
  const fixture = harness({ initialSha: commitSha, pulls: [pull({ state: 'closed', body: 'marker removed manually' })] });
  await assert.rejects(
    () => publishCandidate({ artifact, expected, client: fixture.client, gitPublisher: fixture.gitPublisher, writerLogin, runUrl: 'https://github.com/run' }),
    (error) => error instanceof AutonomyPublisherError && /tombstoned/.test(error.message),
  );
  assert.equal(fixture.calls.some((entry) => ['push', 'createPull', 'updatePull'].includes(entry[0])), false);
});

test('malformed or incomplete pull history is rejected before mutation', async () => {
  const fixture = harness({ pulls: [{ number: 1, state: 'open', head: { ref: branch } }] });
  await assert.rejects(
    () => publishCandidate({ artifact, expected, client: fixture.client, gitPublisher: fixture.gitPublisher, writerLogin, runUrl: 'https://github.com/run' }),
    /pull request history is incomplete/,
  );
  assert.equal(fixture.calls.some((entry) => entry[0] === 'push'), false);
});

test('duplicate PR numbers from a shifting pagination window are rejected as incomplete', async () => {
  const existing = pull();
  const fixture = harness({ initialSha: commitSha, pulls: [existing, { ...existing }] });
  await assert.rejects(
    () => publishCandidate({ artifact, expected, client: fixture.client, gitPublisher: fixture.gitPublisher, writerLogin, runUrl: 'https://github.com/run' }),
    /pull request history is incomplete/,
  );
  assert.equal(fixture.calls.some((entry) => entry[0] === 'updatePull'), false);
});

test('an unowned pre-existing branch is rejected rather than overwritten', async () => {
  const fixture = harness({ initialSha: 'e'.repeat(40) });
  await assert.rejects(
    () => publishCandidate({ artifact, expected, client: fixture.client, gitPublisher: fixture.gitPublisher, writerLogin, runUrl: 'https://github.com/run' }),
    /unowned managed branch/,
  );
  assert.equal(fixture.calls.some((entry) => entry[0] === 'push'), false);
});

test('publication stops when the pushed ref does not resolve to the exact commit', async () => {
  const fixture = harness({ pushedSha: 'e'.repeat(40) });
  await assert.rejects(
    () => publishCandidate({ artifact, expected, client: fixture.client, gitPublisher: fixture.gitPublisher, writerLogin, runUrl: 'https://github.com/run' }),
    /exact managed branch SHA/,
  );
  assert.equal(fixture.calls.some((entry) => entry[0] === 'createPull'), false);
});

test('base drift before or after PR mutation fails closed', async () => {
  for (const baseShas of [
    [baseSha, 'f'.repeat(40)],
    [baseSha, baseSha, 'f'.repeat(40)],
  ]) {
    const fixture = harness({ baseShas });
    await assert.rejects(
      () => publishCandidate({
        artifact, expected, client: fixture.client, gitPublisher: fixture.gitPublisher, writerLogin,
        runUrl: 'https://github.com/JinPengGeng/aeris-token/actions/runs/456',
      }),
      (error) => error instanceof AutonomyPublisherError && /base branch drifted/.test(error.message),
    );
  }
});

test('post-write verification checks PR head, base, draft, author, repository, auto-merge, and URL', async (t) => {
  const cases = [
    ['head SHA', { sha: 'e'.repeat(40) }],
    ['base', { base: 'release' }],
    ['draft', { draft: false }],
    ['author', { login: 'other-writer[bot]' }],
    ['head repository', { headRepository: 'outside/fork' }],
    ['auto merge', { autoMerge: { merge_method: 'squash' } }],
    ['URL', { htmlUrl: 'https://github.com/JinPengGeng/aeris-token/pull/99' }],
  ];
  for (const [name, persisted] of cases) {
    await t.test(name, async () => {
      await assert.rejects(() => publish({ persisted }), AutonomyPublisherError);
    });
  }
});

test('remote commit provenance must bind the exact trusted Candidate executor before a PR mutation', async (t) => {
  for (const [name, persistedCommit] of [
    ['commit SHA', { sha: 'e'.repeat(40) }],
    ['tree SHA', { tree: { sha: 'e'.repeat(40) } }],
    ['executor trailer', { message: commitMessage().replace('codex-action-v1', 'untrusted-action-v1') }],
  ]) {
    await t.test(name, async () => {
      const fixture = harness({ persistedCommit });
      await assert.rejects(
        () => publishCandidate({ artifact, expected, client: fixture.client, gitPublisher: fixture.gitPublisher, writerLogin, runUrl: 'https://github.com/run' }),
        /exact managed candidate commit|executor provenance/,
      );
      assert.equal(fixture.calls.some((entry) => ['createPull', 'updatePull'].includes(entry[0])), false);
    });
  }
});

test('ordinary git subprocesses strip inherited Writer token and askpass variables and carry a hard timeout', () => {
  const previous = {
    token: process.env.AERIS_WRITER_TOKEN,
    askpass: process.env.GIT_ASKPASS,
    require: process.env.GIT_ASKPASS_REQUIRE,
  };
  process.env.AERIS_WRITER_TOKEN = 'must-not-leak';
  process.env.GIT_ASKPASS = 'must-not-leak';
  process.env.GIT_ASKPASS_REQUIRE = 'force';
  const calls = [];
  try {
    const publisher = new LocalGitPublisher({
      repositoryRoot: '.',
      token: 'writer-token',
      repository,
      gitTimeoutMs: 321,
      execFileImpl(file, args, options) {
        calls.push({ file, args, options });
        return args[0] === 'rev-parse' ? `${baseSha}\n` : '';
      },
    });
    publisher.verifyBase(baseSha);
  } finally {
    for (const [key, value] of Object.entries({
      AERIS_WRITER_TOKEN: previous.token,
      GIT_ASKPASS: previous.askpass,
      GIT_ASKPASS_REQUIRE: previous.require,
    })) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  }
  assert.equal(calls.length, 2);
  for (const call of calls) {
    assert.equal(call.options.env.AERIS_WRITER_TOKEN, undefined);
    assert.equal(call.options.env.GIT_ASKPASS, undefined);
    assert.equal(call.options.env.GIT_ASKPASS_REQUIRE, undefined);
    assert.equal(call.options.timeout, 321);
    assert.equal(call.options.killSignal, 'SIGKILL');
  }
});

test('push injects the Writer token only into askpass and uses an exact lease for a missing branch', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-push-test-'));
  let captured;
  try {
    const publisher = new LocalGitPublisher({
      repositoryRoot: root,
      token: 'writer-token',
      repository,
      execFileImpl(file, args, options) {
        captured = { file, args, options, askpassExists: fs.existsSync(options.env.GIT_ASKPASS) };
        return '';
      },
    });
    publisher.push(branch, null);
    assert.equal(fs.existsSync(captured.options.env.GIT_ASKPASS), false);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
  assert.equal(captured.options.env.AERIS_WRITER_TOKEN, 'writer-token');
  assert.equal(captured.options.env.GIT_ASKPASS_REQUIRE, 'force');
  assert.equal(captured.askpassExists, true);
  assert.ok(captured.args.includes(`--force-with-lease=refs/heads/${branch}:`));
  assert.ok(captured.args.includes('credential.helper='));
  assert.ok(captured.args.includes('http.https://github.com/.extraheader='));
  assert.equal(captured.args.some((value) => value.includes('writer-token')), false);
});

test('git timeout failures are normalized without exposing subprocess details', () => {
  const publisher = new LocalGitPublisher({
    repositoryRoot: '.',
    token: null,
    repository,
    execFileImpl() {
      const error = new Error('timed out');
      error.code = 'ETIMEDOUT';
      throw error;
    },
  });
  assert.throws(() => publisher.verifyBase(baseSha), /git rev-parse timed out/);
});

test('Writer REST calls encode branch names and stop pagination at the hard page bound', async () => {
  const calls = [];
  const fullPage = Array.from({ length: 100 }, (_, index) => ({ id: index + 1 }));
  const client = new WriterGitHubClient({
    token: 'test-token',
    repository,
    requestTimeoutMs: 1_000,
    fetchImpl: async (url) => {
      calls.push(url);
      if (url.includes('/git/ref/heads/')) return new Response(JSON.stringify(ref()), { status: 200 });
      return new Response(JSON.stringify(fullPage), { status: 200 });
    },
  });

  await client.getBranch('agent/issue-123/topic');
  await assert.rejects(() => client.listBranchPulls('JinPengGeng', 'agent/issue-123/topic'), /too many historical pull requests/);
  assert.match(calls[0], /git\/ref\/heads\/agent%2Fissue-123%2Ftopic$/);
  assert.match(calls[1], /sort=created&direction=asc&head=JinPengGeng%3Aagent%2Fissue-123%2Ftopic&per_page=100&page=1$/);
  assert.match(calls[2], /page=2$/);
  assert.equal(calls.length, 3);
});

test('Writer REST timeout covers response-body reads, not only connection setup', async () => {
  const client = new WriterGitHubClient({
    token: 'test-token',
    repository,
    requestTimeoutMs: 20,
    fetchImpl: async () => ({
      status: 200,
      ok: true,
      text: () => new Promise(() => {}),
    }),
  });
  await assert.rejects(() => client.getBranch(branch), /GitHub API request timed out/);
});

function executorRegistry(toolVersion = candidateExecutor.tool_version) {
  return JSON.stringify({
    schema_version: 1,
    executors: [
      { id: 'openai-chat-v1', kind: 'completion', protocol: 'openai-chat-completions-v1' },
      { id: 'openai-responses-v1', kind: 'completion', protocol: 'openai-responses-v1' },
      { ...candidateExecutor, tool_version: toolVersion },
    ],
    routes: {
      agent_analysis: 'openai-chat-v1',
      sync_conflict_resolver: 'openai-chat-v1',
      sync_conflict_reviewer: 'openai-chat-v1',
      candidate: 'codex-action-v1',
    },
  });
}

function registryRepository(registry) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-publisher-registry-'));
  git(root, ['init', '--initial-branch=main']);
  git(root, ['config', 'user.name', 'test']);
  git(root, ['config', 'user.email', 'test@example.com']);
  fs.mkdirSync(path.join(root, '.github'), { recursive: true });
  fs.writeFileSync(path.join(root, '.github', 'ai-executors.json'), registry);
  git(root, ['add', '.github/ai-executors.json']);
  git(root, ['commit', '-m', 'registry']);
  return root;
}

test('Publisher binds Candidate executor provenance from the exact clean base checkout', () => {
  const root = registryRepository(executorRegistry());
  try {
    const base = git(root, ['rev-parse', 'HEAD']).trim();
    fs.writeFileSync(path.join(root, '.github', 'ai-executors.json'), executorRegistry('0.148.1'));
    git(root, ['add', '.github/ai-executors.json']);
    git(root, ['commit', '-m', 'later registry']);
    const later = git(root, ['rev-parse', 'HEAD']).trim();
    git(root, ['checkout', '--detach', base]);
    assert.deepEqual(trustedCandidateExecutorForBase({ repositoryRoot: root, baseSha: base, repository }), candidateExecutor);
    assert.throws(
      () => trustedCandidateExecutorForBase({ repositoryRoot: root, baseSha: later, repository }),
      /checkout does not match candidate base SHA/,
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('Publisher rejects an invalid Candidate executor registry at the bound base', () => {
  const root = registryRepository('{"schema_version":1,"executors":[],"routes":{}}');
  try {
    const base = git(root, ['rev-parse', 'HEAD']).trim();
    assert.throws(
      () => trustedCandidateExecutorForBase({ repositoryRoot: root, baseSha: base, repository }),
      /trusted candidate executor registry is invalid/,
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

function git(root, args, options = {}) {
  return execFileSync('git', args, { cwd: root, encoding: options.encoding ?? 'utf8' });
}

test('prepareCommit applies data without executing candidate files or repository hooks', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'aeris-candidate-noexec-'));
  try {
    git(root, ['init']);
    git(root, ['config', 'core.autocrlf', 'false']);
    git(root, ['config', 'user.name', 'test']);
    git(root, ['config', 'user.email', 'test@example.com']);
    fs.writeFileSync(path.join(root, 'base.txt'), 'base\n');
    git(root, ['add', 'base.txt']);
    git(root, ['commit', '-m', 'base']);
    const actualBase = git(root, ['rev-parse', 'HEAD']).trim();

    const candidatePath = path.join(root, 'candidate-script.sh');
    fs.writeFileSync(candidatePath, '#!/bin/sh\nprintf executed > candidate-ran\n');
    git(root, ['add', 'candidate-script.sh']);
    const patchBytes = git(root, ['diff', '--cached', '--binary', 'HEAD', '--'], { encoding: 'buffer' });
    const patchPath = path.join(root, '.git', 'candidate.patch');
    fs.writeFileSync(patchPath, patchBytes);
    git(root, ['reset', '--', 'candidate-script.sh']);
    fs.rmSync(candidatePath);

    const hookPath = path.join(root, '.git', 'hooks', 'pre-commit');
    fs.writeFileSync(hookPath, '#!/bin/sh\nprintf hook > hook-ran\n');
    fs.chmodSync(hookPath, 0o700);

    const publisher = new LocalGitPublisher({ repositoryRoot: root, token: null, repository });
    const commit = publisher.prepareCommit({
      patchPath,
      verified: {
        paths: ['candidate-script.sh'],
        manifest: { ...manifest, base_sha: actualBase },
      },
    });
    assert.match(commit.sha, /^[0-9a-f]{40}$/);
    assert.equal(fs.existsSync(path.join(root, 'candidate-ran')), false);
    assert.equal(fs.existsSync(path.join(root, 'hook-ran')), false);
    assert.equal(fs.existsSync(candidatePath), true);
    assert.match(git(root, ['log', '-1', '--format=%B']), /Aeris-Autonomy-Executor-ID: codex-action-v1/);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
