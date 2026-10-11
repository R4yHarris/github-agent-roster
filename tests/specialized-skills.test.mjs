import assert from 'node:assert/strict';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { assertSeatCovers, parseRecipe } from '../src/lib/recipe.mjs';
import { planStub, runtimeRecipe } from '../src/planner/stub.mjs';
import { loadContext } from '../src/runtime/context.mjs';
import { loadSkills } from '../src/runtime/skills.mjs';
import { runCoder } from '../src/seats/coder.mjs';

const sourceRoot = fileURLToPath(new URL('../', import.meta.url));
const config = parseConfig(readFileSync(path.join(sourceRoot, 'roster.config.example.yml'), 'utf8')
  .replace('base_url: ""', 'base_url: http://localhost:8000/v1').replace('model: ""', 'model: fixture-model'));
const categories = ['research', 'implement', 'test', 'review', 'incident', 'docs'];
const selectedTask = (names, metadata) => planStub('Add a one-line Status section to README.md.', { metadata }).task
  .replace(/^skills:.*$/m, 'skills: [' + names.join(', ') + ']');
function fixture(t, names, metadata = { task_class: 'docs', difficulty: 1 }) {
  const root = mkdtempSync(path.join(process.cwd(), '.specialized-skills-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const worktree = path.join(root, 'worktree');
  mkdirSync(worktree);
  cpSync(path.join(sourceRoot, 'principals'), path.join(root, 'principals'), { recursive: true });
  cpSync(path.join(sourceRoot, 'skills'), path.join(root, 'skills'), { recursive: true });
  writeFileSync(path.join(worktree, 'TASK.md'), selectedTask(names, metadata));
  writeFileSync(path.join(worktree, 'README.md'), '# Project\n');
  writeFileSync(path.join(worktree, 'AGENTS.md'), 'Repository scope and default deny remain enforced.');
  return { repoRoot: root, worktree, config, env: {}, task: 'skills-fixture', session: 'skills-fixture' };
}

for (const name of categories) {
  test('installed spec packet reaches minimum context: ' + name, async (t) => {
    const options = fixture(t, [name]);
    const context = await loadContext({ ...options, askKind: 'slice' });
    assert.ok(context.skillNames.includes(name));
    assert.ok(context.skills.some((skill) => skill.name === name));
    const [packet] = await loadSkills({ repoRoot: options.repoRoot, names: [name] });
    assert.match(packet.content, new RegExp('^name: ' + name + '$', 'm'));
    for (const section of ['When to use', 'Procedure', 'Stop condition']) assert.ok(packet.content.includes('## ' + section));
    assert.ok(context.pack.includes(packet.content.replaceAll('\r\n', '\n').trim()));
    const recipe = parseRecipe(runtimeRecipe('local:skills').replace('    sequence: [load_context, implement, run_tests, summarize]',
      '    sequence: [load_context, implement, run_tests, summarize]\n    skills: [' + name + ']'));
    assert.doesNotThrow(() => assertSeatCovers(recipe.seats[1], { skills: [name], difficulty: 1 }));
    assert.throws(() => assertSeatCovers({ ...recipe.seats[1], skills: [] }, { skills: [name] }), /does not cover/);
  });
}

test('selected categories survive full context budget, never silently omitted', async (t) => {
  const options = fixture(t, ['incident', 'research'], { task_class: 'feat', difficulty: 4 });
  const context = await loadContext({ ...options, askKind: 'feature', config: { ...config, seat: { ...config.seat, context_chars: 16000 } } });
  assert.equal(context.minimalDocs, false);
  assert.deepEqual(context.skills.map(({ name }) => name), ['incident', 'research']);
  for (const skill of context.skills) assert.ok(context.pack.includes(skill.content.trim()));
});

for (const malformed of ['missing', 'empty', 'oversized']) {
  test('selected malformed packet fails before inference or implementation: ' + malformed, async (t) => {
    const options = fixture(t, ['incident']);
    const file = path.join(options.repoRoot, 'skills', 'incident', 'SKILL.md');
    if (malformed === 'missing') rmSync(file);
    else writeFileSync(file, malformed === 'empty' ? '' : 'x'.repeat(65537));
    let inference = 0;
    await assert.rejects(runCoder({ ...options, askKind: 'slice', fetchImpl: async () => { inference++; throw new Error('must not infer'); } }), /Unknown task skill|must not be empty|64 KiB/);
    assert.equal(inference, 0);
    assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Project\n');
    assert.equal(existsSync(path.join(options.worktree, 'CONTEXT.md')), false);
  });
}

test('selected categories cannot be budget-truncated; unselected broken packets are ignored', async (t) => {
  const options = fixture(t, ['docs']);
  writeFileSync(path.join(options.repoRoot, 'skills', 'incident', 'SKILL.md'), '');
  await assert.rejects(loadContext({ ...options, config: { ...config, seat: { ...config.seat, context_chars: 1 } } }), /exceed.*context_chars/);
  assert.equal(existsSync(path.join(options.worktree, 'CONTEXT.md')), false);
  const context = await loadContext(options);
  assert.ok(context.skills.some(({ name }) => name === 'docs'));
  assert.ok(!context.skills.some(({ name }) => name === 'incident'));
});

test('research packet is instructions, not internet permission or reviewer authority', async (t) => {
  const options = fixture(t, ['research', 'review']);
  let requests = 0;
  await assert.rejects(runCoder({ ...options, askKind: 'slice', fetchImpl: async (_url, request) => {
    requests++;
    const body = JSON.parse(request.body);
    assert.ok(body.messages.some(({ content }) => content.includes('## research') && content.includes('## review')));
    return Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [
      { id: 'search', type: 'function', function: { name: 'web_search', arguments: '{"query":"fixture"}' } },
    ] } }] });
  } }), /invalid or unavailable tool|disabled|denied/);
  assert.equal(requests, 1);
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Project\n');
});

