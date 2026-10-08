// Shared fixtures for the tests/builtin-*.test.mjs files (split for parallel test runs).
import { execFileSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { parseConfig } from '../../src/lib/config.mjs';
import { runBuiltinIssue as runIssueWithSeats } from '../../src/lib/builtin.mjs';
import { withResearchSummary } from './research.mjs';

export function runBuiltinIssue(issue, options) {
  return runIssueWithSeats(issue, { ...options, fetchImpl: withResearchSummary(options.fetchImpl) });
}

export const example = readFileSync(new URL('../../roster.config.example.yml', import.meta.url), 'utf8');
export const stubConfig = parseConfig(example);
export const llmConfig = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:1234/v1')
  .replace('model: ""', 'model: local-model'));
export const vllmConfig = parseConfig(example.replace('profile: ""', 'profile: vllm-local')
  .replace('model: ""', 'model: local-model'));
export const multiFileScope = ['README.md', 'smoke.test.mjs'];

export function git(cwd, ...args) {
  return execFileSync('git', args, { cwd, encoding: 'utf8' }).trim();
}

export function fixture(context) {
  const base = mkdtempSync(path.join(tmpdir(), 'roster-builtin-'));
  context.after(() => rmSync(base, { recursive: true, force: true }));
  const repoRoot = path.join(base, 'roster');
  const target = path.join(base, 'project');
  const contracts = path.join(base, 'contracts');
  const machineRoot = path.join(base, 'machine');
  mkdirSync(machineRoot);
  mkdirSync(repoRoot);
  mkdirSync(path.join(repoRoot, 'principals'));
  writeFileSync(path.join(repoRoot, 'principals', 'coder.md'),
    readFileSync(new URL('../../principals/coder.md', import.meta.url), 'utf8'));
  writeFileSync(path.join(repoRoot, 'principals', 'reviewer.md'),
    readFileSync(new URL('../../principals/reviewer.md', import.meta.url), 'utf8'));
  mkdirSync(target);
  cpSync(new URL('../../skills/', import.meta.url), path.join(repoRoot, 'skills'), { recursive: true });
  cpSync(new URL('../../examples/', import.meta.url), path.join(repoRoot, 'examples'), { recursive: true });
  mkdirSync(path.join(contracts, 'scripts'), { recursive: true });
  writeFileSync(path.join(repoRoot, 'roster.config.example.yml'), example);
  writeFileSync(path.join(repoRoot, 'skills', 'implement-task', 'SKILL.md'), '# Code and test\n');
  writeFileSync(path.join(contracts, 'scripts', 'agent-pr.mjs'), 'export {};\n');
  writeFileSync(path.join(target, '.gitignore'), '.env\n.worktrees/\n.roster/runs/\n.roster/logs/\n.roster/fleet.yml\n.roster/config.yml\n');
  writeFileSync(path.join(target, 'AGENTS.md'), '# Agent instructions\nStay in the worktree.\n');
  writeFileSync(path.join(target, 'README.md'), '# Example\n');
  writeFileSync(path.join(target, 'smoke.test.mjs'),
    "import test from 'node:test';\nimport assert from 'node:assert/strict';\n" +
    "test('smoke', () => assert.equal(1, 1));\n");
  git(target, 'init', '-b', 'main');
  git(target, 'add', '--all');
  git(target, '-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid',
    'commit', '-m', 'Fixture setup');
  git(target, 'remote', 'add', 'origin', 'https://github.com/example/project.git');
  const cwd = path.join(target, 'nested');
  mkdirSync(cwd);
  // The fixture is a human CLI: a suite run inside a coder seat must not leak ROSTER_SEAT into it.
  const env = { ...process.env, ROSTER_MODEL: '', AI_MODEL: '', AI_MODEL_VERSION: '', ROSTER_SEAT: undefined,
    PATHS_OVERRIDE: undefined, ROSTER_STATE_ROOT: machineRoot,
    GITHUB_APP_ID: undefined, GITHUB_APP_PRIVATE_KEY_PATH: undefined,
    GITHUB_AGENT_CONTRACTS: contracts };
  const issue = {
    number: 42, title: 'Add Status to README',
    body: 'Add a Status section to README.md.\n\n## Acceptance checks\n' +
      '- node --test exits 0\n- README has a Status section\n\n## Files allowed\n- `README.md`\n',
    url: 'https://github.com/example/project/issues/42',
  };
  const calls = [];
  let stderr = '';
  const errorOutput = { write(text) { stderr += String(text); } };
  const runCommand = async (program, args, workingDirectory) => {
    calls.push({ program, args, workingDirectory });
    if (program === 'gh') return JSON.stringify(issue);
    // Hermetic: never reach the fake GitHub origin from tests.
    if (program === 'git' && args[0] === 'fetch') return '';
    return git(workingDirectory, ...args);
  };
  return { base, repoRoot, target, cwd, env, contracts, issue, calls, runCommand, errorOutput,
    get stderr() { return stderr; } };
}

export function multiFileFixture(context) {
  const options = fixture(context);
  options.issue.body = options.issue.body.replace(
    '## Files allowed\n- `README.md`\n',
    '## Files allowed\n- `README.md`\n- `smoke.test.mjs`\n',
  );
  return options;
}
