import assert from 'node:assert/strict';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { assertSeatCovers } from '../src/lib/recipe.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { parseTaskDocument } from '../src/planner/task.mjs';
import { loadContext } from '../src/runtime/context.mjs';
import { coderTaskSkillNames } from '../src/runtime/context-policy.mjs';
import { taskSkillNames } from '../src/runtime/skills.mjs';
import { runCoder } from '../src/seats/coder.mjs';

const sourceRoot = path.resolve(import.meta.dirname, '..');
const config = parseConfig(readFileSync(path.join(sourceRoot, 'roster.config.example.yml'), 'utf8')
  .replace('base_url: ""', 'base_url: http://localhost:8000/v1').replace('model: ""', 'model: fixture-model'));
const incidentAsk = 'Fix the regression in README.md: the Status heading disappeared after the last change. Keep deployment and network access disabled.';
function fixture(t, ask = incidentAsk, metadata = { task_class: 'docs', difficulty: 1 }) {
  const repoRoot = mkdtempSync(path.join(process.cwd(), '.incident-context-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = path.join(repoRoot, 'worktree');
  mkdirSync(worktree);
  for (const directory of ['principals', 'skills']) {
    cpSync(path.join(sourceRoot, directory), path.join(repoRoot, directory), { recursive: true });
  }
  const taskText = planStub(ask, { metadata }).task;
  writeFileSync(path.join(worktree, 'TASK.md'), taskText);
  writeFileSync(path.join(worktree, 'README.md'), '# Project\n');
  writeFileSync(path.join(worktree, 'AGENTS.md'), 'Default deny; no production, workflow or network authority.');
  return { repoRoot, worktree, config, env: {}, task: 'incident-context', session: 'incident-context', taskText };
}

for (const full of [false, true]) {
  test(`authorized incident automatically delivers guidance in ${full ? 'full' : 'minimum'} coder pack`, async (t) => {
    const options = fixture(t, incidentAsk, full ? { task_class: 'feat', difficulty: 4 } : undefined);
    const before = parseTaskDocument(options.taskText);
    assert.ok(!taskSkillNames(options.taskText).includes('incident'));
    const context = await loadContext({ ...options, askKind: full ? 'feature' : 'slice',
      config: { ...config, seat: { ...config.seat, context_chars: full ? 16000 : 8000 } } });
    assert.equal(context.minimalDocs, !full);
    assert.equal(context.skillNames.filter((name) => name === 'incident').length, 1);
    const packet = readFileSync(path.join(options.repoRoot, 'skills', 'incident', 'SKILL.md'), 'utf8').replaceAll('\r\n', '\n').trim();
    assert.ok(context.pack.includes(packet));
    assert.ok(context.pack.includes(before.ask));
    for (const check of before.acceptance_checks) assert.ok(context.pack.includes(check));
    assert.deepEqual(context.files, before.files_allowed);
    assert.equal(readFileSync(path.join(options.worktree, 'TASK.md'), 'utf8'), options.taskText);
    assert.equal(context.contextPolicy.sliceReadsOnly, !full);
  });
}

for (const ask of [
  'Add a Status heading to README.md.',
  'Add regression troubleshooting documentation to README.md.',
  'Why did the regression affect README.md?',
  'Production is down; README.md describes the symptoms.',
  'Add a README.md note about how to fix a regression.',
  'Do not fix the regression in README.md; document its symptoms.',
]) {
  test('ordinary or ambiguous ask does not infer incident authority: ' + ask, async (t) => {
    const options = fixture(t, ask);
    writeFileSync(path.join(options.repoRoot, 'skills', 'incident', 'SKILL.md'), '');
    const context = await loadContext({ ...options, askKind: 'slice' });
    assert.ok(!context.skillNames.includes('incident'));
    assert.ok(!context.pack.includes('Investigate a scoped regression or incident'));
  });
}

for (const ask of [
  'Triage the outage documented in README.md.',
  'Diagnose the regression in README.md.',
  'Investigate why production is down using README.md only.',
  'Reproduce the hotfix regression documented in README.md.',
]) {
  test('recognized investigation directive selects guidance, not new authority: ' + ask, (t) => {
    const options = fixture(t, ask);
    const before = parseTaskDocument(options.taskText);
    assert.ok(coderTaskSkillNames(options.taskText).includes('incident'));
    assert.deepEqual(parseTaskDocument(options.taskText), before);
  });
}

test('authorized incident retains explicit skills and honors bounded seat coverage', (t) => {
  const options = fixture(t);
  const names = coderTaskSkillNames(options.taskText);
  assert.deepEqual(names, [...taskSkillNames(options.taskText), 'incident']);
  assert.throws(() => assertSeatCovers({ id: 'coder', skills: taskSkillNames(options.taskText) }, { skills: names }), /does not cover.*incident/);
  assert.doesNotThrow(() => assertSeatCovers({ id: 'coder', skills: names }, { skills: names }));
  const selected = options.taskText.replace(/^skills:.*$/m, 'skills: [incident, docs]');
  assert.deepEqual(coderTaskSkillNames(selected), ['incident', 'docs']);
});

for (const full of [false, true]) {
  test(`automatic incident procedure cannot be budget omitted (${full ? 'full' : 'minimum'})`, async (t) => {
    const options = fixture(t, incidentAsk, full ? { task_class: 'feat', difficulty: 4 } : undefined);
    await assert.rejects(loadContext({ ...options, askKind: full ? 'feature' : 'slice',
      config: { ...config, seat: { ...config.seat, context_chars: 1 } } }), /exceed.*context_chars/);
    assert.equal(existsSync(path.join(options.worktree, 'CONTEXT.md')), false);
  });
}

test('automatic missing incident packet fails before model or product write', async (t) => {
  const options = fixture(t);
  rmSync(path.join(options.repoRoot, 'skills', 'incident', 'SKILL.md'));
  let requests = 0;
  await assert.rejects(runCoder({ ...options, askKind: 'slice', fetchImpl: async () => { requests++; } }), /Unknown task skill: incident/);
  assert.equal(requests, 0);
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Project\n');
});

for (const name of ['web_search', 'write_file']) {
  test('actual incident coder guidance does not override denied tool: ' + name, async (t) => {
    const options = fixture(t);
    let requests = 0;
    await assert.rejects(runCoder({ ...options, askKind: 'slice', fetchImpl: async (_url, request) => {
      requests++;
      const body = JSON.parse(request.body);
      assert.ok(body.messages.some(({ content }) => content.includes('## incident') &&
        content.includes('Urgency does not permit production access')));
      const args = name === 'web_search' ? { query: 'urgent outage' } :
        { path: '.github/workflows/recovery.yml', content: 'unauthorized' };
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [
        { id: 'denied', type: 'function', function: { name, arguments: JSON.stringify(args) } },
      ] } }] });
    } }), /invalid or unavailable tool|disabled|denied|protected|allowed/);
    assert.equal(requests, 1);
    assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Project\n');
    assert.equal(existsSync(path.join(options.worktree, '.github')), false);
  });
}

