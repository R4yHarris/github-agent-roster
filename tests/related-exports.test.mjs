import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { relatedExports } from '../src/planner/related-exports.mjs';
import { planOutline } from '../src/planner/plan.mjs';

const sources = {
  'src/lib/paths.mjs': 'export function resolveMachineRoot() {}\nexport const unrelatedThing = 1;\n',
  'src/lib/redaction.mjs': 'export function redactEvidence() {}\n',
  'src/lib/colors.mjs': 'export function paintButton() {}\n',
  'docs/STATE.md': 'export function resolveMachineRoot() {}\n',
};
const readFile = async (file) => {
  const key = path.relative('/repo', file).split(path.sep).join('/');
  if (!(key in sources)) throw Object.assign(new Error('missing'), { code: 'ENOENT' });
  return sources[key];
};

test('related exports rank source modules owning the Ask concepts and skip unrelated or non-source files', async () => {
  const found = await relatedExports('/repo', [...Object.keys(sources), 'src/gone.mjs'],
    'Persist redacted machine history under the machine root', { readFile });
  assert.deepEqual(found.map(({ file }) => file).sort(), ['src/lib/paths.mjs', 'src/lib/redaction.mjs']);
  assert.deepEqual(found.find(({ file }) => file === 'src/lib/paths.mjs').exports, ['resolveMachineRoot']);
  assert.deepEqual(await relatedExports('/repo', undefined, 'machine root', { readFile }), []);
});

test('the feature planner receives existing exports and is told to reuse them', async () => {
  const existingExports = [{ file: 'src/lib/paths.mjs', exports: ['resolveMachineRoot'] }];
  const outline = { outcomes: ['History persists'], issues: [
    { title: 'Schema', outcome: 'Defined', acceptance_checks: ['node --test exits 0'], wave: 1 },
    { title: 'Writer', outcome: 'Written', acceptance_checks: ['node --test exits 0'], wave: 2 },
  ] };
  let body;
  await planOutline('Persist machine history.', {
    kind: 'feature', env: {}, existingExports,
    config: { llm: { base_url: 'http://127.0.0.1:9/v1', model: 'stub', request_timeout_ms: 1000 },
      planner: { turn_budget: 2 } },
    fetchImpl: async (_url, request) => {
      body = JSON.parse(request.body);
      return Response.json({ choices: [{ finish_reason: 'stop',
        message: { role: 'assistant', content: JSON.stringify(outline) } }] });
    },
  });
  assert.deepEqual(JSON.parse(body.messages[1].content).existing_exports, existingExports);
  assert.match(body.messages[0].content, /never invent a parallel module, state root, or hardcoded path/);
});
