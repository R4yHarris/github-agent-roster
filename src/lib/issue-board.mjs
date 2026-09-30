import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { materializeRun, resolvePublishModel } from '../metrics/run.mjs';
import { resolveContractsPath } from './paths.mjs';

const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));

function positiveNumber(value, name) {
  if (!/^[1-9]\d*$/.test(String(value)) || !Number.isSafeInteger(Number(value))) {
    throw new TypeError(`${name} must be a positive safe integer`);
  }
  return Number(value);
}

async function request(pathname, token, { method = 'GET', body, fetchImpl }) {
  let response;
  try {
    response = await fetchImpl(`https://api.github.com${pathname}`, {
      method,
      headers: {
        Accept: 'application/vnd.github+json',
        Authorization: `Bearer ${token}`,
        'Content-Type': 'application/json',
        'User-Agent': 'github-agent-roster',
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      redirect: 'error',
      signal: AbortSignal.timeout(30_000),
    });
  } catch {
    throw new Error('GitHub issue API request failed; check connectivity and App authentication');
  }
  if (!response.ok) throw new Error(`GitHub issue API request failed (HTTP ${Number(response.status)})`);
  if (response.status === 204) return null;
  try {
    return await response.json();
  } catch {
    throw new Error('GitHub issue API returned invalid JSON');
  }
}

export function issueMergeMessage(subject, issueNumber) {
  const number = positiveNumber(issueNumber, 'Issue number');
  if (typeof subject !== 'string' || !subject.trim() || subject.includes('\0')) {
    throw new TypeError('Publish subject must be nonempty text');
  }
  const closing = new RegExp(
    `\\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\\s+(?:[A-Za-z0-9-]+/[A-Za-z0-9_.-]+)?#${number}(?!\\d)`,
    'i',
  );
  if (closing.test(subject)) {
    throw new Error(`Issue #${number} must remain open until human AI-Eval; use Refs #${number}, not a closing keyword`);
  }
  return `${subject.trimEnd()}\n\nRefs #${number}`;
}

export function mergedPullNumber(output) {
  const match = typeof output === 'string'
    ? /^Merged PR #([1-9]\d*) with a merge commit,/m.exec(output) : null;
  if (!match) throw new Error('Publisher did not confirm a merged PR; issue remains open');
  return positiveNumber(match[1], 'Merged PR number');
}

export function mergedPullNumberFromFailure(output) {
  const match = /\bPR #([1-9]\d*) was merged;/.exec(String(output ?? ''));
  return match ? positiveNumber(match[1], 'Merged PR number') : null;
}

export async function commentMergedIssue({
  issue, pullNumber, model, runLine, run, repoRoot, cwd = process.cwd(),
  env = process.env, fetchImpl = globalThis.fetch,
  git = execFileSync, readKey = readFileSync,
} = {}) {
  const number = positiveNumber(issue?.number, 'Issue number');
  const pull = positiveNumber(pullNumber, 'Merged PR number');
  const completed = run?.metrics ? materializeRun(run.metrics, run.version) : null;
  const actualModel = completed?.metrics.model ?? resolvePublishModel({ env: { AI_MODEL: model } });
  if (completed) runLine = completed.line;
  if (typeof runLine !== 'undefined' &&
      (typeof runLine !== 'string' || !runLine || /[\r\n]/.test(runLine))) {
    throw new TypeError('AI-Run must be one nonempty line');
  }
  const contractsPath = resolveContractsPath({ repoRoot: rosterRoot, cwd, env });
  const sdk = await import(pathToFileURL(path.join(contractsPath, 'scripts', 'agent-pr.mjs')).href);
  const policyPack = await import(pathToFileURL(path.join(contractsPath, 'scripts', 'load-agent-policy.mjs')).href);
  if (runLine) {
    const { parseAgentRun } = await import(pathToFileURL(path.join(contractsPath, 'scripts', 'parse-agent-run.mjs')).href);
    if (!parseAgentRun(runLine, actualModel)) throw new Error('Coder AI-Run must match the published model');
  }
  const policy = policyPack.loadAgentPolicy({ cwd: repoRoot });
  policyPack.requireCapability(policy, 'coder', 'comment');
  policyPack.requireCapability(policy, 'merger', 'merge');
  const origin = git('git', ['remote', 'get-url', 'origin'], {
    cwd: repoRoot, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
  }).trim();
  const { owner, repository } = sdk.parseOrigin(origin);
  const slug = `${owner}/${repository}`;
  if (typeof issue.url !== 'string' ||
      issue.url.toLowerCase() !== `https://github.com/${slug}/issues/${number}`.toLowerCase()) {
    throw new Error('The issue does not belong to the publishing repository');
  }
  if (!env.GITHUB_APP_ID || !env.GITHUB_APP_PRIVATE_KEY_PATH) {
    throw new Error('Issue comment requires GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH');
  }
  let key;
  try {
    key = readKey(path.resolve(cwd, env.GITHUB_APP_PRIVATE_KEY_PATH));
  } catch {
    throw new Error('Could not load the App key for issue comment');
  }
  let jwt;
  try {
    jwt = sdk.createAppJwt(env.GITHUB_APP_ID, key);
  } finally {
    if (Buffer.isBuffer(key)) key.fill(0);
  }
  const installation = await request(`/repos/${owner}/${repository}/installation`, jwt, { fetchImpl });
  if (!Number.isSafeInteger(installation?.id) || installation.id <= 0 ||
      String(installation.app_id) !== String(env.GITHUB_APP_ID) || installation.suspended_at) {
    throw new Error('The App installation does not match the issue repository');
  }
  const access = await request(`/app/installations/${installation.id}/access_tokens`, jwt, {
    method: 'POST', body: {
      repositories: [repository], permissions: { issues: 'write', pull_requests: 'read' },
    }, fetchImpl,
  });
  if (typeof access?.token !== 'string' || !access.token || /\s/.test(access.token) ||
      !Number.isFinite(Date.parse(access.expires_at)) || Date.parse(access.expires_at) <= Date.now()) {
    throw new Error('GitHub did not return a valid issue-scoped App token');
  }
  const token = access.token;
  const repoPath = `/repos/${owner}/${repository}`;
  let result;
  let failure;
  try {
    const pr = await request(`${repoPath}/pulls/${pull}`, token, { fetchImpl });
    if (pr?.merged !== true || pr.state !== 'closed' || pr.head?.ref !== `issue-${number}` ||
        pr.head?.repo?.full_name?.toLowerCase() !== slug.toLowerCase() ||
        pr.base?.repo?.full_name?.toLowerCase() !== slug.toLowerCase() ||
        typeof pr.html_url !== 'string' ||
        pr.html_url.toLowerCase() !== `https://github.com/${slug}/pull/${pull}`.toLowerCase() ||
        typeof pr.body !== 'string' ||
        !new RegExp(`(?:^|\\n)Refs #${number}(?:\\r?\\n|$)`).test(pr.body)) {
      throw new Error('PR was not confirmed merged for this issue; leaving the issue unchanged');
    }
    const current = await request(`${repoPath}/issues/${number}`, token, { fetchImpl });
    if (current?.number !== number || current.state !== 'open') {
      throw new Error('Issue must remain open for human AI-Eval; no comment was posted');
    }
    const body = `Merged ${pr.html_url} for issue #${number}.\n\nModel: ${actualModel}` +
      (completed ? `\nProvider: ${completed.metrics.provider}` +
        (completed.metrics.prompt_tokens === undefined ? '' : `\nPrompt tokens: ${completed.metrics.prompt_tokens}`) +
        (completed.metrics.completion_tokens === undefined ? '' : `\nCompletion tokens: ${completed.metrics.completion_tokens}`) +
        (completed.metrics.context_max === undefined ? '' : `\nContext max: ${completed.metrics.context_max}`) +
        (completed.metrics.session === undefined ? '' : `\nSession: ${completed.metrics.session}`) +
        (completed.metrics.task === undefined ? '' : `\nTask: ${completed.metrics.task}`) : '') +
      (runLine ? `\nAI-Run: ${runLine}` : '') +
      '\n\nIssue remains open for human AI-Eval.';
    const comment = await request(`${repoPath}/issues/${number}/comments`, token, {
      method: 'POST', body: { body }, fetchImpl,
    });
    if (!Number.isSafeInteger(comment?.id) || comment.id <= 0) {
      throw new Error('GitHub did not confirm the issue comment');
    }
    result = { issueNumber: number, pullNumber: pull, commentId: comment.id, issueState: 'open' };
  } catch (error) {
    failure = error;
  } finally {
    try {
      await request('/installation/token', token, { method: 'DELETE', fetchImpl });
    } catch (error) {
      failure = new Error(`${failure?.message ?? 'Issue comment posted.'} App token revocation failed`, {
        cause: failure ?? error,
      });
    }
  }
  if (failure) throw failure;
  return result;
}
