import assert from 'node:assert/strict';
import { generateKeyPairSync } from 'node:crypto';
import { copyFileSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  commentMergedIssue, issueMergeMessage, mergedPullNumber, mergedPullNumberFromFailure, setIssueRunStatus,
} from '../src/lib/issue-board.mjs';
import { materializeRun } from '../src/metrics/run.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
const issue = { number: 42, url: 'https://github.com/example/project/issues/42' };
const runLine = '1|local|local-model@-|m|3/-|2|roster-42-coder|issue-42';
const model = 'local-model';

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

function githubResponses({ merged = true, issueState = 'open', expectedRunLine = runLine,
  expectedModel = model, expectedMetadata = '',
  headRepo = 'example/project', htmlUrl = 'https://github.com/example/project/pull/7',
  prBody = issueMergeMessage('feat: issue 42', 42) } = {}) {
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
        html_url: htmlUrl, body: prBody,
      });
    }
    if (route === '/repos/example/project/issues/42/comments') {
      assert.equal(body.body,
        `Merged https://github.com/example/project/pull/7 for issue #42.\n\nModel: ${expectedModel}` +
        expectedMetadata +
        (expectedRunLine ? `\nAI-Run: ${expectedRunLine}` : '') +
        '\n\nIssue remains open for human AI-Eval.');
      return Response.json({ id: 55 }, { status: 201 });
    }
    if (route === '/repos/example/project/issues/42') {
      assert.equal(request.method, 'GET', 'Issue state must never be patched by the App');
      return Response.json({ number: 42, state: issueState });
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
  assert.equal(issueMergeMessage('feat: issue 42', 42), 'feat: issue 42\n\nRefs #42');
  for (const closing of ['Closes #42', 'Fixes #42', 'Resolved example/project#42']) {
    assert.throws(() => issueMergeMessage(`feat: issue 42\n\n${closing}`, 42), /must remain open/);
  }
  assert.equal(issueMergeMessage('fix: Closes #420', 42), 'fix: Closes #420\n\nRefs #42');
  assert.equal(mergedPullNumber('Merged PR #7 with a merge commit, removed its branch.\n'), 7);
  assert.throws(() => mergedPullNumber('Created a draft pull request.\n'), /did not confirm a merged PR/);
  assert.equal(mergedPullNumberFromFailure('PR #7 was merged; local cleanup is incomplete.'), 7);
  assert.equal(mergedPullNumberFromFailure('GitHub API request failed (HTTP 422).'), null);
});

test('App comments with PR URL, model, and AI-Run without closing the open issue', async (t) => {
  const { calls, fetchImpl } = githubResponses();
  const result = await commentMergedIssue({
    ...fixture(t), issue, pullNumber: 7, model, runLine, fetchImpl,
  });
  assert.deepEqual(result, { issueNumber: 42, pullNumber: 7, commentId: 55, issueState: 'open' });
  assert.deepEqual(calls.map(({ route, method }) => `${method} ${route}`), [
    'GET /repos/example/project/installation',
    'POST /app/installations/9/access_tokens',
    'GET /repos/example/project/pulls/7',
    'GET /repos/example/project/issues/42',
    'POST /repos/example/project/issues/42/comments',
    'DELETE /installation/token',
  ]);
});

test('merged issue comments use the complete measured object and omit unknown counts and capacity', async (t) => {
  for (const reported of [true, false]) {
    const run = materializeRun({
      provider: 'vllm', model: 'actual-served-model', effort: 'h',
      ...(reported ? { prompt_tokens: 100, completion_tokens: 40, context_max: 8192 } : {}),
      session: 'roster-42-coder', task: 'issue-42',
    });
    const { calls, fetchImpl } = githubResponses({
      expectedModel: run.metrics.model, expectedRunLine: run.line,
      expectedMetadata: '\nProvider: vllm' +
        (reported ? '\nPrompt tokens: 100\nCompletion tokens: 40\nContext max: 8192' : '') +
        '\nSession: roster-42-coder\nTask: issue-42',
    });
    await commentMergedIssue({
      ...fixture(t), issue, pullNumber: 7, run, model: 'GPT-6.1-Sol', runLine, fetchImpl,
    });
    const body = calls.find(({ route }) => route.endsWith('/comments')).body.body;
    assert.doesNotMatch(body, /GPT-6\.1-Sol|1000000|not-read\.pem|installation-token|PRIVATE KEY/);
    if (!reported) assert.doesNotMatch(body, /Prompt tokens|Completion tokens|Context max/);
    assert.equal(calls.at(-1).route, '/installation/token');
  }
});

