import { createHash } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { ensureLocalPath } from '../lib/paths.mjs';
import { isForbiddenRead } from './tools.mjs';

const sha256 = (value) => createHash('sha256').update(value).digest('hex');
const digestPattern = /^[0-9a-f]{64}$/;
const observationKeys = 'actual_exit,actual_output,check_id,evidence,expected_exit,expected_output,id';

async function fileDigest(worktree, name, optional = false) {
  const file = path.join(worktree, name);
  await ensureLocalPath(file, worktree);
  let entry;
  try {
    entry = await fs.lstat(file);
  } catch (error) {
    if (optional && error.code === 'ENOENT') return null;
    throw error;
  }
  if (!entry.isFile() || entry.isSymbolicLink() || entry.nlink !== 1 || entry.size > 65_536) {
    throw new Error(`Independent verification requires ${name} to be a single-link regular file of at most 64 KiB`);
  }
  const bytes = await fs.readFile(file);
  if (bytes.length > 65_536) throw new Error(`Independent verification file ${name} exceeds 64 KiB`);
  return sha256(bytes);
}

export async function captureVerificationSubject({ worktree, files } = {}) {
  if (typeof worktree !== 'string' || !Array.isArray(files) || !files.length || files.length > 64) {
    throw new TypeError('Independent verification requires a worktree and 1-64 exact product file paths');
  }
  const names = files.map((name) => {
    if (typeof name !== 'string' || !name || name.length > 500 || /[\x00-\x1f\x7f*?]/.test(name) ||
        path.win32.isAbsolute(name) || path.posix.isAbsolute(name)) {
      throw new TypeError('Independent verification file paths must be exact relative paths');
    }
    const normalized = name.replaceAll('\\', '/');
    if (normalized.split('/').some((part) => !part || part === '.' || part === '..') ||
        ['TASK.md', 'RESULT.md', 'REVIEW.md'].includes(normalized) || isForbiddenRead(normalized)) {
      throw new Error('Independent verification file paths must name readable product files');
    }
    return normalized;
  }).sort();
  if (new Set(names).size !== names.length) throw new Error('Independent verification file paths must be unique');
  const subject = {
    task_digest: await fileDigest(worktree, 'TASK.md'),
    result_digest: await fileDigest(worktree, 'RESULT.md'),
    files: await Promise.all(names.map(async (name) => ({ path: name,
      digest: await fileDigest(worktree, name, true) }))),
  };
  return { ...subject, digest: sha256(JSON.stringify(subject)) };
}

function output(value) {
  return typeof value === 'string' && value.length <= 2000 && !value.includes('\0');
}

function exit(value) {
  return Number.isSafeInteger(value) && value >= 0 && value <= 255;
}

// Receipts are supplied by a trusted host, never discovered from model output or worktree files.
export function judgeVerificationReceipt(receipt, { subjectDigest, checkCount } = {}) {
  if (!digestPattern.test(subjectDigest ?? '') || !Number.isSafeInteger(checkCount) ||
      checkCount < 1 || checkCount > 16) {
    throw new TypeError('Independent verification requires a subject digest and 1-16 acceptance checks');
  }
  if (!receipt || typeof receipt !== 'object' || Array.isArray(receipt) ||
      Object.keys(receipt).sort().join(',') !== 'observations,schema_version,subject_digest' ||
      receipt.schema_version !== 1 || receipt.subject_digest !== subjectDigest ||
      !Array.isArray(receipt.observations) || !receipt.observations.length || receipt.observations.length > 64) {
    throw new Error('Independent verification receipt is missing, malformed, or bound to different product bytes');
  }
  const ids = new Set();
  const checks = new Set();
  const observations = receipt.observations.map((entry) => {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry) ||
        Object.keys(entry).sort().join(',') !== observationKeys ||
        !Number.isSafeInteger(entry.id) || entry.id < 1 || entry.id > 64 || ids.has(entry.id) ||
        !Number.isSafeInteger(entry.check_id) || entry.check_id < 1 || entry.check_id > checkCount ||
        !output(entry.expected_output) || !output(entry.actual_output) ||
        !exit(entry.expected_exit) || !exit(entry.actual_exit) ||
        typeof entry.evidence !== 'string' || !entry.evidence.trim() ||
        entry.evidence !== entry.evidence.trim() || entry.evidence.length > 500 ||
        /[\x00-\x1f\x7f]/.test(entry.evidence)) {
      throw new Error('Independent verification observations require unique IDs, valid checks, bounded outputs, exits and evidence');
    }
    ids.add(entry.id);
    checks.add(entry.check_id);
    return { ...entry, met: entry.expected_output === entry.actual_output && entry.expected_exit === entry.actual_exit };
  });
  if (checks.size !== checkCount) {
    throw new Error('Independent verification receipt must cover every acceptance check');
  }
  const failures = observations.filter((entry) => !entry.met);
  return {
    status: failures.length ? 'fail' : 'pass', subjectDigest, observations,
    reasons: failures.map(({ id, check_id, evidence }) =>
      `Independent verification observation ${id}, check ${check_id} failed: ${evidence}`),
  };
}
