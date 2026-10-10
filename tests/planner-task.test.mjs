import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { buildPlan, cleanAskText, planAsk, planFromTask, planStub } from '../src/planner/stub.mjs';
import { deterministicPlanDefects } from '../src/planner/critic.mjs';
import { parseTaskDocument, taskFilesAllowed } from '../src/planner/task.mjs';
import { readTaskMetadata, updateTaskMetadata } from '../src/runtime/estimate.mjs';
import { runPlanner } from '../src/seats/planner.mjs';

const task = readFileSync(new URL('./fixtures/planner-task-92.md', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
const ask = 'Add a one-line Status section to README.md';
const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:8000/v1')
  .replace('model: ""', 'model: served-model'));

const directoryAsk = 'Write me a one page design of a simple marketing web page for a Security and Risk Analaysis consulting business and setup instructions for obtaining a web page and serving the web page to market the business. Output it in a new "new-design" directory';
const directoryOptions = {
  reference: 'local:directory',
  title: 'Design and setup documents',
  filesAllowed: ['new-design/DESIGN.md', 'new-design/SETUP.md'],
  acceptanceChecks: ['DESIGN describes a one-page consulting business web page',
    'SETUP explains how to obtain and serve the web page without deploying it'],
};

test('directory plans derive concrete paths, preserve the original ask and declare new docs for the critic', () => {
  const plan = buildPlan(directoryAsk, directoryOptions);
  const document = parseTaskDocument(plan.task, { expectedAsk: directoryAsk });
  assert.equal(document.ask, directoryAsk);
  assert.deepEqual(document.files_allowed, directoryOptions.filesAllowed);
  assert.deepEqual(deterministicPlanDefects(plan.task, {
    index: { files: new Set(), symbols: new Set(), exportsByFile: new Map() },
  }), []);
  assert.equal(planFromTask(plan.task, directoryAsk).title, directoryOptions.title);
  for (const filesAllowed of [['elsewhere/DESIGN.md'], ['new-designish/DESIGN.md'], ['new-design/**'],
    ['new-design'], ['new-design/../escape.md'], ['new-design/.git/config'], ['new-design/vendor/info.md'],
    ['new-design/.github/workflows/ci.yml'], ['new-design/agent-policy.yml'], ['new-design/private.pem'],
    ['new-design/.env'], ['new-design/name.']]) {
    assert.throws(() => buildPlan(directoryAsk, { ...directoryOptions, filesAllowed }),
      /concrete|protected/, filesAllowed.join(','));
  }
  assert.throws(() => buildPlan(directoryAsk, { ...directoryOptions, acceptanceChecks: ['node --test exits 0'] }),
    /verifiable requested outcome/);
  assert.throws(() => buildPlan(directoryAsk, { ...directoryOptions, acceptanceChecks: [] }), /Acceptance checks/);
  assert.throws(() => buildPlan(`${directoryAsk}\n\n## Files allowed\n- new-design/DESIGN.md`, directoryOptions),
    /cannot invent extra files/);
});

test('directory TASK validation rejects a changed Ask or hand-edited outside scope, including multiline text', () => {
  const ask = directoryAsk.replace('. Output', '.\nOutput');
  const plan = buildPlan(ask.replace(/\n/g, '\r\n'), directoryOptions);
  assert.equal(parseTaskDocument(plan.task, { expectedAsk: ask }).ask, ask);
  assert.throws(() => parseTaskDocument(plan.task.replace('Analaysis', 'Analysis'), { expectedAsk: ask }),
    /unchanged original Ask/);
  assert.throws(() => parseTaskDocument(plan.task.replaceAll('new-design/DESIGN.md', 'outside/DESIGN.md'),
    { expectedAsk: ask }), /granted output directory/);
});

test('directory documentation rejects executable artifacts and action-success checks, not unrelated implementation intent', () => {
  for (const filesAllowed of [['new-design/deploy.sh'], ['new-design/index.html'], ['new-design/setup.mjs']]) {
    assert.throws(() => buildPlan(directoryAsk, { ...directoryOptions, filesAllowed }), /Markdown documents/);
  }
  for (const check of ['npm run deploy succeeds', 'Run pip install and deploy the page',
    'The account is created', 'Purchase hosting successfully', 'Publish the page',
    'curl https://example.invalid succeeds', 'SETUP documents instructions and npm run deploy succeeds',
    'SETUP describes instructions; register an account',
    'SETUP describes deployment and the page is deployed',
    'SETUP describes domain purchase and the account is activated']) {
    assert.throws(() => buildPlan(directoryAsk, { ...directoryOptions, acceptanceChecks: [check] }),
      /document-content checks/, check);
  }
  const safe = buildPlan(directoryAsk, { ...directoryOptions, acceptanceChecks: [
    'SETUP documents installation, deployment and account setup instructions without executing them',
    'DESIGN describes the page and SETUP contains human approval warnings',
  ] });
  assert.deepEqual(parseTaskDocument(safe.task, { expectedAsk: directoryAsk }).files_allowed, directoryOptions.filesAllowed);
  const changed = safe.task.replace('DESIGN describes the page and SETUP contains human approval warnings',
    'SETUP documents instructions and npm run deploy succeeds');
  assert.throws(() => parseTaskDocument(changed, { expectedAsk: directoryAsk }), /document-content checks/);
  assert.throws(() => parseTaskDocument(safe.task.replaceAll('new-design/DESIGN.md', 'new-design/deploy.sh'),
    { expectedAsk: directoryAsk }), /Markdown documents/);
  const implementation = 'Write a script with setup instructions. Output it in a new "scripts" directory';
  const code = buildPlan(implementation, { ...directoryOptions, filesAllowed: ['scripts/task.mjs'],
    acceptanceChecks: ['node --test exits 0', 'The script implements the requested behavior'] });
  assert.deepEqual(parseTaskDocument(code.task, { expectedAsk: implementation }).files_allowed, ['scripts/task.mjs']);
  for (const ask of ['Create a design and setup guide. Output it in a new "new-design" directory',
    'Document how to obtain and serve a web page. Output it in a new "new-design" directory',
    'Draft a migration report. Output it in a new "new-design" directory',
    'Generate documentation for the site. Output it in a new `new-design` directory',
    'Compose setup instructions for the site. Output it in a new "new-design" directory',
    'A one-page design and hosting guide, please. Output it in a new "new-design" directory',
    'Give me a deployment manual. Output it in a new "new-design" directory',
    'Summarize the account setup instructions. Output it in a new "new-design" directory',
    'Generate documentation explaining how to build a website. Output it in a new "new-design" directory',
    'Generate docs for the site. Output it in a new "new-design" directory',
    'Prepare setup for the site. Output it in a new "new-design" directory',
    'Create a README for the site. Output it in a new "new-design" directory']) {
    assert.throws(() => buildPlan(ask, { ...directoryOptions, filesAllowed: ['new-design/deploy.sh'] }),
      /Markdown documents/);
  }
  for (const ask of ['Generate a script with documentation. Output it in a new "scripts" directory',
    'Implement a program and provide setup instructions. Output it in a new "scripts" directory',
    'Build a static website with a design guide. Output it in a new "scripts" directory']) {
    assert.deepEqual(parseTaskDocument(buildPlan(ask, { ...directoryOptions, filesAllowed: ['scripts/main.mjs'],
      acceptanceChecks: ['node --test exits 0', 'The implementation satisfies the requested behavior'] }).task,
    { expectedAsk: ask }).files_allowed, ['scripts/main.mjs']);
  }
});

test('directory handoffs preserve captured whitespace and CRLF except normalized line endings', () => {
  const raw = ` \t\r\n${directoryAsk.replace('. Output', '.\r\nOutput')}\r\n \t\r\n`;
  const original = raw.replace(/\r\n/g, '\n');
  assert.equal(cleanAskText(raw), original);
  assert.equal(cleanAskText(' \r\nUpdate README.md. \r\n'), 'Update README.md.');
  const plan = buildPlan(raw, directoryOptions);
  assert.equal(parseTaskDocument(plan.task, { expectedAsk: raw }).ask, original);
  assert.equal(parseTaskDocument(plan.task).ask, original);
  const estimated = updateTaskMetadata(plan.task, { estimate_min: 9 });
  assert.equal(parseTaskDocument(estimated, { expectedAsk: raw }).ask, original);
  for (const task of [plan.task.replace(original, original.trim()),
    plan.task.replace('Analaysis', 'Analysis'), plan.task.replace('Security', 'security'),
    plan.task.replace('one page', 'one  page')]) {
    assert.throws(() => parseTaskDocument(task, { expectedAsk: raw }), /unchanged original Ask/);
  }
});

test('critic revisions cannot exchange concrete directory files', async () => {
  const task = buildPlan(directoryAsk, directoryOptions).task;
  let calls = 0;
  await assert.rejects(planAsk(directoryAsk, { config, env: {}, criticFeedback: { task, defects: [] },
    fetchImpl: async () => {
      calls += 1;
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant',
        content: JSON.stringify({ title: 'Design documents', files_allowed: ['new-design/EXTRA.md'],
          acceptance_checks: directoryOptions.acceptanceChecks }),
      } }] });
    } }), /cannot invent extra files/);
  assert.ok(calls > 0);
});

