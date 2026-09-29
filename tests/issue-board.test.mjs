import assert from 'node:assert/strict';
import { generateKeyPairSync } from 'node:crypto';
import { copyFileSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  closeMergedIssue, issueMergeMessage, mergedPullNumber, mergedPullNumberFromFailure,
} from '../src/lib/issue-board.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
const issue = { number: 42, url: 'https://github.com/example/project/issues/42' };
const runLine = '1|-|local@unknown|m|3/-|2|roster-42-coder|issue-42';

function fixture(t) {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-issue-close-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  copyFileSync(join(root, 'agent-policy.yml'), join(repoRoot, 'agent-policy.yml'));
  return {
    repoRoot, cwd: repoRoot,
    env: { GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'not-read.pem' },
    readKey: () => privateKey,
    git: () => 'https://github.com/example/project.git\n',
  };
}

function githubResponses({
  merged = true, alreadyClosed = false, expectedRunLine = runLine,
  headRepo = 'example/project',
} = {}) {
  const calls = [];
  const fetchImpl = async (url, request) => {
    const route = new URL(url).pathname;
    const body = request.body && JSON.parse(request.body);
    calls.push({ route, method: request.method, body });
    assert.match(request.headers.Authorization, /^Bearer \S+$/);
    if (route === '/repos/example/project/installation') {
      return Response.json({ id: 9, app_id: 123, app_slug: 'example-agent',
        suspended_at: null });
    }
    if (route === '/app/installations/9/access_tokens') {
      assert.deepEqual(body, { repositories: ['project'],
        permissions: { issues: 'write', pull_requests: 'read' } });
      return Response.json({ token: 'test-only-installation-token',
        expires_at: new Date(Date.now() + 3_600_000).toISOString() });
    }
    if (route === '/repos/example/project/pulls/7') {
      return Response.json({
        merged, state: 'closed', head: { ref: 'issue-42', repo: { full_name: headRepo } },
        base: { repo: { full_name: 'example/project' } },
        html_url: 'https://github.com/example/project/pull/7',
        body: issueMergeMessage('feat: issue 42', 42),
      });
    }
    if (route === '/repos/example/project/issues/42/comments') {
      assert.equal(body.body,
        `Merged https://github.com/example/project/pull/7 for issue #42.` +
        (expectedRunLine ? `\n\nAI-Run: ${expectedRunLine}` : ''));
      return Response.json({ id: 55 }, { status: 201 });
    }
    if (route === '/repos/example/project/issues/42') {
      return Response.json({ number: 42,
        state: request.method === 'PATCH' ? 'closed' : alreadyClosed ? 'closed' : 'open' });
    }
    if (route === '/installation/token') {
      assert.equal(request.method, 'DELETE');
      return new Response(null, { status: 204 });
    }
    assert.fail(`Unexpected issue API route: ${request.method} ${route}`);
  };
  return { calls, fetchImpl };
}

test('publisher message links the issue and only confirmed merge output provides a PR', () => {
  assert.equal(issueMergeMessage('feat: issue 42', 42), 'feat: issue 42\n\nCloses #42');
  assert.equal(mergedPullNumber('Merged PR #7 with a merge commit, removed its branch.\n'), 7);
  assert.throws(() => mergedPullNumber('Created a draft pull request.\n'), /did not confirm a merged PR/);
  assert.equal(mergedPullNumberFromFailure('PR #7 was merged; local cleanup is incomplete.'), 7);
  assert.equal(mergedPullNumberFromFailure('GitHub API request failed (HTTP 422).'), null);
});

test('App comments with AI-Run and closes the issue only after verifying the merged PR', async (t) => {
  const { calls, fetchImpl } = githubResponses();
  const result = await closeMergedIssue({
    ...fixture(t), issue, pullNumber: 7, runLine, fetchImpl,
  });
  assert.deepEqual(result, { issueNumber: 42, pullNumber: 7, commentId: 55 });
  assert.deepEqual(calls.map(({ route, method }) => `${method} ${route}`), [
    'GET /repos/example/project/installation',
    'POST /app/installations/9/access_tokens',
    'GET /repos/example/project/pulls/7',
    'POST /repos/example/project/issues/42/comments',
    'GET /repos/example/project/issues/42',
    'PATCH /repos/example/project/issues/42',
    'DELETE /installation/token',
  ]);
});

test('already-closed issues are commented without a redundant close request', async (t) => {
  const { calls, fetchImpl } = githubResponses({ alreadyClosed: true, expectedRunLine: null });
  await closeMergedIssue({ ...fixture(t), issue, pullNumber: 7, fetchImpl });
  assert.equal(calls.filter(({ method }) => method === 'PATCH').length, 0);
});

test('unmerged PRs cannot receive issue comments or closures', async (t) => {
  const { calls, fetchImpl } = githubResponses({ merged: false });
  await assert.rejects(closeMergedIssue({
    ...fixture(t), issue, pullNumber: 7, runLine, fetchImpl,
  }), /not confirmed merged/);
  assert.equal(calls.filter(({ route }) => route.includes('/issues/42')).length, 0);
  assert.equal(calls.at(-1).route, '/installation/token');
});

test('a merged PR from a different head repository cannot close this issue', async (t) => {
  const { calls, fetchImpl } = githubResponses({ headRepo: 'other/fork' });
  await assert.rejects(closeMergedIssue({
    ...fixture(t), issue, pullNumber: 7, runLine, fetchImpl,
  }), /not confirmed merged/);
  assert.equal(calls.filter(({ route }) => route.includes('/issues/42')).length, 0);
});
