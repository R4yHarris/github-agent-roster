import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { classifyAsk } from '../src/planner/classify.mjs';
import { askRequirements } from '../src/planner/stub.mjs';
import { planOutline, validatePlan } from '../src/planner/plan.mjs';
import { parsePlanDocument } from '../src/planner/plan-document.mjs';
import { createTools, isForbiddenWrite, isManagedFile } from '../src/runtime/tools.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const stub = parseConfig(example);
const configured = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:8000/v1')
  .replace('model: ""', 'model: plan-model'));

test('ordinary output-directory asks are executable without classifier-invented filenames', () => {
  const ask = 'Write me a one page design of a simple marketing web page for a Security and Risk Analaysis consulting business and setup instructions for obtaining a web page and serving the web page to market the business. Output it in a new "new-design" directory';
  assert.equal(classifyAsk(ask).kind, 'slice');
  assert.deepEqual(askRequirements(ask), { files: [], outcomes: null, explicit: false,
    outputDirectory: 'new-design', requiresRun: true });
  assert.equal(classifyAsk('Write a design. Output it in a new "notes/design" directory').kind, 'slice');
  assert.deepEqual(askRequirements('Write a design. Output it in a new "notes.md" directory').files, []);
  const explicit = `Implement directory intake for this example:\n${ask}\n\n## Files allowed\n- src/planner/classify.mjs`;
  assert.deepEqual(askRequirements(explicit).files, ['src/planner/classify.mjs']);
  assert.equal(askRequirements(explicit).outputDirectory, undefined);
  for (const text of ['Explain the new-design directory.', 'Write a design mentioning the new-design directory.',
    'The example says Output it in a new "new-design" directory', 'Write a design for this business.',
    'Explain this example:\n```\nOutput it in a new "new-design" directory\n```',
    'Explain this quoted example:\n> Output it in a new "new-design" directory',
    'Explain this phrase: \'Write docs. Output it in a new "new-design" directory\'',
    'Explain this hidden example. <!-- Output it in a new "new-design" directory -->']) {
    assert.equal(classifyAsk(text).kind, 'clarify', text);
  }
  assert.equal(classifyAsk('Build a platform. Output it in a new "new-design" directory').kind, 'initiative');
  assert.equal(classifyAsk('Add an end-to-end feature. Output it in a new "new-design" directory').kind, 'feature');
});

test('directory authority rejects unsafe destinations instead of silently widening intake', () => {
  for (const directory of ['/', '/outside', 'C:/outside', 'C:\\outside', '\\\\host\\share', '..', '.', './notes',
    'notes/../escape', 'notes.', 'notes//docs', 'notes/**', '.git', 'notes/.git', 'vendor',
    'notes/vendor', '.github/workflows', 'notes/.github/workflows', 'agent-policy.yml', 'keys.pem',
    '.env', 'private.env', '.roster', 'notes/.env', 'notes\\escape']) {
    assert.throws(() => classifyAsk(`Write a design. Output it in a new "${directory}" directory`),
      /Output directory/, directory);
  }
  assert.throws(() => askRequirements('Write a design. Output it in a "notes" directory. Output it in an "other" directory'),
    /one output directory/);
});

function outline(count = 2) {
  return { outcomes: ['An agreed feature works'], issues: Array.from({ length: count }, (_, index) => ({
    title: `Bounded slice ${index + 1}`, outcome: `Deliver outcome ${index + 1}`, wave: index + 1,
    acceptance_checks: ['Acceptance evidence passes'], files_allowed: ['README.md'],
  })) };
}

test('a feature-titled ask with an explicit allow-list, one outcome, and acceptance checks is a slice', () => {
  const body = '## Outcome\nResolve the state root.\n\n## Acceptance criteria\n- node --test exits 0\n\n' +
    '## Allowed files\n- `src/lib/paths.mjs`\n- `tests/paths.test.mjs`';
  assert.equal(classifyAsk(body, { title: 'feature: Implement a state-root resolver' }).kind, 'slice');
  assert.equal(classifyAsk('Implement a profile feature.\n\n## Allowed files\n- `README.md`',
    { title: 'Implement a profile feature' }).kind, 'feature');
});

