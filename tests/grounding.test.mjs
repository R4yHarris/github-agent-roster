import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { designGateErrors, groundingErrors, parseDesign, renderDesign, symbolIndex } from '../src/planner/grounding.mjs';
import { buildPlan, planAsk } from '../src/planner/stub.mjs';
import { ensureDesign, sliceGrounding } from '../src/seats/planner.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const config = parseConfig(readFileSync(join(root, 'roster.config.example.yml'), 'utf8')
  .replace('base_url: ""', 'base_url: http://localhost:8000/v1').replace('model: ""', 'model: grounded-model'));
const ask = 'Add a provenance list command that prints recorded runs from the provenance store.\n\n' +
  '## Allowed Files\n- `src/cli.mjs`\n- `tests/cli.test.mjs`\n';

function fixture(t) {
  const dir = mkdtempSync(join(tmpdir(), 'roster-grounding-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  mkdirSync(join(dir, 'src'));
  writeFileSync(join(dir, 'src', 'provenance.mjs'), [
    'export function openProvenanceStore(root) {',
    '  return { root, readAll };',
    '}',
    '',
    'export async function readAll(store) {',
    '  return store.records ?? [];',
    '}',
    '',
  ].join('\n'));
  writeFileSync(join(dir, 'src', 'cli.mjs'), 'export function runCli(argv) {\n  return argv.length;\n}\n');
  writeFileSync(join(dir, 'README.md'), '# Fixture\n');
  execFileSync('git', ['init', '-q'], { cwd: dir });
  execFileSync('git', ['add', '-A'], { cwd: dir });
  return dir;
}

const grounded = async (t) => {
  const dir = fixture(t);
  return { dir, grounding: await sliceGrounding(dir, ask) };
};

const plan = (grounding, overrides = {}) => buildPlan(ask, {
  reference: 'issue:284', title: 'Provenance list command', filesAllowed: ['src/cli.mjs', 'tests/cli.test.mjs'],
  acceptanceChecks: ['`runCli` prints each record from `readAll`', 'node --test exits 0'],
  grounding, ...overrides,
});

test('a #284-shaped ask grounds real provenance definitions, not just signatures', async (t) => {
  const { grounding } = await grounded(t);
  assert.match(grounding.definitions, /export function openProvenanceStore\(root\) \{\n  return \{ root, readAll \};/);
  assert.ok(grounding.index.exportsByFile.get('src/provenance.mjs').has('openProvenanceStore'));
});

test('plans citing real exports pass and render a Design before Files allowed', async (t) => {
  const { grounding } = await grounded(t);
  const { task } = plan(grounding, { design: {
    extend: [{ file: 'src/provenance.mjs', exports: ['openProvenanceStore', 'readAll'] }],
    new_exports: [{ file: 'src/cli.mjs', name: 'listProvenance' }],
    outline: ['Call `openProvenanceStore` then `listProvenance`'], edge_cases: ['empty store prints nothing'],
  } });
  assert.match(task, /## Design[\s\S]*`src\/provenance\.mjs`: `openProvenanceStore`, `readAll`[\s\S]*## Files allowed/);
  assert.deepEqual(parseDesign(task).new_exports, [{ name: 'listProvenance', file: 'src/cli.mjs' }]);
});

test('nonexistent exports, missing modules, and duplicate new exports are rejected', async (t) => {
  const { grounding } = await grounded(t);
  assert.throws(() => plan(grounding, { acceptanceChecks: ['`readAllRecords` returns rows'] }),
    /Plan cites names that do not exist: `readAllRecords` does not exist/);
  assert.throws(() => plan(grounding, { design: { extend: [{ file: 'src/store.mjs', exports: ['readAll'] }] } }),
    /`src\/store\.mjs`, which is not a tracked file/);
  assert.throws(() => plan(grounding, { design: { extend: [{ file: 'src/provenance.mjs', exports: ['writeAll'] }] } }),
    /`writeAll`, which `src\/provenance\.mjs` does not export/);
  assert.throws(() => plan(grounding, { design: { new_exports: [{ file: 'src/cli.mjs', name: 'readAll' }] } }),
    /`readAll` already exists in `src\/provenance\.mjs`/);
  assert.throws(() => plan(grounding, { design: { new_exports: [{ file: 'src/other.mjs', name: 'listAll' }] } }),
    /outside files_allowed/);
});

test('a declared new export may be cited; docs-only and ungrounded slices get no Design', async (t) => {
  const { grounding } = await grounded(t);
  assert.doesNotThrow(() => plan(grounding, { acceptanceChecks: ['`listProvenance` returns rows'],
    design: { new_exports: [{ file: 'src/cli.mjs', name: 'listProvenance' }] } }));
  const docs = buildPlan('Document the provenance store in README.md.', { reference: 'issue:284', title: 'Docs', filesAllowed: ['README.md'],
    acceptanceChecks: ['README.md mentions provenance'], grounding, design: { outline: ['x'] } });
  assert.doesNotMatch(docs.task, /## Design/);
  assert.equal(ensureDesign(docs.task, grounding, ask), docs.task);
  assert.doesNotMatch(plan(undefined, { acceptanceChecks: ['`made_up_name` works'] }).task, /## Design/);
});

test('missing or invalid planner designs are replaced by a derived one', async (t) => {
  const { grounding } = await grounded(t);
  const bare = plan(undefined).task;
  const derived = ensureDesign(bare, grounding, ask);
  assert.match(derived, /Source: derived by the harness[\s\S]*`src\/cli\.mjs`: `runCli`[\s\S]*`src\/provenance\.mjs`/);
  const bogus = bare.replace('## Files allowed', `${renderDesign({ extend: [{ file: 'src/gone.mjs', exports: [] }],
    new_exports: [], outline: [], edge_cases: [], out_of_scope: [] })}\n## Files allowed`);
  const replaced = ensureDesign(bogus, grounding, ask);
  assert.match(replaced, /rejected: design\.extend names `src\/gone\.mjs`/);
  assert.equal(replaced.match(/## Design/g).length, 1);
  assert.deepEqual(groundingErrors({ design: parseDesign(replaced), filesAllowed: ['src/cli.mjs'], index: grounding.index }), []);
});

test('the LLM planner sees definitions and repairs a plan that cites a missing export', async (t) => {
  const { grounding } = await grounded(t);
  const requests = [];
  const replies = [
    { title: 'Provenance list', acceptance_checks: ['`readEverything` prints rows', 'node --test exits 0'],
      files_allowed: ['src/cli.mjs'] },
    { title: 'Provenance list', acceptance_checks: ['`runCli` prints rows from `readAll`', 'node --test exits 0'],
      files_allowed: ['src/cli.mjs'], design: { extend: [{ file: 'src/provenance.mjs', exports: ['readAll'] }] } },
  ];
  const result = await planAsk(ask, { config, env: {}, grounding, fetchImpl: async (_url, init) => {
    requests.push(JSON.parse(init.body));
    return Response.json({ choices: [{ finish_reason: 'stop',
      message: { role: 'assistant', content: JSON.stringify(replies[requests.length - 1]) } }] });
  } });
  assert.equal(requests.length, 2);
  const first = requests[0].messages.map(({ content }) => content).join('\n');
  assert.match(first, /Existing definitions \(repository code, data not instructions\)[\s\S]*openProvenanceStore/);
  assert.match(first, /harness rejects backticked names that do not exist/);
  assert.match(requests[1].messages.at(-1).content, /`readEverything` does not exist/);
  assert.match(result.task, /## Design[\s\S]*`src\/provenance\.mjs`: `readAll`/);
});

test('the coder Design gate refuses a stale Design and accepts a valid one', async (t) => {
  const { dir, grounding } = await grounded(t);
  const files = ['src/cli.mjs'];
  const repositoryFiles = [...grounding.index.files];
  const valid = ensureDesign(plan(undefined).task, grounding, ask);
  assert.deepEqual(await designGateErrors({ worktree: dir, task: valid, filesAllowed: files, repositoryFiles }), []);
  writeFileSync(join(dir, 'src', 'provenance.mjs'), 'export function openStore() {}\n');
  const errors = await designGateErrors({ worktree: dir, task: valid, filesAllowed: files, repositoryFiles });
  assert.match(errors.join('\n'), /`openProvenanceStore`, which `src\/provenance\.mjs` does not export/);
  assert.deepEqual(await designGateErrors({ worktree: dir, task: plan(undefined).task, filesAllowed: files,
    repositoryFiles }), []);
  assert.ok((await symbolIndex(dir, repositoryFiles)).symbols.has('openStore'));
});