test('full context refuses insufficient skill budget instead of omitting selected procedure', async (t) => {
  const options = fixture(t, ['incident'], { task_class: 'feat', difficulty: 4 });
  writeFileSync(path.join(options.repoRoot, 'skills', 'incident', 'SKILL.md'),
    '# Incident\n' + Array.from({ length: 35 }, () => 'Keep the explicitly selected incident procedure. '.repeat(5)).join('\n'));
  await assert.rejects(loadContext({ ...options, askKind: 'feature', config: { ...config, seat: { ...config.seat, context_chars: 8000 } } }), /exceed.*context_chars/);
  assert.equal(existsSync(path.join(options.worktree, 'CONTEXT.md')), false);
});

test('selected packet junction paths fail before inference without reading target instructions', async (t) => {
  const options = fixture(t, ['incident']);
  const directory = path.join(options.repoRoot, 'skills', 'incident');
  const target = path.join(options.repoRoot, 'foreign-packet');
  rmSync(directory, { recursive: true });
  mkdirSync(target);
  writeFileSync(path.join(target, 'SKILL.md'), 'UNTRUSTED_TARGET_PACKET');
  symlinkSync(target, directory, process.platform === 'win32' ? 'junction' : 'dir');
  await assert.rejects(loadContext(options), /symlink/);
  assert.equal(existsSync(path.join(options.worktree, 'CONTEXT.md')), false);
});

test('selected docs procedure reaches actual coder request and existing final gate', async (t) => {
  const options = fixture(t, ['docs']);
  let requests = 0;
  const result = await runCoder({ ...options, askKind: 'slice', fetchImpl: async (_url, request) => {
    requests++;
    const body = JSON.parse(request.body);
    assert.ok(body.messages.some(({ content }) => content.includes('## docs') && content.includes('Write bounded repository documentation')));
    const message = requests === 1 ? { role: 'assistant', tool_calls: [{ id: 'write', type: 'function', function: {
      name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content: '# Project\n\n## Status\nReady.\n' }),
    } }] } : { role: 'assistant', content: 'Added the requested Status documentation.' };
    return Response.json({ choices: [{ finish_reason: message.tool_calls ? 'tool_calls' : 'stop', message }] });
  } });
  assert.equal(result.excellence.pass, true);
  assert.equal(requests, 2);
  assert.match(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), /## Status/);
});