test('README one-liners classify slice independently of task class, case and templates', () => {
  for (const ask of ['README one-liner', 'Add a one-line Status to README.md.',
    'feat: Add one-line Status to `README.md`.',
    '# Ask\n\nDOCS: Add a one-line Status to README.md.\n\n## Acceptance checks\n- node --test exits 0']) {
    assert.equal(classifyAsk(ask).kind, 'slice', ask);
  }
  assert.equal(classifyAsk('Different body lead.', { title: 'Add a one-line Status to README' }).kind, 'slice');
  assert.equal(classifyAsk('Update the intro.', { filesAllowed: ['README.md'] }).kind, 'slice');
  assert.equal(classifyAsk('Document how to build an orchestrator in README.md.').kind, 'slice');
  assert.equal(classifyAsk('Document feature and initiative classification in README.md.').kind, 'slice');
  assert.equal(classifyAsk('Add a one-line feature status to README.md.').kind, 'slice');
  assert.equal(classifyAsk('Update README.md.\n\n## Acceptance checks\n- Describe an initiative and a feature').kind, 'slice');
});

test('whole systems classify initiative; features and multiple outcomes are planning-only kinds', () => {
  for (const ask of ['build an orchestrator', 'Build an orchestrator in README.md.',
    'Create a standalone platform.', 'Deliver a multi-wave initiative.',
    '## Epic outcome\n\nSeparate machine history from repository state.']) {
    assert.equal(classifyAsk(ask).kind, 'initiative', ask);
  }
  assert.deepEqual(classifyAsk('## Epic outcome\n\nSeparate machine history from repository state.', {
    title: 'feature: separate machine-local history from repository-local state',
  }), {
    kind: 'initiative',
    specKind: 'epic',
    reason: 'Explicit epic or initiative outcome needs an issue plan',
  });
  assert.equal(classifyAsk('Deliver the bounded change.', { title: 'epic: state ownership' }).kind, 'initiative');
  for (const ask of ['Implement the profile feature.', 'Add an end-to-end workflow.',
    'Update README.md.\n\n## Outcomes\n- Add Status\n- Rewrite intro\n\n## Allowed Files\n- `README.md`']) {
    assert.equal(classifyAsk(ask).kind, 'feature', ask);
  }
  assert.equal(classifyAsk('Improve things.').kind, 'clarify');
  assert.equal(classifyAsk('Fix the bug.').kind, 'clarify');
  assert.equal(classifyAsk('Plan outcomes.\n\n## Outcomes\n' +
    Array.from({ length: 6 }, (_, index) => `- Outcome ${index + 1}`).join('\n')).kind, 'initiative');
  assert.throws(() => classifyAsk(''), /Ask must be nonempty/);
});

test('spec ask taxonomy is additive and never changes legacy planning kinds', () => {
  for (const [ask, kind, specKind] of [
    ['Update README.md.', 'slice', 'slice'],
    ['Implement the profile feature.', 'feature', 'story'],
    ['Build a standalone platform.', 'initiative', 'epic'],
    ['Why is README.md structured this way?', 'slice', 'question'],
    ['How does the planner work?', 'clarify', 'question'],
    ['Fix the bug.', 'clarify', null],
    ['Can you update README.md?', 'slice', 'slice'],
    ['Add a feature to explain how things work.', 'feature', 'story'],
  ]) {
    const classification = classifyAsk(ask);
    assert.equal(classification.kind, kind, ask);
    assert.equal(classification.specKind, specKind, ask);
    assert.equal(typeof classification.reason, 'string');
  }
  for (const signal of ['OUTAGE', 'prod down', 'production is down', 'regression', 'hotfix']) {
    assert.equal(classifyAsk(`Fix ${signal} in README.md.`).kind, 'slice');
    assert.equal(classifyAsk(`Fix ${signal} in README.md.`).specKind, 'incident');
    assert.equal(classifyAsk(signal).kind, 'clarify');
    assert.equal(classifyAsk(signal).specKind, 'incident');
  }
  assert.equal(classifyAsk('What caused the outage?').specKind, 'incident');
  assert.equal(classifyAsk('Different body.', { title: 'Why is README.md structured this way?' }).specKind, 'question');
  assert.equal(classifyAsk('Update README.md.\n\n## Acceptance checks\n- Explain the outage').specKind, 'slice');
});

test('initiative stub retains all eight declared outcomes without truncating child drafts or invalid wave labels', async () => {
  const outcomes = Array.from({ length: 8 }, (_, index) => `Outcome ${index + 1}`);
  const ask = 'build an orchestrator\n\n## Outcomes\n' + outcomes.map((outcome) => `- ${outcome}`).join('\n');
  const plan = await planOutline(ask, { kind: 'initiative', config: stub, env: {} });
  assert.deepEqual(plan.outline.outcomes, outcomes);
  assert.equal(plan.outline.issues.length, 9);
  for (const outcome of outcomes) assert.ok(plan.outline.issues.some((issue) => issue.outcome === outcome));
  assert.equal(plan.outline.issues.at(-1).wave, 8);
});

