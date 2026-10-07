import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { loadContext, moduleExports, readPublicSeams } from '../src/runtime/context.mjs';

test('public seams list export signatures of direct imports, matching module first, never forbidden paths', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'roster-seams-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, 'src', 'lib'), { recursive: true });
  mkdirSync(join(root, 'tests'));
  writeFileSync(join(root, 'src', 'lib', 'alpha.mjs'), 'export const ALPHA = 1;\n');
  writeFileSync(join(root, 'src', 'lib', 'route.mjs'),
    'const hidden = 1;\nexport function formatRoute(choice,\n  env = process.env) {\n  return hidden;\n}\nexport async function routeTask() {}\n');
  writeFileSync(join(root, 'tests', 'route.test.mjs'),
    "import { ALPHA } from '../src/lib/alpha.mjs';\nimport { formatRoute } from '../src/lib/route.mjs';\n" +
    "import { x } from '../.env.mjs';\nimport { y } from '../../outside.mjs';\n");
  const seams = await readPublicSeams(root, ['tests/route.test.mjs', 'tests/missing.test.mjs']);
  assert.equal(seams, '`src/lib/route.mjs`\n- export function formatRoute(choice, env = process.env)\n' +
    '- export async function routeTask()\n\n`src/lib/alpha.mjs`\n- export const ALPHA = 1;');
  assert.equal(await readPublicSeams(root, ['tests/missing.test.mjs', 'docs/*.md']), '');
});

test('earlier-wave modules render as a required reuse section in the coder context', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'roster-waves-ctx-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, 'src', 'lib'), { recursive: true });
  writeFileSync(join(root, 'src', 'lib', 'store.mjs'), 'export function openStore(root, options = {}) {}\n');
  assert.equal(await moduleExports(root, ['src/lib/store.mjs', 'src/lib/gone.mjs', '.env.mjs']),
    '`src/lib/store.mjs`\n- export function openStore(root, options = {})');
  writeFileSync(join(root, 'TASK.md'), '# Task\n\n## Ask\n\nBuild the API.\n\n## Outcome\n\nAPI\n\n' +
    '## Files allowed\n\n- `src/lib/api.mjs`\n\n## Acceptance checks\n\n- `node --test` exits 0.\n');
  writeFileSync(join(root, 'AGENTS.md'), '# Agents\n');
  const { pack } = await loadContext({ worktree: root, repoRoot: process.cwd(), config: { seat: { context_chars: 40000 }, llm: {} },
    env: {}, askKind: 'slice', principal: { id: 'p', content: 'principal' }, memoryPath: join(root, 'memory.jsonl'),
    priorWaveFiles: ['src/lib/store.mjs'] });
  assert.match(pack, /## Earlier waves delivered\n\nEarlier slices of this plan already merged these modules\. Import and extend them/);
  assert.match(pack, /- export function openStore\(root, options = \{\}\)/);
});

test('a Windows test slice tells the coder to prove link escapes with junctions, not skipped symlinks', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'roster-test-host-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  writeFileSync(join(root, 'TASK.md'), '# Task\n\n## Ask\n\nRefuse escapes.\n\n## Outcome\n\nSafe paths\n\n' +
    '## Files allowed\n\n- `src/lib/clean.mjs`\n- `tests/clean.test.mjs`\n\n## Acceptance checks\n\n- `node --test` exits 0.\n');
  writeFileSync(join(root, 'AGENTS.md'), '# Agents\n');
  const load = (platform) => {
    rmSync(join(root, 'CONTEXT.md'), { force: true });
    return loadContext({ worktree: root, repoRoot: process.cwd(), config: { seat: { context_chars: 40000 }, llm: {} },
      env: {}, askKind: 'slice', principal: { id: 'p', content: 'principal' }, memoryPath: join(root, 'memory.jsonl'), platform });
  };
  assert.match((await load('win32')).pack, /## Test host\n\n.*skipped test is not evidence.*'junction'/);
  assert.doesNotMatch((await load('linux')).pack, /## Test host/);
});
