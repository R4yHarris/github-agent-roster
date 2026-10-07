import { execFile } from 'node:child_process';
import { promises as fs } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { createTools, isForbiddenRead } from './tools.mjs';

const execute = promisify(execFile);

export const maxReviewChars = 6000;
export const maxReviewReadRounds = 3;
export const maxReviewReadsPerRound = 4;
export const maxEndToEndCommands = 3;
export const endToEndPrefix = 'End-to-end:';
const endToEndTimeoutMs = 30_000;
const outputChars = 3000;
const diffChars = 20_000;

// Read-only roster subcommands the harness may execute; anything that asks, runs seats, publishes,
// probes endpoints, or touches the vault is never run by the reviewer step.
const readOnlyCommands = new Map([
  ['history', new Set(['list', 'show'])],
  ['status', null],
  ['recipe', new Set(['validate'])],
  ['stats', null],
  ['recommend', null],
  ['doctor', null],
  ['fleet', new Set(['list'])],
]);
const invalidOption = '--roster-review-invalid-option';

export const reviewToolNames = Object.freeze(['read_file', 'search_text', 'git_diff']);

// The reviewer reads through a JSON request channel, not native tool calls, so every reviewer turn stays a
// strict json_object response and the seat never sees a write tool.
export function parseReviewRequests(content) {
  let value;
  try { value = JSON.parse(content); } catch { return null; }
  if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).join(',') !== 'requests') return null;
  const { requests } = value;
  if (!Array.isArray(requests) || !requests.length || requests.length > maxReviewReadsPerRound) {
    throw new Error(`Reviewer requests must list 1-${maxReviewReadsPerRound} reads`);
  }
  return requests.map((request) => {
    if (!request || typeof request !== 'object' || Array.isArray(request) || typeof request.tool !== 'string') {
      throw new Error('Each reviewer request must be {tool, path} or {tool, query, path?}');
    }
    const args = Object.fromEntries(Object.entries(request).filter(([key]) => key !== 'tool'));
    if (Object.values(args).some((arg) => typeof arg !== 'string' || arg.length > 500)) {
      throw new Error('Reviewer request arguments must be short strings');
    }
    return { tool: request.tool, args };
  });
}

export async function answerReviewRequests(requests, tools) {
  const answers = [];
  for (const { tool, args } of requests) {
    let content;
    if (!reviewToolNames.includes(tool)) {
      content = `Refused: the reviewer is read-only; only ${reviewToolNames.join(', ')} are available.`;
    } else {
      try {
        const result = await tools[tool](args);
        content = typeof result === 'string' ? result : JSON.stringify(result);
      } catch (error) {
        if (!(error instanceof Error)) throw error;
        content = `Denied: ${error.message.split('\n')[0]}`;
      }
    }
    answers.push({ tool, args, content: content.slice(0, diffChars) });
  }
  return answers;
}

// Built on the coder's read guards; only the read methods are exposed, so the write scope is unreachable.
export async function createReviewTools({ worktree, allowedFiles, runCommand = execute } = {}) {
  const tools = await createTools({ worktree, allowedFiles, seat: 'coder', allowRunTest: false });
  return {
    read_file: (args) => tools.read_file(args),
    search_text: (args) => tools.search_text(args),
    async git_diff(args = {}) {
      const file = args.path === undefined ? undefined : String(args.path).replaceAll('\\', '/');
      if (file !== undefined && (!file || path.isAbsolute(file) || isForbiddenRead(file))) {
        throw new Error('git_diff path must be a readable worktree file');
      }
      const { stdout } = await runCommand('git', ['--literal-pathspecs', 'diff', '--no-ext-diff', '--no-textconv',
        '--no-renames', 'HEAD', ...(file ? ['--', file] : [])], { cwd: worktree, encoding: 'utf8', maxBuffer: 4 * diffChars });
      if (stdout.trim() || !file) return stdout.slice(0, diffChars) || 'No tracked changes.';
      return `${file} is untracked or unchanged; use read_file to see a new file.`;
    },
  };
}

