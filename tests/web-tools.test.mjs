import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { createTools } from '../src/runtime/tools.mjs';

function page(body, { status = 200, type = 'text/html', location } = {}) {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: (name) => name === 'content-type' ? type : name === 'location' ? location : null },
    json: async () => body,
    text: async () => typeof body === 'string' ? body : '',
  };
}

async function toolsWith(fetchImpl, allowInternet = true) {
  const worktree = await mkdtemp(path.join(os.tmpdir(), 'roster-web-'));
  await writeFile(path.join(worktree, 'TASK.md'), 'Allowed files: README.md\n');
  const tools = await createTools({
    worktree, allowedFiles: ['README.md'], allowInternet, fetchImpl,
  });
  return { worktree, tools };
}

test('web_search returns titles and refuses when internet is off', async () => {
  const { worktree, tools } = await toolsWith(async () => page({
    Heading: 'Roster', AbstractText: 'A harness', AbstractURL: 'https://example.com/roster', RelatedTopics: [],
  }));
  try {
    const result = await tools.web_search({ query: 'agent harness' });
    assert.equal(result.results[0].url, 'https://example.com/roster');
    assert.equal(result.untrusted, true);
    const blocked = await createTools({ worktree, allowedFiles: ['README.md'], allowInternet: false, fetchImpl: async () => { throw new Error('network'); } });
    await assert.rejects(blocked.web_search({ query: 'x' }), /disabled/);
  } finally {
    await rm(worktree, { recursive: true, force: true });
  }
});

test('web_fetch accepts a searched URL and refuses a guessed or private URL', async () => {
  const { worktree, tools } = await toolsWith(async (url) => {
    if (String(url).includes('duckduckgo')) {
      return page({ Heading: 'Docs', AbstractText: 'Fleet probe', AbstractURL: 'https://example.com/docs', RelatedTopics: [] });
    }
    return page('<html><script>secret()</script><body><p>Fleet probe</p></body></html>');
  });
  try {
    await assert.rejects(tools.web_fetch({ url: 'https://example.com/docs' }), /returned by web_search/);
    await tools.web_search({ query: 'agent harness' });
    const result = await tools.web_fetch({ url: 'https://example.com/docs' });
    assert.match(result.text, /Fleet probe/);
    assert.equal(result.untrusted, true);
    assert.equal(result.text.includes('secret'), false);
    await assert.rejects(tools.web_fetch({ url: 'https://github.com/guessed' }), /returned by web_search/);
    await assert.rejects(tools.web_fetch({ url: 'https://127.0.0.1/secret' }), /private or local/);
    await assert.rejects(tools.web_fetch({ url: 'https://user:pass@example.com' }), /credentials/);
  } finally {
    await rm(worktree, { recursive: true, force: true });
  }
});

test('web_search falls back to result links when the instant answer is empty', async () => {
  const { worktree, tools } = await toolsWith(async (url) => String(url).includes('html')
    ? page('<a href="https://duckduckgo.com/l/?uddg=https%3A%2F%2Fdocs.github.com%2Fen%2Fauthentication">GitHub</a>')
    : page({ RelatedTopics: [] }));
  try {
    const result = await tools.web_search({ query: 'GitHub agent identity commit trailers' });
    assert.equal(result.results[0].url, 'https://docs.github.com/en/authentication');
  } finally {
    await rm(worktree, { recursive: true, force: true });
  }
});

test('web_fetch refuses a private redirect and a non-text body', async () => {
  const { worktree, tools } = await toolsWith(async (url) => {
    const href = String(url);
    if (href.includes('duckduckgo')) {
      return page({
        AbstractText: 'Docs', AbstractURL: 'https://example.com/start', RelatedTopics: [
          { Text: 'Other', FirstURL: 'https://example.com/file' },
        ],
      });
    }
    if (href.endsWith('/start')) return page('', { status: 302, location: 'https://127.0.0.1/secret' });
    return page('binary', { type: 'application/octet-stream' });
  });
  try {
    await tools.web_search({ query: 'agent harness' });
    await assert.rejects(tools.web_fetch({ url: 'https://example.com/start' }), /private or local/);
    await assert.rejects(tools.web_fetch({ url: 'https://example.com/file' }), /text\/html or text\/plain/);
  } finally {
    await rm(worktree, { recursive: true, force: true });
  }
});
