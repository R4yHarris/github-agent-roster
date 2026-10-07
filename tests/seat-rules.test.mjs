import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { loadContext } from '../src/runtime/context.mjs';
import { omittedRulesNote, seatRules } from '../src/runtime/seat-rules.mjs';
import { runCoder } from '../src/seats/coder.mjs';
import { withResearchSummary } from './helpers/research.mjs';

const agents = readFileSync(new URL('../AGENTS.md', import.meta.url), 'utf8');
const publication = /agent-pr|GITHUB_APP|merge-when-green|gh pr create|git push|AI_MODEL|AI_PROVIDER|private-key/i;

test('seat rules keep this repo\'s coding rules and drop publication, credentials, and session metadata', () => {
  const rules = seatRules(agents);
  assert.match(agents, publication, 'the fixture must contain publication rules to drop');
  assert.doesNotMatch(rules, publication);
  assert.doesNotMatch(rules, /^## Publish metadata|^## Contracts dependency/m);
  for (const kept of [/## Product spec/, /## Hard boundaries/, /Do not invent a Kanban database/,
    /Do not commit PEMs, tokens, or `\.env`/, /Never edit `\.github\/workflows\/\*`/]) assert.match(rules, kept);
  assert.ok(rules.endsWith(omittedRulesNote));
  assert.ok(rules.length < agents.length * 0.6, `${rules.length} of ${agents.length}`);
});

test('rules with nothing to omit pass through unchanged, and a nested item leaves with its parent', () => {
  assert.equal(seatRules('# Rules\r\nRead before editing.\r\n'), '# Rules\nRead before editing.');
  const sliced = seatRules('## Boundaries\n\n- Keep diffs small.\n- Publish with agent-pr.\n  Use --merge-when-green.\n' +
    '- Test first.\n\n## Release\n\n- Only via git push.\n');
  assert.equal(sliced, `## Boundaries\n\n- Keep diffs small.\n- Test first.\n\n${omittedRulesNote}`);
  assert.throws(() => seatRules(null), /AGENTS\.md must be text/);
});

function fixture(context, checks) {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-seat-rules-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = path.join(repoRoot, 'worktree');
  mkdirSync(worktree);
  mkdirSync(path.join(repoRoot, 'principals'));
  writeFileSync(path.join(repoRoot, 'principals', 'coder.md'), '# Conduct\nStay within scope.\n');
  const task = planStub('Update `README.md` with a Status section; keep `smoke.test.mjs` in scope.',
    { reference: 'issue:4', metadata: { task_class: 'feat', difficulty: 4 } }).task.replace(/^skills:.*$/m, 'skills: []');
  writeFileSync(path.join(worktree, 'TASK.md'), task.replace(/(## Acceptance checks\n)(?:- .*\n)+/,
    `$1${checks.map((check) => `- ${check}`).join('\n')}\n`));
  writeFileSync(path.join(worktree, 'AGENTS.md'), agents);
  writeFileSync(path.join(worktree, 'README.md'), '# Example\n');
  writeFileSync(path.join(worktree, 'smoke.test.mjs'), '// fixture\n');
  return { repoRoot, worktree, memoryPath: path.join(repoRoot, '.roster', 'memory', 'coder.jsonl') };
}

const checks = ['README.md has a `## Status` section', '`node --test` exits 0', 'No other files change'];

test('the coder pack carries scoped AGENTS.md rules, never publication or credential rules', async (context) => {
  const options = fixture(context, checks);
  const result = await loadContext({ ...options, config: { seat: { context_chars: 200000 } } });
  assert.match(result.pack, /## AGENTS\.md\n\n# AGENTS\.md[\s\S]*## Hard boundaries/);
  assert.doesNotMatch(result.pack, publication);
  assert.equal(result.agents, agents.replace(/\r\n/g, '\n'), 'the raw AGENTS.md stays available to the harness');
});

test('the coder user turn is a numbered checklist that maps 1:1 to TASK.md checks', async (context) => {
  const options = fixture(context, checks);
  const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
  const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
    .replace('model: ""', 'model: local-model').replace('turn_budget: 1000', 'turn_budget: 3'));
  let turn;
  await runCoder({
    ...options, config, task: 'issue-4', session: 'roster-session', env: {},
    memoryPath: path.join(options.repoRoot, config.paths.memory),
    fetchImpl: withResearchSummary(async (_url, request) => {
      turn ??= JSON.parse(request.body).messages[1].content;
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Done.' } }] }) };
    }),
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  }).catch(() => {});
  const numbered = turn.split('\n').filter((line) => /^\d+\. /.test(line));
  assert.deepEqual(numbered, checks.map((check, index) => `${index + 1}. ${check}`));
  assert.match(turn, /\nRules:\n- Work to completion/);
  assert.match(turn, /naming each numbered check as done or blocked\.$/);
});