test('configured planning refuses tool calls rather than executing app writes or emitting response bodies', async () => {
  let calls = 0;
  await assert.rejects(planOutline('build an orchestrator', {
    kind: 'initiative', config: configured, env: {}, fetchImpl: async () => {
      calls += 1;
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant', content: 'PRIVATE_PLAN_RESPONSE_BODY', tool_calls: [{
          id: 'edit', type: 'function', function: { name: 'write_file',
            arguments: JSON.stringify({ path: 'README.md', content: 'PRIVATE_CODE_BODY' }) },
        }],
      } }] });
    },
  }), (error) => /Planning-only initiative failed/.test(error.message) && !/PRIVATE_/.test(error.message));
  assert.equal(calls, 4);
  await assert.rejects(planOutline('build an orchestrator', { kind: 'slice', config: configured,
    fetchImpl: () => assert.fail('Invalid PLAN kind must fail before inference') }), /feature or initiative/);
});

test('feature PLAN validates the exact 2-5 draft threshold, known file scope, and contiguous issue labels', () => {
  const options = { kind: 'feature', filesAllowed: ['README.md'] };
  for (const count of [2, 5]) assert.equal(validatePlan(outline(count), options).issues.length, count);
  for (const count of [1, 6]) assert.throws(() => validatePlan(outline(count), options), /2-5 child issue drafts/);
  const extraFiles = outline();
  extraFiles.issues[0].files_allowed.push('src/extra.mjs');
  assert.throws(() => validatePlan(extraFiles, options), /cannot invent allowed files/);
  assert.throws(() => validatePlan(outline(), { kind: 'feature' }), /cannot invent allowed files/);
  const gap = outline();
  gap.issues[1].wave = 3;
  assert.throws(() => validatePlan(gap, options), /no gaps/);
  const duplicate = outline();
  duplicate.issues[1].title = duplicate.issues[0].title.toUpperCase();
  assert.throws(() => validatePlan(duplicate, options), /distinct/);
  const grant = outline();
  grant.issues[0].tools = ['write_file'];
  assert.throws(() => validatePlan(grant, options), /PLAN fields/);
});

test('empty endpoint writes deterministic feature/initiative plans without inventing file scope or inference', async () => {
  for (const kind of ['feature', 'initiative']) {
    const ask = kind === 'feature' ? 'Implement the profile feature.' : 'build an orchestrator';
    const planned = await planOutline(ask, { kind, config: stub, env: {},
      fetchImpl: () => assert.fail('Stub PLAN must not call a model') });
    assert.equal(planned.mode, 'stub');
    assert.equal(planned.turns, 0);
    assert.equal(planned.planningOnly, true);
    assert.equal(planned.askKind, kind);
    assert.equal(planned.task, undefined);
    assert.equal(planned.recipe, undefined);
    assert.ok(planned.outline.issues.length >= 2 && planned.outline.issues.length <= 5);
    assert.ok(planned.outline.issues.every(({ files_allowed }) => files_allowed.length === 0));
    for (const heading of ['Outcomes', 'Waves', 'Child issue drafts', 'Original Ask']) {
      assert.ok(planned.plan.includes(`## ${heading}\n`));
    }
    assert.match(planned.plan, /Labels: `wave:1`[\s\S]*Labels: `wave:2`/);
    assert.ok(planned.plan.includes(ask));
    assert.match(planned.plan, /Human must name allowed files/);
    assert.doesNotMatch(planned.plan, /README\.md|\*\*\/\*/);
  }
});

test('configured planning uses one validated JSON response and never offers app or file tools', async () => {
  let calls = 0;
  const ask = 'Implement the profile feature.\n\n## Allowed files\n- `README.md`';
  const plan = await planOutline(ask, { kind: 'feature', config: configured, env: {},
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.equal(body.tools, undefined);
      assert.match(body.messages[0].content, /feature planner seat[\s\S]*PLAN\.md only/);
      assert.deepEqual(JSON.parse(body.messages[1].content).human_files_allowed, ['README.md']);
      return Response.json({ model: 'served-plan-model', usage: { prompt_tokens: 100, completion_tokens: 40 },
        choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: JSON.stringify(outline(5)) } }] });
    } });
  assert.equal(calls, 1);
  assert.equal(plan.outline.issues.length, 5);
  assert.equal(plan.turns, 1);
  assert.equal(plan.response.model, 'served-plan-model');
  assert.match(plan.plan, /Labels: `wave:5`/);
});

