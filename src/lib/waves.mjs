import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';
import { githubRepository, renderIssueBody } from './issue.mjs';
import { ensureLocalPath } from './paths.mjs';
import { parsePlanDocument } from '../planner/plan-document.mjs';
import { redactEvidence } from '../runtime/excellence.mjs';
import { issueWave } from './wave-labels.mjs';
export { issueWave } from './wave-labels.mjs';

const execute = promisify(execFile);

async function run(program, args, cwd, env) {
  try { return (await execute(program, args, { cwd, env: { ...env, GH_PROMPT_DISABLED: '1' },
    encoding: 'utf8', timeout: 30000, maxBuffer: 2 * 1024 * 1024 })).stdout; }
  catch (error) { throw new Error('GitHub wave operation failed; inspect access before retrying.', { cause: error }); }
}

const labels = (issue) => (issue.labels ?? []).map((label) => typeof label === 'string' ? label : label?.name);
// Accepts the plain board label and the label a Roster run sets through the App (e.g. roster:review).
const hasLabel = (issue, name) => labels(issue ?? {}).some((label) => label === name || label === `roster:${name}`);

export async function requireEarlierWavesClosed({ issue, repository, cwd, runCommand }) {
  const wave = issueWave(issue);
  if (wave === null || wave === 1) return;
  const key = /<!-- Roster-Plan: ([a-f0-9]{64}) -->/.exec(issue.body ?? '')?.[1];
  for (let earlier = 1; earlier < wave; earlier += 1) {
    const args = ['issue', 'list', '--repo', repository, '--state', 'open', '--label', `wave:${earlier}`,
      '--limit', '1', '--json', 'number,title'];
    if (key) args.push('--search', `in:body "Roster-Plan: ${key}"`);
    let open;
    try { open = JSON.parse(await runCommand('gh', args, cwd)); }
    catch (error) { throw new Error('Earlier-wave status is unavailable; later wave is blocked.', { cause: error }); }
    if (!Array.isArray(open) || open.length > 1 || open.some((item) => !Number.isSafeInteger(item?.number) || item.number < 1)) {
      throw new Error('Earlier-wave metadata is invalid; later wave is blocked.');
    }
    if (open.length) throw new Error(`Wave ${wave} is blocked while wave ${earlier} issue #${open[0].number} is open.`);
  }
}

