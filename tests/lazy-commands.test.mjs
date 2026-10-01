import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { commandRegistry, completeCommand } from '../src/repl.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));

test('help, status, quit, and completion use the command registry without loading seat or publisher modules', (t) => {
  const directory = mkdtempSync(path.join(os.tmpdir(), 'roster-lazy-commands-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const loader = path.join(directory, 'guard-loader.mjs');
  writeFileSync(loader, `
export async function resolve(specifier, context, nextResolve) {
  const resolved = await nextResolve(specifier, context);
  if (/\\/(?:lib\\/llm|llm\\/(?:openai|http)|issue-board|seats\\/reviewer)\\.mjs$/.test(new URL(resolved.url).pathname) ||
      /github-agent-contracts\\/scripts\\/agent-pr\\.mjs$/.test(new URL(resolved.url).pathname)) {
    throw new Error('A deferred module was imported before publication or inference: ' +
      resolved.url + ' from ' + context.parentURL);
  }
  return resolved;
}
`, 'utf8');
  const replUrl = new URL('../src/repl.mjs', import.meta.url).href;
  const source = `
import { createDispatcher } from ${JSON.stringify(replUrl)};
const shell = createDispatcher({
  cwd: process.cwd(), repoRoot: process.cwd(),
  config: { seat: { id: 'coder' }, llm: { base_url: '', model: '', effort: '' } },
  env: { GITHUB_APP_PRIVATE_KEY_PATH: 'missing-private-key.pem' },
  output: { write() {} }, errorOutput: { write() {} },
  services: {
    repositoryRoot: () => process.cwd(),
    readStatus: async () => ({}), formatStatus: () => 'status\\\\n',
  },
});
await shell.dispatch('/help');
await shell.dispatch('/status');
await shell.dispatch('/quit');
`;
  const result = spawnSync(process.execPath, ['--experimental-loader', pathToFileURL(loader).href, '--input-type=module',
    '--eval', source], { cwd: directory, encoding: 'utf8', timeout: 10_000 });
  assert.equal(result.status, 0, result.stderr);
  assert.ok(commandRegistry.includes('diff'));
  assert.ok(commandRegistry.includes('status'));
  assert.deepEqual(completeCommand('/st'), [['/status', '/stats'], '/st']);
  assert.deepEqual(completeCommand('/d'), [['/diff'], '/d']);
});
