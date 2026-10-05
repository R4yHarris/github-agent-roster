import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { readPublicSeams } from '../src/runtime/context.mjs';

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
