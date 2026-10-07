// Command parsing and rendering for the durable, read-only history reader.
import { execFile } from 'node:child_process';
import { stat } from 'node:fs/promises';
import path from 'node:path';
import { promisify } from 'node:util';
import { createHistoryReader, filterRecords, filterRepositories, findHistoryRecord, historyFields } from './history-query.mjs';

const execute = promisify(execFile);
const USAGE = 'Use roster history list [--store DIR] [--repo HASH] [--issue N] [--seat NAME] [--model MODEL] [--outcome VALUE] [--since TIME] [--until TIME] or roster history show <session-or-run-or-record-id> [--store DIR].';

// The common directory is .git in the main checkout, including from a linked
// worktree or a nested cwd. No origin/network access or repository state scan.
export async function resolveHistoryRoot({ cwd = process.cwd(), storePath, run = execute } = {}) {
  if (storePath !== undefined) return path.resolve(cwd, storePath);
  const { stdout } = await run('git', ['--no-pager', 'rev-parse', '--git-common-dir'], {
    cwd, encoding: 'utf8', timeout: 10_000,
  });
  if (!stdout.trim()) throw new Error('Could not resolve the repository git common directory.');
  return path.join(path.resolve(cwd, stdout.trim()), 'roster', 'provenance');
}

export async function loadProvenanceRecords(options = {}) {
  const root = await resolveHistoryRoot(options);
  let missing = false;
  try {
    if (!(await stat(root)).isDirectory()) throw new TypeError('History store root must be a directory.');
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
    missing = true;
  }
  return { ...(await createHistoryReader({ root }).read()), root, missing };
}

function renderRecord(record) {
  const fields = historyFields(record);
  return [
    `session=${record.sessionId ?? '-'}`, `run=${record.runId ?? '-'}`,
    `id=${record.id ?? '-'}`, `repo=${fields.repository ?? '-'}`,
    `issue=${fields.issue ?? '-'}`, `seat=${fields.seat ?? '-'}`,
    `model=${fields.model ?? '-'}`, `outcome=${fields.outcome ?? '-'}`,
    `at=${fields.at ?? '-'}`,
  ].join('  ');
}

export function formatList(records) {
  return records.length ? `${records.map(renderRecord).join('\n')}\n` : 'No history records matched.\n';
}

export function formatShow(record) { return `${renderRecord(record)}\n`; }

const FLAG_MAP = { '--repo': 'repository', '--issue': 'issue', '--seat': 'seat', '--model': 'model', '--outcome': 'outcome', '--since': 'since', '--until': 'until', '--store': 'storePath' };

function parseFlags(flags, { show = false } = {}) {
  const options = {};
  for (let index = 0; index < flags.length; index += 2) {
    const flag = flags[index];
    const key = FLAG_MAP[flag];
    const value = flags[index + 1];
    if (!key || (show && key !== 'storePath') || Object.hasOwn(options, key) ||
        !value || value.startsWith('--')) throw new TypeError(USAGE);
    options[key] = value;
  }
  return options;
}

export function parseListFlags(flags) { return parseFlags(flags); }

export async function runHistory(argv, options = {}) {
  const [action, ...rest] = argv;
  if (!['list', 'show'].includes(action)) throw new TypeError(USAGE);
  const id = action === 'show' ? rest.shift() : undefined;
  if (action === 'show' && (!id || id.startsWith('--'))) throw new TypeError(USAGE);
  const { storePath, ...filters } = parseFlags(rest, { show: action === 'show' });
  // Validate filters before accessing git or the store.
  filterRecords([], filters);
  const { records, skipped, root, missing } = await loadProvenanceRecords({
    ...options, ...(storePath === undefined ? {} : { storePath }),
  });
  const warning = skipped.length ? `Skipped ${skipped.length} unreadable or incompatible history record(s).\n` : '';
  if (action === 'show') return warning + formatShow(findHistoryRecord(records, id));
  if (missing) return `No history store found at ${root}.\n`;
  return warning + formatList(filterRecords(records, filters));
}

export { filterRecords, filterRepositories };