test('critic can repair directory new-file declarations without changing the original Ask or concrete files', async () => {
  const task = buildPlan(directoryAsk, directoryOptions).task.replace(/## New files\n[\s\S]*?(?=## Ask)/, '');
  assert.equal(deterministicPlanDefects(task, {
    index: { files: new Set(), symbols: new Set(), exportsByFile: new Map() },
  }).length, 2);
  const plan = await planAsk(directoryAsk, { config, env: {}, criticFeedback: { task, defects: [] },
    fetchImpl: async () => Response.json({ choices: [{ finish_reason: 'stop', message: {
      role: 'assistant', content: JSON.stringify({ title: directoryOptions.title,
        files_allowed: directoryOptions.filesAllowed, acceptance_checks: directoryOptions.acceptanceChecks }),
    } }] }) });
  const revised = parseTaskDocument(plan.task, { expectedAsk: directoryAsk });
  assert.equal(revised.ask, directoryAsk);
  assert.deepEqual(revised.files_allowed, directoryOptions.filesAllowed);
  assert.deepEqual(deterministicPlanDefects(plan.task, {
    index: { files: new Set(), symbols: new Set(), exportsByFile: new Map() },
  }), []);
});

test('the actual issue-92 TASK fixture validates with Original Ask, Scope, Acceptance Checks and Allowed Files', () => {
  const parsed = parseTaskDocument(task, { expectedAsk: ask });
  assert.equal(parsed.title, ask);
  assert.equal(parsed.ask, ask);
  assert.equal(parsed.acceptance_checks.length, 5);
  assert.deepEqual(parsed.files_allowed, ['README.md']);
  const plan = planFromTask(task, ask);
  assert.equal(plan.difficulty, 1);
  assert.equal(plan.estimate_min, 8);
  assert.equal(plan.task_class, 'docs');
});

test('a missing Allowed Files section fails instead of using paths found in Scope or the Ask', () => {
  const missing = task.replace(/## Allowed Files\n[\s\S]*?(?=## Metadata)/, '');
  assert.throws(() => parseTaskDocument(missing, { expectedAsk: ask }), /Allowed Files/);
  assert.throws(() => taskFilesAllowed(missing), /Allowed Files/);
});

test('heading aliases and case are accepted without requiring Task: on the title', () => {
  for (const acceptance of ['aCcEpTaNcE cHeCkS', 'acceptance_checks']) {
    const aliased = task.replace('# Task: ', '# ').replace('## Original Ask', '## aSK')
      .replace('## Acceptance Checks', `## ${acceptance}`).replace('## Allowed Files', '## fIlEs AlLoWeD')
      .replace(/\n/g, '\r\n');
    assert.deepEqual(parseTaskDocument(aliased, { expectedAsk: ask }).files_allowed, ['README.md']);
    assert.equal(readTaskMetadata(aliased).estimate_min, 8);
  }
});

test('Original Ask can follow the checks/files while metadata after it remains task metadata', () => {
  const source = `# ${ask}\n\n## acceptance_checks\n- node --test exits 0\n\n## Allowed Files\n` +
    `- README.md\n\n## Original Ask\n${ask}\n\n## Metadata\n- task_class: docs\n- difficulty: 1\n- estimate_min: 8\n`;
  assert.deepEqual(parseTaskDocument(source, { expectedAsk: ask }).files_allowed, ['README.md']);
  assert.equal(readTaskMetadata(source).task_class, 'docs');
  assert.equal(readTaskMetadata(source).estimate_min, 8);
});

test('the Ask may include surrounding text but must contain the issue text', () => {
  const containing = task.replace(`## Original Ask\n${ask}`, `## Original Ask\nOperator request:\n${ask}\nPreserve the scope.`);
  assert.equal(parseTaskDocument(containing, { expectedAsk: ask }).title, ask);
  assert.throws(() => parseTaskDocument(task, { expectedAsk: 'Edit a different task.' }), /unchanged Ask/);
});

test('canonical Ask sections still retain embedded issue headings and metadata-looking text as Ask data', () => {
  const original = 'Update README.md.\nmodel: ignored-ask-value\n\n## Acceptance checks\n' +
    '- README.md documents the requested update\n\n' +
    '## Files allowed\n- `README.md`';
  const canonical = planStub(original, { reference: 'issue:92' }).task;
  assert.equal(parseTaskDocument(canonical, { expectedAsk: original }).ask, original);
  assert.equal(readTaskMetadata(canonical).model, '');
});

test('metadata updates keep Scope and Original Ask without leaving duplicate metadata declarations', () => {
  const updated = updateTaskMetadata(task, { estimate_min: 12, model: 'served-model' });
  assert.equal(readTaskMetadata(updated).estimate_min, 12);
  assert.equal(readTaskMetadata(updated).model, 'served-model');
  assert.equal(readTaskMetadata(updated).difficulty, 1);
  assert.match(updated, /## Scope[\s\S]*No other files, sections, or wording changes are in scope/);
  assert.equal(parseTaskDocument(updated, { expectedAsk: ask }).ask, ask);
});

test('ambiguous duplicate headings and protected Allowed Files are rejected', () => {
  assert.throws(() => parseTaskDocument(task.replace('## Metadata', '## allowed_files\n- `src/**`\n\n## Metadata')),
    /duplicate files allowed/);
  assert.throws(() => parseTaskDocument(task.replace('## Allowed Files\n- `README.md`',
    '## Allowed Files\n- `.github/workflows/ci.yml`')), /protected files/);
});

test('writing a complete fixture TASK finishes in the first model response even with a two-turn budget', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-complete-task-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  let calls = 0;
  const result = await runPlanner({
    worktree, repoRoot, config, env: {}, issue: { number: 92, title: ask, body: ask },
    fetchImpl: async () => {
      calls += 1;
      assert.equal(calls, 1, 'A valid written task must not ask the model for confirmation');
      return Response.json({
        model: 'actual-planner-model', usage: { prompt_tokens: 100, completion_tokens: 40 },
        choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [{
          id: 'task', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content: task }),
          },
        }] } }],
      });
    },
  });
  assert.equal(calls, 1);
  assert.equal(result.turns, 1);
  assert.equal(result.run.metrics.prompt_tokens, 100);
  assert.equal(result.run.metrics.completion_tokens, 40);
  assert.equal(result.metadata.estimate_min, 8);
  assert.match(readFileSync(result.taskPath, 'utf8'), /## Original Ask[\s\S]*## Scope[\s\S]*## Allowed Files/);
  assert.equal(parseTaskDocument(result.task, { expectedAsk: ask }).ask, ask);
});

test('a template-copied Task title heading takes the real title from the next line', () => {
  const body = '\n\n## Original Ask\nStore records.\n\n## Acceptance Checks\n- Works.\n\n## Allowed Files\n- `src/a.mjs`\n';
  assert.equal(parseTaskDocument(`# Task title\nImplement atomic persistence\n\ndifficulty: 2${body}`).title,
    'Implement atomic persistence');
  assert.equal(parseTaskDocument(`# Task title\n\ndifficulty: 2${body}`).title, 'Task title');
  assert.equal(parseTaskDocument(`# Ship the store\nDetails here.${body}`).title, 'Ship the store');
});