// Code files that closed earlier-wave slices of the same plan merged, so a later slice reuses them.
export async function earlierWaveFiles({ issue, repository, worktree, cwd, runCommand, limit = 12 }) {
  const wave = issueWave(issue);
  const key = /<!-- Roster-Plan: ([a-f0-9]{64}) -->/.exec(issue.body ?? '')?.[1];
  if (wave === null || wave === 1 || !key) return [];
  let closed;
  try {
    closed = JSON.parse(await runCommand('gh', ['issue', 'list', '--repo', repository, '--state', 'closed',
      '--search', `in:body "Roster-Plan: ${key}"`, '--limit', '100', '--json', 'number,body,labels'], cwd));
  } catch { return []; }
  if (!Array.isArray(closed)) return [];
  const numbers = closed.filter((item) => Number.isSafeInteger(item?.number) && item.number !== issue.number &&
    (issueWave(item) ?? wave) < wave).map(({ number }) => number).sort((a, b) => a - b);
  const files = new Set();
  for (const number of numbers) {
    let output;
    try {
      output = await runCommand('git', ['log', 'HEAD', '-i', '-E',
        `--grep=(close[sd]?|fix(e[sd])?|resolve[sd]?) #${number}([^0-9]|$)`,
        '--diff-filter=AM', '--name-only', '--format='], worktree);
    } catch { continue; }
    for (const line of String(output).split('\n')) {
      const file = line.trim().replaceAll('\\', '/');
      if (/\.[cm]?js$/.test(file) && !/(^|\/)tests?\//.test(file) && !/\.(test|spec)\.[cm]?js$/.test(file)) files.add(file);
    }
  }
  return [...files].sort().slice(0, limit);
}

export async function readWavePlan(worktree) {
  const file = path.join(worktree, 'PLAN.md');
  await ensureLocalPath(file, worktree);
  const entry = await fs.lstat(file);
  if (!entry.isFile() || entry.isSymbolicLink() || entry.nlink !== 1 || entry.size > 65536) {
    throw new Error('Wave plan must be a bounded regular single-link file');
  }
  const source = await fs.readFile(file, 'utf8');
  return { source, key: createHash('sha256').update(source.replaceAll('\r\n', '\n')).digest('hex'),
    plan: parsePlanDocument(source) };
}

async function linkedIssues(repository, key, command, cwd) {
  let issues;
  try { issues = JSON.parse(await command('gh', ['issue', 'list', '--repo', repository, '--state', 'all',
    '--search', `in:body "Roster-Plan: ${key}"`, '--limit', '100', '--json', 'number,title,state,body,labels'], cwd)); }
  catch (error) { throw new Error('GitHub wave metadata is unavailable.', { cause: error }); }
  if (!Array.isArray(issues) || issues.length === 100 || issues.some((issue) =>
    !Number.isSafeInteger(issue?.number) || issue.number < 1 || !['OPEN', 'CLOSED'].includes(issue.state) ||
    typeof issue.body !== 'string' || issue.labels !== undefined && !Array.isArray(issue.labels))) {
    throw new Error('GitHub wave metadata is invalid or exceeds the bounded lookup');
  }
  return issues.filter((issue) => issue.body.includes(`<!-- Roster-Plan: ${key} -->`));
}

// A lost PLAN.md changes the plan key; opening a second wave set would duplicate the parent's open children.
async function refuseOrphanWaves(repository, parent, key, command, cwd) {
  let issues;
  try { issues = JSON.parse(await command('gh', ['issue', 'list', '--repo', repository, '--state', 'open',
    '--search', `in:body "Parent: #${parent}"`, '--limit', '100', '--json', 'number,body'], cwd)); }
  catch (error) { throw new Error('GitHub wave metadata is unavailable.', { cause: error }); }
  if (!Array.isArray(issues)) throw new Error('GitHub wave metadata is invalid or exceeds the bounded lookup');
  const parentLine = new RegExp(`^Parent: #${parent}$`, 'm');
  const orphans = issues.filter((issue) => typeof issue?.body === 'string' && parentLine.test(issue.body) &&
    /<!-- Roster-Plan: [0-9a-f]{64} -->/.test(issue.body) && !issue.body.includes(`<!-- Roster-Plan: ${key} -->`))
    .map((issue) => `#${issue.number}`);
  if (orphans.length) {
    throw new Error(`Parent #${parent} already has open wave children from another plan (${orphans.join(', ')}); ` +
      'run those children directly or close them before opening a new wave set.');
  }
}

export async function waveBoard({
  worktree, cwd, env = process.env, apiKeyEnv = 'ROSTER_API_KEY', open = false, activeIssue, activeState,
  runCommand = (program, args, root) => run(program, args, root, env),
}) {
  if (open && env.ROSTER_SEAT) throw new Error('Opening wave drafts requires an explicit human command, not an agent seat.');
  const { source, key, plan } = await readWavePlan(worktree);
  if (redactEvidence(source, { env, apiKeyEnv }) !== source) throw new Error('Wave plans must not contain secret material');
  const repository = githubRepository((await runCommand('git', ['remote', 'get-url', 'origin'], cwd)).trim());
  const existing = await linkedIssues(repository, key, runCommand, cwd);
  const matched = new Map();
  for (const issue of existing) {
    const draft = Number(/<!-- Roster-Draft: ([1-9]\d*) -->/.exec(issue.body)?.[1]);
    if (!Number.isSafeInteger(draft) || !plan.issues[draft - 1] || matched.has(draft) ||
        issueWave(issue) !== plan.issues[draft - 1].wave) throw new Error('Linked GitHub drafts conflict with PLAN.md');
    matched.set(draft, issue);
  }
  if (open && matched.size === 0 && /^issue:[1-9]\d*$/.test(plan.reference)) {
    await refuseOrphanWaves(repository, plan.reference.slice('issue:'.length), key, runCommand, cwd);
  }
  if (open) {
    let known;
    try { known = JSON.parse(await runCommand('gh', ['label', 'list', '--repo', repository, '--limit', '1000', '--json', 'name'], cwd)); }
    catch (error) { throw new Error('Wave labels could not be verified.', { cause: error }); }
    if (!Array.isArray(known) || known.some((entry) => typeof entry?.name !== 'string')) throw new Error('Wave label metadata is invalid');
    for (const wave of new Set(plan.issues.map((draft) => draft.wave))) {
      if (!known.some(({ name }) => name === `wave:${wave}`)) await runCommand('gh', ['label', 'create',
        `wave:${wave}`, '--repo', repository, '--color', '1D76DB', '--description', `Roster plan wave ${wave}`], cwd);
    }
    for (const [index, draft] of plan.issues.entries()) {
      if (matched.has(index + 1)) continue;
      const parent = /^issue:[1-9]\d*$/.test(plan.reference) ? `\n\nParent: #${plan.reference.slice('issue:'.length)}` : '';
      // Markers precede the headed lists: a trailing line would be parsed as part of Files allowed.
      const ask = `${draft.title}\n\n${draft.outcome}${parent}\n\n` +
        `<!-- Roster-Plan: ${key} -->\n<!-- Roster-Wave: ${draft.wave} -->\n<!-- Roster-Draft: ${index + 1} -->\n\n` +
        '## Acceptance checks\n' + draft.acceptance_checks.map((check) => `- ${check}`).join('\n') +
        (draft.files_allowed.length ? '\n\n## Files allowed\n' + draft.files_allowed.map((file) => `- \`${file}\``).join('\n') : '');
      const url = (await runCommand('gh', ['issue', 'create', '--repo', repository, '--title', draft.title,
        '--body', renderIssueBody(ask), '--label', `wave:${draft.wave}`], cwd)).trim();
      const prefix = `https://github.com/${repository}/issues/`;
      const number = url.startsWith(prefix) ? Number(url.slice(prefix.length)) : NaN;
      if (!Number.isSafeInteger(number) || number < 1) throw new Error('GitHub did not confirm the created wave draft.');
      matched.set(index + 1, { number, state: 'OPEN', labels: [{ name: `wave:${draft.wave}` }] });
    }
  }
  const rows = [];
  for (const [index, draft] of plan.issues.entries()) {
    const issue = matched.get(index + 1);
    let state = issue?.state === 'CLOSED' ? 'done'
      : [...matched.values()].some((earlier) => issueWave(earlier) < draft.wave && earlier.state === 'OPEN') ||
        hasLabel(issue, 'blocked') ? 'blocked'
      : issue && activeIssue === issue.number && ['planning', 'drafting', 'testing'].includes(activeState) ? 'running'
      : issue && activeIssue === issue.number && ['reviewing', 'passed'].includes(activeState) || hasLabel(issue, 'review')
        ? 'review'
      // Another agent's claim is shared through the board, so it is not offered as the next wave slice.
      : hasLabel(issue, 'in-progress') ? 'running' : 'todo';
    if (issue && state === 'todo') {
      let pulls;
      try { pulls = JSON.parse(await runCommand('gh', ['pr', 'list', '--repo', repository, '--head',
        `issue-${issue.number}`, '--state', 'all', '--limit', '1', '--json', 'number'], cwd)); }
      catch (error) { throw new Error('Wave PR state is unavailable.', { cause: error }); }
      if (!Array.isArray(pulls) || pulls.length > 1 ||
          pulls.some((pull) => !Number.isSafeInteger(pull?.number) || pull.number < 1)) {
        throw new Error('Wave PR metadata is invalid');
      }
      if (pulls.length) state = 'review';
    }
    rows.push({ wave: draft.wave, title: draft.title, state, issue: issue?.number ?? null });
  }
  return rows;
}
