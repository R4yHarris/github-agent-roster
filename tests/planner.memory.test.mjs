import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { appendMemory } from '../src/runtime/memory.mjs';
import { runPlanner } from '../src/seats/planner.mjs';

const llmConfig = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
  .replace('base_url: ""', 'base_url: "http://localhost:1234/v1"').replace('model: ""', 'model: test-model'));

for (const kind of ['feature', 'initiative']) {
  test(`${kind} outline receives only recent own-seat planner memory and preserves structured Ask`, async (context) => {
    const repoRoot = mkdtempSync(join(tmpdir(), 'roster-outline-memory-'));
    context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
    const worktree = join(repoRoot, 'worktree');
    mkdirSync(worktree);
    for (let index = 0; index < 25; index += 1) {
      await appendMemory({ repoRoot, file: join(repoRoot, '.roster', 'memory', 'planner.jsonl'), env: {},
        record: { index, summary: `planner-lesson-${index}` } });
    }
    await appendMemory({ repoRoot, file: join(repoRoot, '.roster', 'memory', 'coder.jsonl'), env: {},
      record: { summary: 'coder-only-private-lesson' } });
    const ask = 'Plan updates to README.md as independent bounded tasks.';
    const plan = await runPlanner({ repoRoot, worktree, ask, askKind: kind, config: llmConfig, env: {},
      reference: 'local:memory', task: 'local-memory', session: 'planner-memory',
      fetchImpl: async (_url, request) => {
        const body = JSON.parse(request.body);
        const data = JSON.parse(body.messages[1].content);
        assert.equal(data.ask, ask);
        assert.match(data.planner_memory, /planner-lesson-24/);
        assert.doesNotMatch(data.planner_memory, /planner-lesson-0"|coder-only-private/);
        assert.deepEqual(data.human_files_allowed, ['README.md']);
        assert.equal(body.tools, undefined);
        return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
          content: JSON.stringify({ outcomes: ['Update documentation'], issues: [
            { title: 'Add overview', outcome: 'Overview explains the project', acceptance_checks: ['Overview is present'],
              files_allowed: ['README.md'], wave: 1 },
            { title: 'Add setup', outcome: 'Setup explains local use', acceptance_checks: ['Setup is present'],
              files_allowed: ['README.md'], wave: 2 },
          ] }),
        } }] });
      } });
    assert.equal(plan.planningOnly, true);
    assert.equal(existsSync(join(worktree, 'TASK.md')), false);
    assert.equal(existsSync(join(worktree, 'RECIPE.yml')), false);
  });
}
