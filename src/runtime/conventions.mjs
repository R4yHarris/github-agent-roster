import { execFile } from 'node:child_process';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';
import { isForbiddenRead, isManagedFile } from './tools.mjs';

const SKIP_DIRS = new Set(['node_modules', 'vendor', 'dist', 'build', 'coverage', 'out', 'target']);
const SOURCE = /\.(?:[cm]?[jt]sx?|py|go|rs|rb|java|cs)$/;
const TEST = /(?:^|\/)(?:__tests__\/|tests?\/)|\.(?:test|spec)\.[cm]?[jt]sx?$|(?:^|\/)test_[^/]+\.py$/;
const LANGUAGES = [[/\.[cm]?js$|\.jsx$/, 'JavaScript'], [/\.tsx?$|\.[cm]ts$/, 'TypeScript'], [/\.py$/, 'Python'],
  [/\.go$/, 'Go'], [/\.rs$/, 'Rust'], [/\.rb$/, 'Ruby'], [/\.java$/, 'Java'], [/\.cs$/, 'C#']];
const LINT = [[/^(?:\.eslintrc(?:\.\w+)?|eslint\.config\.[cm]?[jt]s)$/, 'ESLint'],
  [/^(?:\.prettierrc(?:\.\w+)?|prettier\.config\.[cm]?js)$/, 'Prettier'], [/^biome\.jsonc?$/, 'Biome'],
  [/^\.editorconfig$/, 'EditorConfig'], [/^(?:ruff|\.ruff)\.toml$/, 'Ruff']];
const MAX_FILES = 2000;
const MAX_READ = 256 * 1024;
const cache = new Map();

async function walk(root) {
  const files = [];
  async function visit(directory) {
    let entries;
    try { entries = await fs.readdir(path.join(root, directory), { withFileTypes: true }); } catch { return; }
    for (const entry of entries.sort((left, right) => left.name.localeCompare(right.name))) {
      if (files.length >= MAX_FILES) return;
      const file = directory ? `${directory}/${entry.name}` : entry.name;
      if (entry.isSymbolicLink() || entry.name.startsWith('.') && entry.isDirectory()) continue;
      if (isForbiddenRead(file) || isManagedFile(file)) continue;
      if (entry.isDirectory()) { if (!SKIP_DIRS.has(entry.name)) await visit(file); }
      else if (entry.isFile()) files.push(file);
    }
  }
  await visit('');
  return files;
}

async function readText(root, file) {
  try {
    const stat = await fs.lstat(path.join(root, file));
    if (!stat.isFile() || stat.size > MAX_READ) return null;
    return (await fs.readFile(path.join(root, file), 'utf8')).replace(/\r\n/g, '\n');
  } catch { return null; }
}

function caseOf(name) {
  if (/^[a-z0-9]+(?:-[a-z0-9]+)+$/.test(name)) return 'kebab-case';
  if (/^[a-z0-9]+(?:_[a-z0-9]+)+$/.test(name)) return 'snake_case';
  if (/^[A-Z][a-zA-Z0-9]*$/.test(name)) return 'PascalCase';
  if (/^[a-z][a-z0-9]*[A-Z][a-zA-Z0-9]*$/.test(name)) return 'camelCase';
  if (/^[a-z0-9]+$/.test(name)) return 'lowercase';
  return null;
}

function dominant(names, single) {
  const counts = new Map();
  for (const name of names) {
    const kind = caseOf(name);
    if (kind) counts.set(kind, (counts.get(kind) ?? 0) + 1);
  }
  const lowercase = counts.get('lowercase') ?? 0;
  counts.delete('lowercase');
  const [best] = [...counts].sort((left, right) => right[1] - left[1] || left[0].localeCompare(right[0]));
  if (!best) return lowercase ? 'lowercase' : null;
  return single && lowercase ? `${best[0]} (single words lowercase)` : best[0];
}

function lineCount(text) { return text.endsWith('\n') ? text.split('\n').length - 1 : text.split('\n').length; }

async function revision(root) {
  try {
    const { stdout } = await promisify(execFile)('git', ['rev-parse', 'HEAD'], { cwd: root, windowsHide: true, timeout: 5000 });
    return stdout.trim() || null;
  } catch { return null; }
}

