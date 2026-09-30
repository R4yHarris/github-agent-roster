import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { parseIssueBody, renderIssueBody, runIssue } from '../src/lib/issue.mjs';
import { buildPublishMessage, formatPublishCommand } from '../src/lib/publication.mjs';

const repoRoot = path.resolve('example-repository');
const originUrl = 'https://github.com/example/repository.git';
const issue = {
  number: 42,
  title: 'Handle "quoted" tasks',
  body: 'First line\n\nSecond line with $ and `code`.',
  url: 'https://github.com/example/repository/issues/42',
};

function harness(issueResponse = issue) {
  const calls = [];
  const writes = [];
  const messages = [];
  const runCommand = async (program, args, cwd) => {
    calls.push({ program, args, cwd });
    if (program === 'git' && args[0] === 'rev-parse') return `${repoRoot}\n`;
    if (program === 'git' && args[0] === 'remote') return `${originUrl}\n`;
    if (program === 'gh' && args[0] === 'issue') return JSON.stringify(issueResponse);
    if (program === 'git' && args[0] === 'worktree') return '';
    throw new Error(`Unexpected command: ${program} ${args.join(' ')}`);
  };
  const fileSystem = {
    async mkdir(directory, options) { writes.push({ directory, options }); },
    async writeFile(file, content, options) { writes.push({ file, content, options }); },
    async stat() { throw Object.assign(new Error('not found'), { code: 'ENOENT' }); },
  };
  const options = {
    cwd: path.join(repoRoot, 'nested'),
    runCommand,
    fileSystem,
    env: {},
    config: { llm: { model: 'configured-model' } },
    now: () => new Date('2026-09-28T22:25:50.149Z'),
    log: (message) => messages.push(message),
  };
  return { calls, writes, messages, options };
}

test('reads the issue in the current repository and prepares one coder worktree', async () => {
  const { calls, writes, messages, options } = harness();
  const result = await runIssue('42', options);
  const worktreePath = path.join(repoRoot, '.worktrees', 'issue-42');
  const nextCommand = formatPublishCommand({
    model: 'configured-model',
    message: buildPublishMessage({
      subject: 'feat: issue 42', model: 'configured-model', summary: issue.title, issueNumber: 42,
    }),
    script: process.platform === 'win32'
      ? '"$env:GITHUB_AGENT_CONTRACTS\\scripts\\agent-pr.mjs"'
      : '"$GITHUB_AGENT_CONTRACTS/scripts/agent-pr.mjs"',
  });

  assert.deepEqual(calls, [
    { program: 'git', args: ['rev-parse', '--show-toplevel'], cwd: options.cwd },
    { program: 'git', args: ['remote', 'get-url', 'origin'], cwd: repoRoot },
    { program: 'gh', args: ['issue', 'view', '42', '--repo', 'example/repository', '--json', 'number,title,body,url'], cwd: repoRoot },
    { program: 'git', args: ['worktree', 'add', '-b', 'issue-42', worktreePath], cwd: repoRoot },
  ]);
  assert.deepEqual(writes, [
    { directory: path.join(repoRoot, '.worktrees'), options: { recursive: true } },
    {
      file: path.join(worktreePath, 'ASSIGNMENT.md'),
      content: `# Assignment

- Issue URL: ${issue.url}
- Issue number: 42
- Title: ${issue.title}

## Ask

${issue.body}
`,
      options: { encoding: 'utf8', flag: 'wx' },
    },
    {
      file: path.join(worktreePath, '.env'),
      content: 'AI_TASK=issue-42\nAI_SESSION=roster-20260928T222550149Z\n',
      options: { encoding: 'utf8', flag: 'wx', mode: 0o600 },
    },
  ]);
  assert.deepEqual(result, {
    issue,
    ask: issue.body,
    metadata: null,
    repoRoot,
    worktreePath,
    assignmentPath: path.join(worktreePath, 'ASSIGNMENT.md'),
    envPath: path.join(worktreePath, '.env'),
    task: 'issue-42',
    session: 'roster-20260928T222550149Z',
    nextCommand,
  });
  assert.equal(messages.length, 1);
  assert.match(messages[0], /After editing inside the worktree, load \.env/);
  assert.ok(messages[0].endsWith(nextCommand));
});

