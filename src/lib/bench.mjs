import { execFile } from 'node:child_process';
import { promises as fs } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import { promisify } from 'node:util';
import { loadConfig } from './config.mjs';

const execute = promisify(execFile);
const fields = Object.freeze([
  'process_start_ms',
  'config_load_ms',
  'command_dispatch_ms',
  'git_worktree_add_ms',
  'submodule_init_ms',
  'mocked_model_round_trip_ms',
]);

function elapsed(started) {
  return Number(Math.max(0, performance.now() - started).toFixed(3));
}

function gitEnvironment() {
  const names = ['PATH', 'SYSTEMROOT', 'WINDIR', 'TEMP', 'TMP', 'HOME', 'USERPROFILE'];
  return Object.fromEntries(names.filter((name) => process.env[name])
    .map((name) => [name, process.env[name]]).concat([
      ['GIT_CONFIG_NOSYSTEM', '1'],
      ['GIT_TERMINAL_PROMPT', '0'],
    ]));
}

export async function runBench({ cwd = process.cwd(), repoRoot = cwd, commandDispatchMs = 0 } = {}) {
  if (typeof cwd !== 'string' || !cwd || typeof repoRoot !== 'string' || !repoRoot ||
      !Number.isFinite(commandDispatchMs) || commandDispatchMs < 0) {
    throw new TypeError('Benchmark requires paths and a nonnegative command dispatch duration');
  }
  const report = {
    process_start_ms: Number((process.uptime() * 1000).toFixed(3)),
  };

  let started = performance.now();
  loadConfig({ repoRoot, cwd });
  report.config_load_ms = elapsed(started);
  report.command_dispatch_ms = Number(commandDispatchMs.toFixed(3));

  const temporaryRoot = await fs.mkdtemp(path.join(os.tmpdir(), 'roster-bench-'));
  try {
    const repository = path.join(temporaryRoot, 'repository');
    const contracts = path.join(temporaryRoot, 'contracts');
    const worktree = path.join(temporaryRoot, 'worktree');
    const env = gitEnvironment();
    await fs.mkdir(repository);
    await fs.mkdir(contracts);
    await execute('git', ['init', '--quiet'], { cwd: repository, env, encoding: 'utf8' });
    await execute('git', ['init', '--quiet'], { cwd: contracts, env, encoding: 'utf8' });
    await fs.writeFile(path.join(repository, 'README.md'), '# benchmark\n', 'utf8');
    await fs.writeFile(path.join(contracts, 'agent-pr.mjs'), 'export {};\n', 'utf8');
    await execute('git', ['add', 'README.md'], { cwd: repository, env, encoding: 'utf8' });
    await execute('git', ['add', 'agent-pr.mjs'], { cwd: contracts, env, encoding: 'utf8' });
    const identity = ['-c', 'user.name=Roster Bench', '-c', 'user.email=bench@localhost'];
    await execute('git', [...identity, 'commit', '--quiet', '-m', 'contracts fixture'], {
      cwd: contracts, env, encoding: 'utf8',
    });
    await execute('git', ['-c', 'protocol.file.allow=always', 'submodule', 'add', '--quiet',
      contracts, 'vendor/github-agent-contracts'], { cwd: repository, env, encoding: 'utf8' });
    await execute('git', ['add', '.gitmodules', 'vendor/github-agent-contracts'], {
      cwd: repository, env, encoding: 'utf8',
    });
    await execute('git', ['-c', 'user.name=Roster Bench', '-c', 'user.email=bench@localhost',
      'commit', '--quiet', '-m', 'benchmark'], { cwd: repository, env, encoding: 'utf8' });

    started = performance.now();
    await execute('git', ['worktree', 'add', '--quiet', '--detach', worktree], {
      cwd: repository, env, encoding: 'utf8',
    });
    report.git_worktree_add_ms = elapsed(started);

    started = performance.now();
    await execute('git', ['-c', 'protocol.file.allow=always', 'submodule', 'update', '--init', '--recursive'], {
      cwd: worktree, env, encoding: 'utf8',
    });
    report.submodule_init_ms = elapsed(started);

    started = performance.now();
    await Promise.resolve({ choices: [{ message: { content: 'benchmark' } }] });
    report.mocked_model_round_trip_ms = elapsed(started);
  } finally {
    await fs.rm(temporaryRoot, { recursive: true, force: true });
  }

  if (Object.keys(report).length !== fields.length || fields.some((field) =>
    !Number.isFinite(report[field]) || report[field] < 0)) {
    throw new Error('Benchmark produced an invalid timing report');
  }
  const reportPath = path.join(cwd, '.roster', 'bench.json');
  await fs.mkdir(path.dirname(reportPath), { recursive: true });
  await fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, { encoding: 'utf8', mode: 0o600 });
  return { report, fields };
}
