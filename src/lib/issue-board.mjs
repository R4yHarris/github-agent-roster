import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { materializeRun, resolvePublishModel } from '../metrics/run.mjs';
import { resolveContractsPath } from './paths.mjs';

const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));
export { issueMergeMessage } from './issue-reference.mjs';

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
  const { sdk, policyPack, contractsPath } = await loadContracts({ cwd, env });
  if (runLine) {
    const { parseAgentRun } = await import(pathToFileURL(path.join(contractsPath, 'scripts', 'parse-agent-run.mjs')).href);
    if (!parseAgentRun(runLine, actualModel)) throw new Error('Coder AI-Run must match the published model');
  }
  return withIssueToken({
    sdk, policyPack, issue, number, repoRoot, cwd, env, fetchImpl, git, readKey,
    capabilities: [['coder', 'comment'], ['merger', 'merge']], purpose: 'Issue comment',
  }, async ({ token, slug, repoPath }) => {
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
    return { issueNumber: number, pullNumber: pull, commentId: comment.id, issueState: 'open' };
  });
}

async function loadContracts({ cwd, env }) {
  const contractsPath = resolveContractsPath({ repoRoot: rosterRoot, cwd, env });
  const sdk = await import(pathToFileURL(path.join(contractsPath, 'scripts', 'agent-pr.mjs')).href);
  const policyPack = await import(pathToFileURL(path.join(contractsPath, 'scripts', 'load-agent-policy.mjs')).href);
  return { sdk, policyPack, contractsPath };
}

// Mints an issue-scoped installation token for the App, runs `work`, and always revokes the token.
async function withIssueToken({
  sdk, policyPack, issue, number, repoRoot, cwd, env, fetchImpl, git, readKey, capabilities, purpose,
  projects = false,
}, work) {
  const policy = policyPack.loadAgentPolicy({ cwd: repoRoot });
  for (const [role, capability] of capabilities) policyPack.requireCapability(policy, role, capability);
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
    throw new Error(`${purpose} requires GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH`);
  }
  let key;
  try {
    key = readKey(path.resolve(cwd, env.GITHUB_APP_PRIVATE_KEY_PATH));
  } catch {
    throw new Error(`Could not load the App key for ${purpose.toLowerCase()}`);
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
      repositories: [repository], permissions: {
        issues: 'write', pull_requests: 'read',
        // Only request project access the installation already holds; asking for more fails the mint.
        ...(projects && installation.permissions?.organization_projects === 'write'
          ? { organization_projects: 'write' } : {}),
      },
    }, fetchImpl,
  });
  if (typeof access?.token !== 'string' || !access.token || /\s/.test(access.token) ||
      !Number.isFinite(Date.parse(access.expires_at)) || Date.parse(access.expires_at) <= Date.now()) {
    throw new Error('GitHub did not return a valid issue-scoped App token');
  }
  const token = access.token;
  const projectAccess = projects && installation.permissions?.organization_projects === 'write';
  let result;
  let failure;
  try {
    result = await work({ token, slug, owner, repository, projectAccess, repoPath: `/repos/${owner}/${repository}` });
  } catch (error) {
    failure = error;
  } finally {
    try {
      await request('/installation/token', token, { method: 'DELETE', fetchImpl });
    } catch (error) {
      failure = new Error(`${failure?.message ?? `${purpose} posted.`} App token revocation failed`, {
        cause: failure ?? error,
      });
    }
  }
  if (failure) throw failure;
  return result;
}

export const RUN_STATUS_LABELS = Object.freeze({
  'in-progress': Object.freeze({ name: 'roster:in-progress', color: '1d76db',
    description: 'A Roster run has claimed this issue' }),
  review: Object.freeze({ name: 'roster:review', color: '0e8a16',
    description: 'Roster delivered a reviewed result; awaiting human AI-Eval' }),
  blocked: Object.freeze({ name: 'roster:blocked', color: 'd93f0b',
    description: 'The last Roster run stopped; see the latest status comment' }),
});

const STATUS_HEADINGS = Object.freeze({
  'in-progress': 'Roster run started', review: 'Roster run delivered for review', blocked: 'Roster run blocked',
});

function statusDetail(detail) {
  if (detail === undefined || detail === null) return [];
  if (!Array.isArray(detail)) detail = [detail];
  return detail.map((line) => String(line).replace(/[\r\n]+/g, ' ').trim().slice(0, 300)).filter(Boolean).slice(0, 8);
}