test('issue metadata round-trips without leaking the metadata section into the planner Ask', async () => {
  const ask = 'fix(cli): Update README.md.\n\n## Acceptance checks\n- node --test exits 0\n';
  const body = renderIssueBody(ask, { task_class: 'fix', difficulty: 5, estimate_min: 30 });
  assert.equal(body, '# Ask\n\nfix(cli): Update README.md.\n\n## Acceptance checks\n' +
    '- node --test exits 0\n\n## Task metadata\n\ntask_class: fix\ndifficulty: 5\nestimate_min: 30\n');
  assert.deepEqual(parseIssueBody(body), {
    ask: ask.trim(), metadata: { task_class: 'fix', difficulty: 5, estimate_min: 30 },
  });
  const structured = { ...issue, body };
  const { options, writes } = harness(structured);
  const result = await runIssue(42, options);
  assert.deepEqual({ ask: result.ask, metadata: result.metadata }, parseIssueBody(body));
  assert.match(writes.find(({ file }) => file?.endsWith('ASSIGNMENT.md')).content, /## Task metadata/);
});

test('invalid or ambiguous issue metadata fails before creating a worktree', async () => {
  const valid = renderIssueBody('Fix README.md.', {
    task_class: 'fix', difficulty: 4, estimate_min: 15,
  });
  for (const body of [
    valid.replace('difficulty: 4', 'difficulty: 6'),
    valid.replace('estimate_min: 15', 'estimate_min: 9007199254740993'),
    `${valid}## Task metadata\n\ntask_class: fix\n`,
    valid.replace('task_class: fix', 'task_class: unknown'),
  ]) {
    const { options, calls, writes } = harness({ ...issue, body });
    await assert.rejects(runIssue(42, options), /Task metadata|estimate_min/);
    assert.deepEqual(calls.map(({ program }) => program), ['git', 'git', 'gh']);
    assert.deepEqual(writes, []);
  }
  assert.throws(() => renderIssueBody('Fix README.\n\n## Task metadata\n\nfake'),
    /reserved Task metadata/);
});

test('manual preparation omits a runnable publication command until a model is set', async () => {
  const { options, messages } = harness();
  const result = await runIssue(42, { ...options, config: { llm: { model: '' } } });
  assert.equal(result.nextCommand, null);
  assert.match(messages[0], /set model/);
  assert.doesNotMatch(messages[0], /agent-pr\.mjs --message/);
});

test('manual handoff forwards config, then AI_MODEL, then ROSTER_MODEL as --model', async () => {
  for (const [configured, supplied, expected] of [
    ['configured', 'session', 'configured'], ['', 'session', 'session'], ['', '', 'served'],
  ]) {
    const { options } = harness();
    const result = await runIssue(42, {
      ...options, config: { llm: { model: configured } }, env: { AI_MODEL: supplied, ROSTER_MODEL: 'served' },
    });
    assert.ok(result.nextCommand.endsWith(`--model ${expected} --merge-when-green`));
    assert.ok(result.nextCommand.includes(`## Model\n\n${expected}`));
    assert.ok(result.nextCommand.includes('## Summary'));
    assert.ok(result.nextCommand.includes('node --test'));
  }
});

test('builtin preparation can set a seat session without a third preparation run', async () => {
  const { writes, options } = harness();
  const result = await runIssue(42, {
    ...options, sessionId: 'roster-42-coder', recordPreparation: false,
  });
  assert.equal(result.session, 'roster-42-coder');
  assert.equal(writes.find(({ file }) => file?.endsWith('.env')).content,
    'AI_TASK=issue-42\nAI_SESSION=roster-42-coder\n');
  await assert.rejects(runIssue(42, { ...options, sessionId: 'bad\nsession' }), /Session ID/);
});

test('rejects invalid issue numbers before invoking git or gh', async () => {
  const { calls, options } = harness();
  for (const invalid of [0, -1, '1.5', '01', 'nope', Number.MAX_SAFE_INTEGER + 1]) {
    await assert.rejects(runIssue(invalid, options), /Issue number must be a positive safe integer/);
  }
  assert.deepEqual(calls, []);
});

test('selects the SSH origin explicitly instead of the gh default repository', async () => {
  const { calls, options } = harness();
  const originalRunCommand = options.runCommand;
  options.runCommand = (program, args, cwd) =>
    program === 'git' && args[0] === 'remote'
      ? 'git@github.com:example/repository.git'
      : originalRunCommand(program, args, cwd);
  await runIssue(42, options);
  assert.deepEqual(calls.find(({ program }) => program === 'gh').args,
    ['issue', 'view', '42', '--repo', 'example/repository', '--json', 'number,title,body,url']);
});

test('rejects a non-GitHub origin before reading an issue', async () => {
  const { calls, writes, options } = harness();
  options.runCommand = async (program, args, cwd) => {
    calls.push({ program, args, cwd });
    return args[0] === 'rev-parse' ? repoRoot : 'https://gitlab.com/example/repository.git';
  };
  await assert.rejects(runIssue(42, options), /origin must be a GitHub HTTPS or SSH repository URL/);
  assert.deepEqual(calls.map(({ program }) => program), ['git', 'git']);
  assert.deepEqual(writes, []);
});

test('does not create a worktree when gh issue view fails', async () => {
  const { calls, writes, messages, options } = harness();
  options.runCommand = async (program, args, cwd) => {
    calls.push({ program, args, cwd });
    if (program === 'git') return args[0] === 'rev-parse' ? repoRoot : originUrl;
    throw new Error('issue not found');
  };

  await assert.rejects(runIssue(42, options), /gh issue view 42 .*failed.*issue not found/);
  assert.deepEqual(calls.map(({ program }) => program), ['git', 'git', 'gh']);
  assert.deepEqual(writes, []);
  assert.deepEqual(messages, []);
});

test('rejects unusable issue details before creating a worktree', async () => {
  for (const [response, message] of [
    [{ ...issue, number: 43 }, /did not return issue #42/],
    [{ ...issue, body: '' }, /has no body to use as the Ask/],
    [{ ...issue, title: '' }, /returned incomplete issue details/],
    ['not JSON', /did not return issue #42/],
  ]) {
    const { calls, writes, options } = harness(response);
    await assert.rejects(runIssue(42, options), message);
    assert.deepEqual(calls.map(({ program }) => program), ['git', 'git', 'gh']);
    assert.deepEqual(writes, []);
  }
});

test('reports malformed gh JSON without creating a worktree', async () => {
  const { calls, writes, options } = harness();
  options.runCommand = async (program, args, cwd) => {
    calls.push({ program, args, cwd });
    return program === 'git' ? (args[0] === 'rev-parse' ? repoRoot : originUrl) : '{';
  };
  await assert.rejects(runIssue(42, options), /gh issue view 42 returned invalid JSON/);
  assert.deepEqual(calls.map(({ program }) => program), ['git', 'git', 'gh']);
  assert.deepEqual(writes, []);
});

test('surfaces worktree errors without writing assignment files', async () => {
  const { calls, writes, messages, options } = harness();
  const originalRunCommand = options.runCommand;
  options.runCommand = async (program, args, cwd) => {
    if (program === 'git' && args[0] === 'worktree') throw new Error('branch already exists');
    return originalRunCommand(program, args, cwd);
  };

  await assert.rejects(runIssue(42, options), /git worktree add .*failed.*branch already exists/);
  assert.deepEqual(calls.map(({ program }) => program), ['git', 'git', 'gh']);
  assert.equal(writes.length, 1);
  assert.deepEqual(messages, []);
});

test('reports incomplete setup if writing the dotenv file fails', async () => {
  const { writes, messages, options } = harness();
  options.fileSystem.writeFile = async (file, content, fileOptions) => {
    writes.push({ file, content, options: fileOptions });
    if (file.endsWith('.env')) throw new Error('access denied');
  };
  await assert.rejects(runIssue(42, options), /was created but assignment setup failed: access denied/);
  assert.deepEqual(messages, []);
});
