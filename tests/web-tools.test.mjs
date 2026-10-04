import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { createTools } from '../src/runtime/tools.mjs';

async function toolsWith(fetchImpl, allowInternet = true) {
  const worktree = await mkdtemp(path.join(os.tmpdir(), 'roster-web-'));
  await writeFile(path.join(worktree, 'TASK.md'), 'Allowed files: README.md\n');
  const tools = await createTools({
    worktree, allowedFiles: ['README.md'], allowInternet, fetchImpl,
  });
  return { worktree, tools };
}

test('web_search returns titles and refuses when internet is off', async () => {
  const { worktree, tools } = await toolsWith(async () => ({
    ok: true,
    json: async () => ({ Heading: 'Roster', AbstractText: 'A harness', AbstractURL: 'https://example.com/roster', RelatedTopics: [] }),
  }));
  try {
    const result = await tools.web_search({ query: 'agent harness' });
    assert.equal(result.results[0].url, 'https://example.com/roster');
    const blocked = await createTools({ worktree, allowedFiles: ['README.md'], allowInternet: false, fetchImpl: async () => { throw new Error('network'); } });
    await assert.rejects(blocked.web_search({ query: 'x' }), /disabled/);
  } finally {
    await rm(worktree, { recursive: true, force: true });
  }
});

test('web_fetch strips markup and refuses private hosts', async () => {
  const { worktree, tools } = await toolsWith(async () => ({
    ok: true,
    text: async () => '<html><script>secret()</script><body><p>Fleet probe</p></body></html>',
  }));
  try {
    const result = await tools.web_fetch({ url: 'https://example.com/docs' });
    assert.match(result.text, /Fleet probe/);
    assert.equal(result.text.includes('secret'), false);
    await assert.rejects(tools.web_fetch({ url: 'http://127.0.0.1/secret' }), /private or local/);
    await assert.rejects(tools.web_fetch({ url: 'https://user:pass@example.com' }), /credentials/);
  } finally {
    await rm(worktree, { recursive: true, force: true });
  }
});