function tokenize(command) {
  if (/[|;&<>`$(){}\r\n]/.test(command)) return null;
  const words = [];
  for (const match of command.matchAll(/"([^"]*)"|'([^']*)'|(\S+)/g)) words.push(match[1] ?? match[2] ?? match[3]);
  return words;
}

// Map a backticked check command to `node src/cli.mjs ...` when it is a read-only roster subcommand.
export function endToEndCommands(checkTexts = []) {
  const commands = [];
  for (const [index, check] of checkTexts.entries()) {
    for (const match of String(check).matchAll(/`([^`\n]+)`/g)) {
      const words = tokenize(match[1].trim());
      if (!words?.length) continue;
      let args;
      if (words[0] === 'roster') args = words.slice(1);
      else if (words[0] === 'node' && ['src/cli.mjs', './src/cli.mjs', 'bin/roster.mjs'].includes(words[1])) args = words.slice(2);
      else continue;
      const subcommands = readOnlyCommands.get(args[0]);
      if (subcommands === undefined || subcommands && !subcommands.has(args[1]) ||
          args.includes('--warm') || args.includes('--discover')) continue;
      if (args[0] === 'status' && !args.includes('--offline')) args.push('--offline');
      const command = ['roster', ...args].join(' ');
      if (commands.some((entry) => entry.command === command)) continue;
      commands.push({ check: index + 1, command, args,
        expectFailure: /\b(?:errors?|fail(?:s|ure)?|reject(?:s|ed)?|refuses?|non-?zero|invalid|exits? [1-9])\b/i.test(check) });
      if (commands.length === maxEndToEndCommands) return commands;
    }
  }
  return commands;
}

async function sandboxEnv(home, env) {
  const keep = ['PATH', 'Path', 'PATHEXT', 'SystemRoot', 'SYSTEMROOT', 'ComSpec', 'COMSPEC', 'windir', 'LANG'];
  const sandbox = Object.fromEntries(keep.filter((name) => env[name] !== undefined).map((name) => [name, env[name]]));
  for (const name of ['HOME', 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'TEMP', 'TMP']) {
    sandbox[name] = home;
  }
  sandbox.GH_CONFIG_DIR = path.join(home, 'gh');
  await fs.mkdir(sandbox.GH_CONFIG_DIR);
  return { ...sandbox, GIT_TERMINAL_PROMPT: '0', NO_COLOR: '1', CI: '1' };
}

function crashed(text) {
  return /^\s+at .+\(?(?:file:|node:|[A-Za-z]:\\|\/).+:\d+/m.test(text) || /^(?:Type|Reference|Syntax|Range)Error: /m.test(text);
}

function jsonOutput(args, check) {
  return args.includes('--json') || args.join(' ').includes('--format json') || /\bJSON\b/.test(check);
}

function parsesAsJson(text) {
  const trimmed = text.trim();
  if (!trimmed) return false;
  try { JSON.parse(trimmed); return true; } catch { /* try JSON lines */ }
  return trimmed.split('\n').every((line) => {
    try { JSON.parse(line); return true; } catch { return false; }
  });
}

async function runOne({ worktree, args, env, runCommand }) {
  try {
    const { stdout, stderr } = await runCommand(process.execPath, ['src/cli.mjs', ...args],
      { cwd: worktree, env, encoding: 'utf8', timeout: endToEndTimeoutMs, maxBuffer: 1 << 20, windowsHide: true });
    return { exitCode: 0, stdout: String(stdout ?? ''), stderr: String(stderr ?? '') };
  } catch (error) {
    if (error?.code === 'ENOENT' || error?.code === 'EACCES') throw error;
    return { exitCode: Number.isInteger(error?.code) ? error.code : 1, stdout: String(error?.stdout ?? ''),
      stderr: String(error?.stderr ?? error?.message ?? ''), timedOut: error?.killed === true };
  }
}

// Run each read-only CLI path the checks name on the worktree's real data, plus one invalid-option error case.
export async function runEndToEnd({ worktree, checkTexts, env = process.env, runCommand = execute } = {}) {
  const commands = endToEndCommands(checkTexts);
  if (!commands.length) return { status: 'none', runs: [], failures: [] };
  try {
    if (!(await fs.lstat(path.join(worktree, 'src', 'cli.mjs'))).isFile()) throw new Error('not a file');
  } catch {
    return { status: 'none', runs: [], failures: [] };
  }
  const home = await fs.mkdtemp(path.join(os.tmpdir(), 'roster-review-'));
  const runs = [];
  const failures = [];
  try {
    const sandbox = await sandboxEnv(home, env);
    for (const entry of commands) {
      const check = checkTexts[entry.check - 1];
      const real = await runOne({ worktree, args: entry.args, env: sandbox, runCommand });
      const output = `${real.stdout}\n${real.stderr}`;
      const problems = [];
      if (real.timedOut) problems.push(`timed out after ${endToEndTimeoutMs / 1000}s`);
      else if (crashed(output)) problems.push('crashed with a stack trace');
      else if (entry.expectFailure ? real.exitCode === 0 : real.exitCode !== 0) {
        problems.push(entry.expectFailure ? 'exited 0 where check expects an error' : `exited ${real.exitCode}`);
      } else if (!entry.expectFailure && jsonOutput(entry.args, check) && !parsesAsJson(real.stdout)) {
        problems.push('stdout is not valid JSON');
      }
      const invalid = await runOne({ worktree, args: [...entry.args, invalidOption], env: sandbox, runCommand });
      const invalidOutput = `${invalid.stdout}\n${invalid.stderr}`;
      if (invalid.exitCode === 0) problems.push(`accepted the unknown option ${invalidOption}`);
      else if (crashed(invalidOutput) || invalid.timedOut) problems.push(`crashed on the unknown option ${invalidOption}`);
      runs.push({ ...entry, exitCode: real.exitCode, output: output.trim().slice(0, outputChars),
        errorCase: { exitCode: invalid.exitCode, output: invalidOutput.trim().slice(0, 500) }, problems });
      for (const problem of problems) failures.push(`${endToEndPrefix} \`${entry.command}\` (check ${entry.check}) ${problem}.`);
    }
  } finally {
    await fs.rm(home, { recursive: true, force: true });
  }
  return { status: failures.length ? 'fail' : 'pass', runs, failures };
}

export function endToEndSection(result) {
  if (!result?.runs?.length) return '';
  return '## Harness end-to-end runs\n\nThe harness ran these read-only CLI paths from the checks in the worktree ' +
    '(no credentials, temporary HOME) and each with one unknown option. Judge the named checks against this output.\n\n' +
    result.runs.map((run) => `### \`${run.command}\` (check ${run.check})\n\nExit ${run.exitCode}` +
      `${run.problems.length ? `; problems: ${run.problems.join('; ')}` : ''}\n\n\`\`\`text\n${run.output || '(no output)'}\n\`\`\`\n\n` +
      `Error case \`${invalidOption}\`: exit ${run.errorCase.exitCode}\n\n\`\`\`text\n${run.errorCase.output || '(no output)'}\n\`\`\``)
      .join('\n\n') + '\n\n';
}