// Deterministic, secret-free repo conventions: names, counts, and tool identities only, never file bodies.
export async function deriveConventions(worktree, { useCache = true } = {}) {
  const root = path.resolve(worktree);
  const rev = useCache ? await revision(root) : null;
  const cached = rev && cache.get(root);
  if (cached && cached.rev === rev) return cached.value;
  const files = await walk(root);
  const sources = files.filter((file) => SOURCE.test(file) && !file.endsWith('.d.ts'));
  const tests = sources.filter((file) => TEST.test(file));
  const modules = sources.filter((file) => !TEST.test(file));
  const languageCounts = new Map();
  for (const file of sources) {
    const language = LANGUAGES.find(([pattern]) => pattern.test(file))?.[1];
    if (language) languageCounts.set(language, (languageCounts.get(language) ?? 0) + 1);
  }
  const languages = [...languageCounts].sort((left, right) => right[1] - left[1]).map(([name]) => name);
  let pkg = null;
  try { pkg = files.includes('package.json') ? JSON.parse(await readText(root, 'package.json') ?? 'null') : null; } catch { pkg = null; }
  const deps = Object.keys(pkg?.dependencies ?? {});
  const devDeps = Object.keys(pkg?.devDependencies ?? {});
  const extensionCounts = new Map();
  for (const file of modules) {
    const extension = path.posix.extname(file);
    extensionCounts.set(extension, (extensionCounts.get(extension) ?? 0) + 1);
  }
  const [extension] = [...extensionCounts].sort((left, right) => right[1] - left[1])[0] ?? [];
  const texts = new Map();
  for (const file of sources.slice(0, 600)) {
    const text = await readText(root, file);
    if (text !== null) texts.set(file, text);
  }
  let esm = 0;
  let cjs = 0;
  for (const [file, text] of texts) {
    if (!/\.[cm]?[jt]sx?$/.test(file)) continue;
    if (/^\s*(?:import\s[^(]|export\s)/m.test(text)) esm += 1;
    if (/\brequire\(\s*['"]|module\.exports\b|\bexports\.\w+\s*=/.test(text)) cjs += 1;
  }
  const js = languages.includes('JavaScript') || languages.includes('TypeScript');
  const moduleSystem = !js ? null : pkg?.type === 'module' || extension === '.mjs' || esm > cjs
    ? 'ES modules (import/export)' : cjs ? 'CommonJS (require/module.exports)' : null;
  const lockfiles = [['package-lock.json', 'npm'], ['pnpm-lock.yaml', 'pnpm'], ['yarn.lock', 'yarn'],
    ['bun.lockb', 'bun'], ['poetry.lock', 'poetry'], ['uv.lock', 'uv'], ['Cargo.lock', 'cargo'], ['go.sum', 'go']];
  const manager = lockfiles.find(([lock]) => files.includes(lock))?.[1] ?? (pkg ? 'npm (no lockfile)' : null);
  const script = String(pkg?.scripts?.test ?? '');
  const runner = /\bvitest\b/.test(script) || devDeps.includes('vitest') ? 'vitest'
    : /\bjest\b/.test(script) || devDeps.includes('jest') || files.some((file) => /^jest\.config\./.test(file)) ? 'jest'
      : /\bmocha\b/.test(script) || devDeps.includes('mocha') ? 'mocha'
        : /node\s+--test\b/.test(script) || [...texts.values()].some((text) => /from\s+['"]node:test['"]/.test(text)) ? 'node:test'
          : /\bpytest\b/.test(script) || files.some((file) => /^(?:pytest\.ini|conftest\.py)$/.test(file)) ? 'pytest' : null;
  const testDirs = new Map();
  for (const file of tests) {
    const directory = file.includes('/') ? `${file.split('/')[0]}/` : '(root)';
    testDirs.set(directory, (testDirs.get(directory) ?? 0) + 1);
  }
  const [testDir] = [...testDirs].sort((left, right) => right[1] - left[1])[0] ?? [];
  const testSuffix = (() => {
    const counts = new Map();
    for (const file of tests) {
      const match = path.posix.basename(file).match(/\.((?:test|spec)\.[cm]?[jt]sx?)$/);
      if (match) counts.set(match[1], (counts.get(match[1]) ?? 0) + 1);
    }
    return [...counts].sort((left, right) => right[1] - left[1])[0]?.[0] ?? null;
  })();
  const sharded = tests.filter((file) => /\.[^./]+\.(?:test|spec)\.[cm]?[jt]sx?$/.test(path.posix.basename(file))).length;
  const stem = (file) => path.posix.basename(file).replace(/(?:\.(?:test|spec))?\.[^.]+$/, '').split('.')[0];
  const fileNaming = dominant(modules.map(stem), true);
  const exportNames = [];
  for (const [file, text] of texts) {
    if (TEST.test(file)) continue;
    for (const match of text.matchAll(/^export\s+(?:default\s+)?(?:async\s+)?(?:function\*?|const|let|class)\s+([A-Za-z_$][\w$]*)/gm)) {
      exportNames.push(match[1]);
    }
    for (const match of text.matchAll(/\bexports\.([A-Za-z_$][\w$]*)\s*=/g)) exportNames.push(match[1]);
    const object = text.match(/module\.exports\s*=\s*\{([^}]*)\}/);
    if (object) for (const name of object[1].split(',')) {
      const key = name.split(':')[0].trim();
      if (/^[A-Za-z_$][\w$]*$/.test(key)) exportNames.push(key);
    }
  }
  const exportNaming = dominant(exportNames.filter((name) => !/^[A-Z][A-Z0-9_]+$/.test(name)), false);
  const lint = [...new Set(files.filter((file) => !file.includes('/'))
    .flatMap((file) => LINT.filter(([pattern]) => pattern.test(file)).map(([, name]) => name)))];
  if (pkg?.eslintConfig && !lint.includes('ESLint')) lint.push('ESLint');
  if (pkg?.prettier && !lint.includes('Prettier')) lint.push('Prettier');
  const largest = (list) => list.map((file) => ({ file, lines: texts.has(file) ? lineCount(texts.get(file)) : 0 }))
    .sort((left, right) => right.lines - left.lines || left.file.localeCompare(right.file))[0] ?? null;
  const imports = new Map();
  const known = new Set(modules);
  for (const [file, text] of texts) {
    if (TEST.test(file)) continue;
    for (const match of text.matchAll(/(?:\bfrom\s+|\brequire\(\s*|\bimport\(\s*)['"](\.{1,2}\/[^'"]+)['"]/g)) {
      const base = path.posix.normalize(path.posix.join(path.posix.dirname(file), match[1]));
      const target = [base, `${base}.js`, `${base}.mjs`, `${base}.cjs`, `${base}.ts`, `${base}/index.js`]
        .find((candidate) => known.has(candidate));
      if (target && target !== file) imports.set(target, (imports.get(target) ?? 0) + 1);
    }
  }
  const core = [...imports].sort((left, right) => right[1] - left[1] || left[0].localeCompare(right[0]))
    .slice(0, 5).map(([file, count]) => ({ file, importers: count }));
  const value = {
    languages, moduleSystem, sourceExtension: extension ?? null, packageManager: manager,
    runtimeDependencies: deps.length, testRunner: runner, testDir: testDir ?? null, testSuffix, shardedTests: sharded,
    testCount: tests.length, fileNaming, exportNaming, lint, largestModule: largest(modules), largestTest: largest(tests), core,
  };
  if (rev) cache.set(root, { rev, value });
  return value;
}

export function conventionsText(conventions) {
  const c = conventions;
  const rows = [];
  if (c.languages.length) rows.push(`- Language: ${c.languages.join(', ')}${c.moduleSystem ? `; ${c.moduleSystem}` : ''}` +
    `${c.sourceExtension ? `; source files use \`${c.sourceExtension}\`` : ''}.`);
  if (c.packageManager) rows.push(`- Packages: ${c.packageManager}; ${c.runtimeDependencies ? `${c.runtimeDependencies} runtime ` +
    'dependencies' : 'no runtime dependencies (do not add one)'}.`);
  if (c.testRunner || c.testCount) {
    const layout = c.testDir ? ` under \`${c.testDir}\`` : '';
    const shape = c.testSuffix ? `, named \`<module>.${c.testSuffix}\`${c.shardedTests ? ` or sharded \`<module>.<topic>.${c.testSuffix}\`` : ''}` : '';
    rows.push(`- Tests: ${c.testRunner ?? 'unknown runner'}; ${c.testCount} files${layout}${shape}. Use the same runner and layout.`);
  }
  if (c.fileNaming || c.exportNaming) rows.push(`- Naming: ${c.fileNaming ? `files ${c.fileNaming}` : ''}` +
    `${c.fileNaming && c.exportNaming ? '; ' : ''}${c.exportNaming ? `exports ${c.exportNaming}` : ''}.`);
  rows.push(`- Lint/format config: ${c.lint.length ? c.lint.join(', ') : 'none; match the surrounding style'}.`);
  if (c.largestModule?.lines) rows.push(`- Size: largest module \`${c.largestModule.file}\` is ${c.largestModule.lines} lines` +
    `${c.largestTest?.lines ? `; largest test \`${c.largestTest.file}\` is ${c.largestTest.lines} lines` : ''}. ` +
    'Keep files below these; add a new focused module or test shard instead of growing one.');
  if (c.core.length) rows.push(`- Core modules (most imported; read before adding parallel code): ${
    c.core.map(({ file, importers }) => `\`${file}\` (${importers})`).join(', ')}.`);
  return rows.join('\n');
}

export const RULE_LAYERS = [
  '1. Human policy and protected surfaces: enforced by tools and gates, not negotiable in prose.',
  '2. Org rules: the contracts SDK, identity, and trailers; applied by the harness at publication.',
  '3. Repo rules: the AGENTS.md coding sections below.',
  '4. Area rules: conventions derived from this repository (below) and notes in the modules you read.',
  '5. Task rules: TASK.md checks and allowed files.',
  '6. Seat principal: professional conduct for this seat.',
].join('\n') + '\nA higher layer wins on conflict; a lower layer may narrow, never widen, a higher one.';