test('malformed or over-scoped PLAN gets bounded repairs that show the rejected answer, then an explicit failure', async () => {
  let calls = 0;
  await assert.rejects(planOutline('Implement the profile feature.', {
    kind: 'feature', config: configured, env: {}, fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.equal(body.tools, undefined);
      if (calls === 2) {
        assert.match(body.messages.at(-1).content, /Emit only the requested PLAN JSON/);
        assert.deepEqual(body.messages.at(-2), { role: 'assistant', content: 'garbage' });
      }
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: calls === 1 ? 'garbage' : JSON.stringify(outline()),
      } }] });
    },
  }), /Planning-only feature failed after 4 attempts: PLAN cannot invent allowed files[\s\S]*No coder or publisher ran/);
  assert.equal(calls, 4);
});

test('with no human file scope the feature planner proposes grounded files for every draft', async () => {
  const repositoryFiles = ['src/lib/paths.mjs', 'tests/paths.test.mjs', 'docs/STATE.md'];
  const options = { kind: 'feature', repositoryFiles };
  const scoped = (files) => ({ outcomes: ['State moves'], issues: [
    { title: 'Move state', outcome: 'Moved', acceptance_checks: ['node --test exits 0'], wave: 1, files_allowed: files },
    { title: 'Document state', outcome: 'Documented', acceptance_checks: ['docs explain it'], wave: 2,
      files_allowed: ['docs/STATE.md'] },
  ] });
  const valid = validatePlan(scoped(['src/lib/paths.mjs', 'src/lib/repo-state.mjs', 'tests/**']), options);
  assert.equal(valid.proposedScope, true);
  assert.throws(() => validatePlan(scoped(['lib/unknown.mjs']), options), /lib\/unknown\.mjs/);
  assert.throws(() => validatePlan(scoped([]), options));
  assert.throws(() => validatePlan(scoped(['**/*']), options));
  assert.throws(() => validatePlan(scoped(['.github/workflows/ci.yml']), options));
  assert.throws(() => validatePlan(scoped(['src/lib/paths.mjs']), { kind: 'feature' }), /cannot invent allowed files/);
  let calls = 0;
  const plan = await planOutline('Split repo state from machine memory.', { kind: 'feature', config: configured, env: {},
    repositoryFiles, fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.deepEqual(JSON.parse(body.messages[1].content).repository_files, repositoryFiles);
      const content = calls === 1 ? JSON.stringify(scoped(['nowhere/file.mjs']))
        : '```json\n' + JSON.stringify(scoped(['src/lib/paths.mjs'])) + '\n```';
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content } }] });
    } });
  assert.equal(calls, 2);
  assert.match(plan.plan, /^Scope: planner-proposed$/m);
  assert.doesNotMatch(plan.plan, /Human must name allowed files/);
  assert.deepEqual(parsePlanDocument(plan.plan).issues.map(({ files_allowed }) => files_allowed),
    [['src/lib/paths.mjs'], ['docs/STATE.md']]);
  assert.throws(() => parsePlanDocument(plan.plan.replace(/^Scope: planner-proposed\n+/m, '')), /cannot invent allowed files/);
});

test('planning-only writer can write PLAN.md, never TASK/recipe/app code, and coder cannot write PLAN', async (t) => {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-plan-writer-'));
  t.after(() => rmSync(worktree, { recursive: true, force: true }));
  const tools = await createTools({ worktree, seat: 'planner', plannerArtifacts: ['PLAN.md'] });
  for (const name of ['TASK.md', 'RECIPE.yml', 'ESTIMATE.md', 'README.md', 'src/code.mjs', '../PLAN.md', '.\\PLAN.md']) {
    await assert.rejects(tools.write_file({ path: name, content: 'denied' }), /Planner write_file|inside|outside the worktree/);
  }
  assert.equal(existsSync(path.join(worktree, 'src')), false);
  await tools.write_file({ path: 'PLAN.md', content: '# Plan\n' });
  assert.equal(readFileSync(path.join(worktree, 'PLAN.md'), 'utf8'), '# Plan\n');
  const fresh = await createTools({ worktree, seat: 'planner', plannerArtifacts: ['PLAN.md'] });
  await assert.rejects(fresh.write_file({ path: 'PLAN.md', content: 'replaced' }), /pre-existing/);
  const slice = await createTools({ worktree, seat: 'planner' });
  await assert.rejects(slice.write_file({ path: 'PLAN.md', content: 'denied' }), /Planner write_file/);
  const coder = await createTools({ worktree, allowedFiles: ['**/*'] });
  await assert.rejects(coder.write_file({ path: 'PLAN.md', content: 'denied' }), /not allowed/);
  assert.equal(isManagedFile('PLAN.md'), true);
  assert.equal(isForbiddenWrite('PLAN.md'), true);
  await assert.rejects(createTools({ worktree, seat: 'planner', plannerArtifacts: ['README.md'] }), /known root/);
});
