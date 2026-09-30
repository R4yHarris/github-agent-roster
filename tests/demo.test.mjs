import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync,
  readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { runDemo } from '../src/lib/demo.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { parseRecipe } from '../src/lib/recipe.mjs';

const sourceRoot = fileURLToPath(new URL('../', import.meta.url));

function fixture(t) {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-demo-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const repoRoot = path.join(root, 'roster');
  const tempRoot = path.join(root, 'worktrees');
  mkdirSync(path.join(repoRoot, 'templates', 'sdlc'), { recursive: true });
  mkdirSync(path.join(repoRoot, 'fixtures', 'demo-task'), { recursive: true });
  mkdirSync(path.join(repoRoot, 'principals'));
  copyFileSync(path.join(sourceRoot, 'principals', 'coder.md'),
    path.join(repoRoot, 'principals', 'coder.md'));
  copyFileSync(path.join(sourceRoot, 'principals', 'reviewer.md'),
    path.join(repoRoot, 'principals', 'reviewer.md'));
  cpSync(path.join(sourceRoot, 'skills'), path.join(repoRoot, 'skills'), { recursive: true });
  mkdirSync(tempRoot);
  copyFileSync(path.join(sourceRoot, 'roster.config.example.yml'),
    path.join(repoRoot, 'roster.config.example.yml'));
  for (const name of ['ASK.md', 'RECIPE.yml', 'TASK.md', 'ASSIGNMENT.md']) {
    copyFileSync(path.join(sourceRoot, 'templates', 'sdlc', name),
      path.join(repoRoot, 'templates', 'sdlc', name));
  }
  for (const name of ['ASK.md', 'AGENTS.md', 'README.md']) {
    copyFileSync(path.join(sourceRoot, 'fixtures', 'demo-task', name),
      path.join(repoRoot, 'fixtures', 'demo-task', name));
  }
  return { root, repoRoot, tempRoot };
}

test('stub seats write RECIPE, TASK, RESULT, and failing REVIEW in an isolated temp worktree', async (t) => {
  const { repoRoot, tempRoot } = fixture(t);
  const example = readFileSync(path.join(repoRoot, 'roster.config.example.yml'), 'utf8');
  const configured = parseConfig(example.replace('profile: ""', 'profile: ollama')
    .replace('model: ""', 'model: local-model'));
  const demo = await runDemo({
    repoRoot, tempRoot, config: configured,
    askFile: path.join(repoRoot, 'templates', 'sdlc', 'ASK.md'),
  });
  assert.equal(demo.mode, 'stub');
  assert.equal(path.dirname(demo.worktreePath), tempRoot);
  assert.equal(existsSync(path.join(demo.worktreePath, '.git')), false);
  assert.deepEqual(parseRecipe(readFileSync(demo.recipePath, 'utf8')).seats.map(({ id }) => id),
    ['planner', 'coder', 'reviewer']);
  assert.match(readFileSync(demo.recipePath, 'utf8'), /ask: local:demo-/);
  assert.match(readFileSync(demo.taskPath, 'utf8'), /# Task: Add a Status section/);
  assert.match(readFileSync(demo.taskPath, 'utf8'), /## Files allowed\n- `README\.md`/);
  assert.match(readFileSync(demo.resultPath, 'utf8'),
    /Deterministic stub only: no implementation or tests were run/);
  assert.match(readFileSync(demo.reviewPath, 'utf8'), /Verdict: fail/);
  assert.equal(readFileSync(path.join(demo.worktreePath, 'README.md'), 'utf8'),
    readFileSync(path.join(repoRoot, 'fixtures', 'demo-task', 'README.md'), 'utf8'));
  for (const seat of ['planner', 'coder']) {
    const record = JSON.parse(readFileSync(path.join(repoRoot, '.roster', 'memory', `${seat}.jsonl`),
      'utf8'));
    assert.equal(record.status, 'stub');
    assert.match(record.session, new RegExp(`^roster-demo-[a-f0-9]+-${seat}$`));
  }
});

test('raw fixture Ask works, while path escapes fail before creating a worktree', async (t) => {
  const { root, repoRoot, tempRoot } = fixture(t);
  const demo = await runDemo({
    repoRoot, tempRoot,
    askFile: path.join(repoRoot, 'fixtures', 'demo-task', 'ASK.md'),
  });
  assert.equal(demo.mode, 'stub');
  assert.match(readFileSync(demo.taskPath, 'utf8'), /Status section/);
  const outside = path.join(root, 'outside.md');
  writeFileSync(outside, 'Outside source\n');
  await assert.rejects(runDemo({ repoRoot, tempRoot, askFile: outside }), /inside the roster repository/);
  assert.equal(readdirSync(tempRoot).length, 1);
});

test('the exact CLI template command runs without GitHub and leaves inspectable stub output', (t) => {
  const { repoRoot } = fixture(t);
  cpSync(path.join(sourceRoot, 'src'), path.join(repoRoot, 'src'), { recursive: true });
  const cli = path.join(repoRoot, 'src', 'cli.mjs');
  const result = spawnSync(process.execPath, [
    cli, 'run', '--ask-file', path.join('templates', 'sdlc', 'ASK.md'),
    '--runtime', 'builtin',
  ], { cwd: repoRoot, encoding: 'utf8', timeout: 10_000 });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  const worktree = /^Worktree: (.+)$/m.exec(result.stdout)?.[1]?.trim();
  assert.ok(worktree, result.stdout);
  assert.equal(path.dirname(worktree), tmpdir());
  assert.match(path.basename(worktree), /^roster-demo-/);
  t.after(() => rmSync(worktree, { recursive: true, force: true }));
  assert.match(result.stdout, /^Mode: stub$/m);
  assert.match(result.stdout, /^REVIEW: .+REVIEW\.md$/m);
  assert.match(readFileSync(path.join(worktree, 'RESULT.md'), 'utf8'),
    /no implementation or tests were run/);
});