// Issue labels and comments are the board (spec section 3): the App claims, reports, and parks the issue.
// It never closes or reopens it; closing stays with the human after AI-Eval.
export async function setIssueRunStatus({
  issue, status, detail, comment: postComment = true, repoRoot, cwd = process.cwd(),
  env = process.env, fetchImpl = globalThis.fetch,
  git = execFileSync, readKey = readFileSync,
} = {}) {
  const number = positiveNumber(issue?.number, 'Issue number');
  const label = RUN_STATUS_LABELS[status];
  if (!label) throw new TypeError(`Run status must be one of ${Object.keys(RUN_STATUS_LABELS).join(', ')}`);
  const { sdk, policyPack } = await loadContracts({ cwd, env });
  return withIssueToken({
    sdk, policyPack, issue, number, repoRoot, cwd, env, fetchImpl, git, readKey,
    capabilities: [['coder', 'comment'], ['coder', 'label']], purpose: 'Issue status', projects: true,
  }, async ({ token, owner, repository, projectAccess, repoPath }) => {
    const current = await request(`${repoPath}/issues/${number}`, token, { fetchImpl });
    if (current?.number !== number || current.state !== 'open') {
      throw new Error('Issue status is only set on open issues; no label or comment was posted');
    }
    const present = new Set((current.labels ?? []).map((entry) => typeof entry === 'string' ? entry : entry?.name));
    for (const other of Object.values(RUN_STATUS_LABELS)) {
      if (other.name === label.name || !present.has(other.name)) continue;
      try {
        await request(`${repoPath}/issues/${number}/labels/${encodeURIComponent(other.name)}`, token, {
          method: 'DELETE', fetchImpl,
        });
      } catch (error) {
        if (!/HTTP 404/.test(error.message)) throw error;
      }
    }
    if (!present.has(label.name)) {
      try {
        await request(`${repoPath}/labels`, token, { method: 'POST', fetchImpl,
          body: { name: label.name, color: label.color, description: label.description } });
      } catch (error) {
        if (!/HTTP 422/.test(error.message)) throw error;
      }
      await request(`${repoPath}/issues/${number}/labels`, token, {
        method: 'POST', body: { labels: [label.name] }, fetchImpl,
      });
    }
    const claimed = status === 'in-progress' && present.has(label.name);
    // The Projects Status column follows the label when the App can write the project (org-owned boards).
    const board = projectAccess ? await syncProjectStatus({ token, owner, repository, number, status, fetchImpl })
      .catch(() => 'unavailable') : 'skipped';
    if (!postComment) return { issueNumber: number, status, label: label.name, commentId: null, claimed, board };
    const lines = statusDetail(detail);
    const body = `${STATUS_HEADINGS[status]} for issue #${number}.` +
      (lines.length ? `\n\n${lines.map((line) => `- ${line}`).join('\n')}` : '') +
      `\n\nStatus label: \`${label.name}\`.`;
    const comment = await request(`${repoPath}/issues/${number}/comments`, token, {
      method: 'POST', body: { body }, fetchImpl,
    });
    if (!Number.isSafeInteger(comment?.id) || comment.id <= 0) {
      throw new Error('GitHub did not confirm the issue status comment');
    }
    return { issueNumber: number, status, label: label.name, commentId: comment.id, claimed, board };
  });
}

const PROJECT_COLUMNS = Object.freeze({ 'in-progress': 'in progress', review: 'in review' });

async function graphql(token, query, variables, fetchImpl) {
  const body = await request('/graphql', token, { method: 'POST', body: { query, variables }, fetchImpl });
  if (!body || body.errors?.length) throw new Error('GitHub project request failed');
  return body.data;
}

async function syncProjectStatus({ token, owner, repository, number, status, fetchImpl }) {
  const column = PROJECT_COLUMNS[status];
  if (!column) return 'unchanged';
  const data = await graphql(token, 'query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name)' +
    '{issue(number:$number){projectItems(first:20){nodes{id project{id field(name:"Status")' +
    '{... on ProjectV2SingleSelectField{id options{id name}}}}}}}}}', { owner, name: repository, number }, fetchImpl);
  let moved = 0;
  for (const item of data?.repository?.issue?.projectItems?.nodes ?? []) {
    const field = item?.project?.field;
    const option = field?.options?.find((entry) => String(entry?.name).toLowerCase() === column);
    if (!item?.id || !item.project?.id || !field?.id || !option?.id) continue;
    await graphql(token, 'mutation($project:ID!,$item:ID!,$field:ID!,$option:String!){updateProjectV2ItemFieldValue(' +
      'input:{projectId:$project,itemId:$item,fieldId:$field,value:{singleSelectOptionId:$option}}){projectV2Item{id}}}',
    { project: item.project.id, item: item.id, field: field.id, option: option.id }, fetchImpl);
    moved += 1;
  }
  return moved ? 'moved' : 'none';
}
