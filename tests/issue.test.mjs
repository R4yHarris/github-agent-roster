import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { runIssue } from '../src/lib/issue.mjs';

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
  };
  const options = {
    cwd: path.join(repoRoot, 'nested'),
    runCommand,
    fileSystem,
    now: () => new Date('2026-09-28T22:25:50.149Z'),
    log: (message) => messages.push(message),
  };
  return { calls, writes, messages, options };
}

test('reads the issue in the current repository and prepares one coder worktree', async () => {
  const { calls, writes, messages, options } = harness();
  const result = await runIssue('42', options);
  const worktreePath = path.join(repoRoot, '.worktrees', 'issue-42');
  const nextCommand = 'node $GITHUB_AGENT_CONTRACTS/scripts/agent-pr.mjs --message "feat: issue 42"';

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
