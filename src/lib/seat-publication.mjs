import { loadLearning } from './learn.mjs';
import { materializeRun } from '../metrics/run.mjs';

export function recordedCoderRun({ repoRoot, run }) {
  if (!run?.metrics?.session || !run.metrics.task) throw new TypeError('Publication requires a measured coder session/task');
  const record = loadLearning({ cwd: repoRoot }).runs.findLast((record) =>
    record.session === run.metrics.session && record.task === run.metrics.task);
  if (!record) throw new Error('Coder run is missing from .roster/runs JSONL; publication is refused');
  const measured = materializeRun({ ...record, effort: record.effort ?? '-' }, run.version);
  if (JSON.stringify(measured.metrics) !== JSON.stringify(materializeRun(run.metrics, run.version).metrics)) {
    throw new Error('Recorded coder metrics differ from the reviewed run; rerun before publication');
  }
  return measured;
}

export function humanEvalHint(session) {
  if (typeof session !== 'string' || !/^roster-(?:[1-9]\d*|local-[a-f0-9]{16})-coder$/.test(session)) {
    throw new TypeError('Human eval hint requires an issue coder session or a local ask coder session');
  }
  return `roster eval ${session} accept 1 n --minutes M`;
}