test('cancellation still stops an incident before preparation or inference', async (t) => {
  const options = fixture(t);
  const controller = new AbortController();
  controller.abort();
  let requests = 0;
  await assert.rejects(runCoder({ ...options, signal: controller.signal, fetchImpl: async () => { requests++; } }), /abort|cancel/i);
  assert.equal(requests, 0);
  assert.equal(existsSync(path.join(options.worktree, 'CONTEXT.md')), false);
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Project\n');
});

for (const incident of [false, true]) {
  test(`actual ${incident ? 'incident' : 'ordinary'} coder retains the same write and final gate`, async (t) => {
    const options = fixture(t, incident ? incidentAsk : 'Add a one-line Status section to README.md.');
    let requests = 0;
    const result = await runCoder({ ...options, askKind: 'slice', fetchImpl: async (_url, request) => {
      requests++;
      const body = JSON.parse(request.body);
      assert.equal(body.messages.some(({ content }) => content.includes('## incident')), incident);
      const message = requests === 1 ? { role: 'assistant', tool_calls: [{ id: 'write', type: 'function', function: {
        name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content: '# Project\n\n## Status\nReady.\n' }),
      } }] } : { role: 'assistant', content: 'Restored the requested Status section.' };
      return Response.json({ choices: [{ finish_reason: message.tool_calls ? 'tool_calls' : 'stop', message }] });
    } });
    assert.equal(result.excellence.pass, true, result.excellence.reasons.join('; '));
    assert.equal(requests, 2);
    assert.match(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), /## Status/);
    assert.equal(readFileSync(path.join(options.worktree, 'TASK.md'), 'utf8'), options.taskText);
  });
}

test('cancellation after incident guidance delivery prevents a returned write', async (t) => {
  const options = fixture(t);
  const controller = new AbortController();
  let requests = 0;
  await assert.rejects(runCoder({ ...options, askKind: 'slice', signal: controller.signal,
    fetchImpl: async (_url, request) => {
      requests++;
      assert.ok(JSON.parse(request.body).messages.some(({ content }) => content.includes('## incident')));
      controller.abort();
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [
        { id: 'write', type: 'function', function: { name: 'write_file',
          arguments: JSON.stringify({ path: 'README.md', content: 'must not write' }) } },
      ] } }] });
    } }), /abort|cancel/i);
  assert.equal(requests, 1);
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Project\n');
});