test('an already-closed issue is reported without another App comment or state change', async (t) => {
  const { calls, fetchImpl } = githubResponses({ issueState: 'closed' });
  await assert.rejects(commentMergedIssue({ ...fixture(t), issue, pullNumber: 7, model, fetchImpl }),
    /must remain open for human AI-Eval/);
  assert.equal(calls.filter(({ route }) => route.endsWith('/comments')).length, 0);
  assert.equal(calls.at(-1).route, '/installation/token');
});

test('unmerged PRs cannot receive issue comments', async (t) => {
  const { calls, fetchImpl } = githubResponses({ merged: false });
  await assert.rejects(commentMergedIssue({
    ...fixture(t), issue, pullNumber: 7, model, runLine, fetchImpl,
  }), /not confirmed merged/);
  assert.equal(calls.filter(({ route }) => route.includes('/issues/42')).length, 0);
  assert.equal(calls.at(-1).route, '/installation/token');
});

test('a merged PR from another repository cannot comment on this issue', async (t) => {
  const { calls, fetchImpl } = githubResponses({ headRepo: 'other/fork' });
  await assert.rejects(commentMergedIssue({
    ...fixture(t), issue, pullNumber: 7, model, runLine, fetchImpl,
  }), /not confirmed merged/);
  assert.equal(calls.filter(({ route }) => route.includes('/issues/42')).length, 0);
});

test('PR URL or non-closing reference mismatch blocks comment before issue mutation', async (t) => {
  for (const changed of [
    { htmlUrl: 'https://github.com/other/project/pull/7' },
    { htmlUrl: 123 },
    { prBody: 'feat: issue 42\n\nCloses #42' },
  ]) {
    const { calls, fetchImpl } = githubResponses(changed);
    await assert.rejects(commentMergedIssue({
      ...fixture(t), issue, pullNumber: 7, model, runLine, fetchImpl,
    }), /not confirmed merged/);
    assert.equal(calls.filter(({ route }) => route.includes('/issues/42')).length, 0);
  }
});

test('invalid model and mismatched AI-Run fail before minting an App token', async (t) => {
  const { calls, fetchImpl } = githubResponses();
  for (const options of [
    { model: 'unknown', runLine },
    { model: 'other-model', runLine },
    { model: undefined, runLine: undefined },
  ]) {
    await assert.rejects(commentMergedIssue({
      ...fixture(t), issue, pullNumber: 7, fetchImpl, ...options,
    }), /set model|AI-Run model@version/);
  }
  assert.deepEqual(calls, []);
});

function statusResponses({ labels = [], state = 'open', projects = false, labelExists = false,
  nodes = [{ id: 'item-1', project: { id: 'project-1', field: { id: 'field-1',
    options: [{ id: 'opt-progress', name: 'In progress' }, { id: 'opt-review', name: 'In Review' }] } } }] } = {}) {
  const calls = [];
  const fetchImpl = async (url, request) => {
    const route = decodeURIComponent(new URL(url).pathname);
    const body = request.body && JSON.parse(request.body);
    calls.push({ route, method: request.method, body });
    if (route === '/repos/example/project/installation') {
      return Response.json({ id: 9, app_id: 123, suspended_at: null,
        permissions: projects ? { issues: 'write', organization_projects: 'write' } : { issues: 'write' } });
    }
    if (route === '/app/installations/9/access_tokens') {
      return Response.json({ token: 'test-only-installation-token',
        expires_at: new Date(Date.now() + 3_600_000).toISOString() });
    }
    if (route === '/repos/example/project/issues/42' && request.method === 'GET') {
      return Response.json({ number: 42, state, labels: labels.map((name) => ({ name })) });
    }
    if (route.startsWith('/repos/example/project/issues/42/labels/') && request.method === 'DELETE') {
      return Response.json([]);
    }
    if (route === '/repos/example/project/labels' && request.method === 'POST') {
      return labelExists ? new Response('{}', { status: 422 }) : Response.json({ id: 1 }, { status: 201 });
    }
    if (route === '/repos/example/project/issues/42/labels' && request.method === 'POST') return Response.json([]);
    if (route === '/repos/example/project/issues/42/comments') return Response.json({ id: 77 }, { status: 201 });
    if (route === '/graphql') {
      return Response.json(body.query.startsWith('mutation') ? { data: { updateProjectV2ItemFieldValue: {} } }
        : { data: { repository: { issue: { projectItems: { nodes } } } } });
    }
    if (route === '/installation/token') return new Response(null, { status: 204 });
    assert.fail(`Unexpected status API route: ${request.method} ${route}`);
  };
  return { calls, fetchImpl };
}

