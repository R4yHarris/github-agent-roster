import { promises as fs } from 'node:fs';
import path from 'node:path';
import { ensureLocalPath } from '../lib/paths.mjs';

export async function readMemory({ file, repoRoot, limit = 20 }) {
  await ensureLocalPath(file, repoRoot);
  let contents;
  try {
    contents = await fs.readFile(file, 'utf8');
  } catch (error) {
    if (error.code === 'ENOENT') return [];
    throw error;
  }
  const lines = contents.split(/\r?\n/);
  if (lines.at(-1) === '') lines.pop();
  const last = lines.slice(-limit);
  for (const [index, line] of last.entries()) {
    try {
      JSON.parse(line);
    } catch {
      throw new Error(`Invalid memory JSONL line ${lines.length - last.length + index + 1}`);
    }
  }
  return last;
}

export async function appendMemory({ file, repoRoot, record }) {
  await ensureLocalPath(file, repoRoot);
  if (!record || typeof record !== 'object' || Array.isArray(record)) {
    throw new TypeError('Memory record must be a JSON object');
  }
  const content = JSON.stringify(record);
  if (!content || content.includes('\n')) throw new TypeError('Memory record must be a JSON object');
  await fs.mkdir(path.dirname(file), { recursive: true });
  await ensureLocalPath(file, repoRoot);
  await fs.appendFile(file, `${content}\n`, { encoding: 'utf8', mode: 0o600 });
}
