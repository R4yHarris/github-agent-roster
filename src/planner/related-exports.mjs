import { promises as fs } from 'node:fs';
import path from 'node:path';

const sourcePattern = /^src\/.+\.(?:mjs|cjs|js|ts)$/;
const exportPattern = /^export\s+(?:default\s+)?(?:async\s+)?(?:function\*?|const|let|class)\s+([A-Za-z_$][\w$]*)/gm;
const stopwords = new Set(['the', 'and', 'for', 'with', 'from', 'into', 'that', 'this', 'when', 'than', 'then',
  'each', 'every', 'must', 'never', 'only', 'without', 'under', 'over', 'their', 'there', 'issue', 'issues',
  'feature', 'add', 'adds', 'make', 'keep', 'use', 'uses', 'used', 'not', 'are', 'can', 'should', 'will', 'any',
  'all', 'new', 'test', 'tests', 'file', 'files', 'docs', 'src', 'lib', 'mjs']);
const stemLength = 5;
const fileLimit = 400;
const byteLimit = 256 * 1024;

const words = (text) => String(text ?? '')
  .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
  .toLowerCase()
  .split(/[^a-z0-9]+/)
  .filter((word) => word.length >= 4 && !stopwords.has(word));
const stems = (text) => new Set(words(text).map((word) => word.slice(0, stemLength)));

// Spec 5.3 planning: a feature planner sees which existing modules already own the concepts in the Ask,
// so child drafts extend them instead of inventing parallel modules, roots, or hardcoded paths.
export async function relatedExports(worktree, repositoryFiles, askText, {
  limit = 12, exportsPerFile = 16, readFile = (file) => fs.readFile(file, 'utf8'),
} = {}) {
  if (!Array.isArray(repositoryFiles)) return [];
  const terms = stems(askText);
  if (!terms.size) return [];
  const scored = [];
  for (const file of repositoryFiles.filter((entry) => sourcePattern.test(entry)).slice(0, fileLimit)) {
    let source;
    try {
      source = await readFile(path.join(worktree, file));
    } catch {
      continue;
    }
    if (typeof source !== 'string' || source.length > byteLimit) continue;
    const names = [...new Set([...source.matchAll(exportPattern)].map((match) => match[1]))];
    if (!names.length) continue;
    const fileHits = [...stems(file)].filter((stem) => terms.has(stem)).length;
    const ranked = names.map((name) => ({ name, hits: [...stems(name)].filter((stem) => terms.has(stem)).length }));
    const exportHits = ranked.reduce((sum, { hits }) => sum + Math.min(hits, 2), 0);
    const score = fileHits * 2 + exportHits;
    if (score < 2) continue;
    const exports = ranked.filter(({ hits }) => hits > 0).sort((left, right) => right.hits - left.hits)
      .map(({ name }) => name).slice(0, exportsPerFile);
    scored.push({ file, exports: exports.length ? exports : names.slice(0, exportsPerFile), score });
  }
  return scored.sort((left, right) => right.score - left.score || left.file.localeCompare(right.file))
    .slice(0, limit).map(({ file, exports }) => ({ file, exports }));
}