test('run status claims the issue: swaps roster labels, creates the label, comments, and revokes', async (t) => {
  const { calls, fetchImpl } = statusResponses({ labels: ['wave:4', 'roster:blocked'], labelExists: true });
  const result = await setIssueRunStatus({ ...fixture(t), issue, status: 'in-progress',
    detail: ['Branch: issue-42', 'line\nbreak'], fetchImpl });
  assert.deepEqual(result, { issueNumber: 42, status: 'in-progress', label: 'roster:in-progress',
    commentId: 77, claimed: false, board: 'skipped' });
  assert.deepEqual(calls.find(({ route }) => route.endsWith('/access_tokens')).body.permissions,
    { issues: 'write', pull_requests: 'read' });
  assert.ok(calls.some(({ route, method }) => method === 'DELETE' && route.endsWith('/labels/roster:blocked')));
  assert.deepEqual(calls.find(({ route }) => route.endsWith('/issues/42/labels')).body, { labels: ['roster:in-progress'] });
  const comment = calls.find(({ route }) => route.endsWith('/comments')).body.body;
  assert.match(comment, /^Roster run started for issue #42\.\n\n- Branch: issue-42\n- line break\n\nStatus label: `roster:in-progress`\.$/);
  assert.equal(calls.filter(({ route }) => route === '/graphql').length, 0);
  assert.equal(calls.at(-1).route, '/installation/token');
});

test('run status reports another claim and moves an accessible project Status column', async (t) => {
  const { calls, fetchImpl } = statusResponses({ labels: ['roster:in-progress'], projects: true });
  const claimed = await setIssueRunStatus({ ...fixture(t), issue, status: 'in-progress', fetchImpl });
  assert.equal(claimed.claimed, true);
  assert.equal(claimed.board, 'moved');
  assert.equal(calls.filter(({ route }) => route.endsWith('/labels')).length, 0, 'an existing label is not re-added');
  assert.equal(calls.find(({ route }) => route.endsWith('/access_tokens')).body.permissions.organization_projects, 'write');
  assert.equal(calls.filter(({ route }) => route === '/graphql').at(-1).body.variables.option, 'opt-progress');

  const review = statusResponses({ labels: ['roster:in-progress'], projects: true });
  const result = await setIssueRunStatus({ ...fixture(t), issue, status: 'review', comment: false,
    fetchImpl: review.fetchImpl });
  assert.equal(result.commentId, null);
  assert.equal(review.calls.filter(({ route }) => route.endsWith('/comments')).length, 0);
  assert.equal(review.calls.filter(({ route }) => route === '/graphql').at(-1).body.variables.option, 'opt-review');

  const blocked = statusResponses({ projects: true });
  assert.equal((await setIssueRunStatus({ ...fixture(t), issue, status: 'blocked',
    fetchImpl: blocked.fetchImpl })).board, 'unchanged');
});

test('run status never touches closed issues and rejects unknown states before any request', async (t) => {
  const { calls, fetchImpl } = statusResponses({ state: 'closed' });
  await assert.rejects(setIssueRunStatus({ ...fixture(t), issue, status: 'review', fetchImpl }), /only set on open issues/);
  assert.equal(calls.filter(({ route }) => /labels|comments/.test(route)).length, 0);
  assert.equal(calls.at(-1).route, '/installation/token');
  const none = statusResponses();
  await assert.rejects(setIssueRunStatus({ ...fixture(t), issue, status: 'done', fetchImpl: none.fetchImpl }),
    /Run status must be one of/);
  assert.deepEqual(none.calls, []);
});
