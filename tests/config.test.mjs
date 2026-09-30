import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync,
  symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { ConfigError, loadConfig, parseConfig, setConfigValue } from '../src/lib/config.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');

function fixture(context) {
  const root = mkdtempSync(join(tmpdir(), 'roster-config-'));
  context.after(() => rmSync(root, { recursive: true, force: true }));
  writeFileSync(join(root, 'roster.config.example.yml'), example);
  return root;
}

test('loads the tracked example when private config is absent', (context) => {
  const repoRoot = fixture(context);
  const config = loadConfig({ repoRoot });
  assert.deepEqual(config, {
    schema: 1,
    llm: {
      base_url: '', model: '', api_key_env: 'ROSTER_API_KEY', effort: 'm', context_max: 0,
      profile: '',
    },
    profiles: {
      'vllm-local': {
        base_url: 'http://127.0.0.1:8000/v1', api_key_env: 'ROSTER_API_KEY', api_key_optional: true,
      },
      ollama: { base_url: 'http://127.0.0.1:11434/v1', api_key_env: 'ROSTER_API_KEY' },
      lmstudio: { base_url: 'http://127.0.0.1:1234/v1', api_key_env: 'ROSTER_API_KEY' },
      openai: { base_url: 'https://api.openai.com/v1', api_key_env: 'OPENAI_API_KEY' },
    },
    planner: { turn_budget: 2 },
    seat: {
      id: 'coder', principal: 'coder', turn_budget: 8, context_chars: 8000,
      tools: ['read_file', 'write_file', 'list_dir', 'run_test', 'search_text'],
    },
    paths: {
      memory: join('.roster', 'memory', 'coder.jsonl'),
      skills: 'skills', asks: join('.roster', 'asks'), worktrees: '.worktrees',
    },
    publish: { enabled: true },
    tools: { internet: false, run_test: true },
    reviewer: { required: true },
  });
  assert.equal(Object.isFrozen(config.seat.tools), true);
  assert.equal(Object.isFrozen(config.llm), true);
  assert.equal(Object.isFrozen(config.planner), true);
  assert.equal(Object.isFrozen(config.profiles), true);
  assert.equal(Object.isFrozen(config.profiles['vllm-local']), true);
  assert.equal(Object.isFrozen(config.profiles.openai), true);
});

test('onboarding vLLM fields accept a custom host without changing the named profile defaults', () => {
  const source = example.replace('base_url: ""', 'base_url: http://172.30.96.1:8000/v1')
    .replace('profile: ""', 'profile: vllm-local')
    .replace('model: ""', 'model: owner/served-model')
    .replace('  effort: m', '  api_key_optional: true\n  provider: vllm\n  effort: m');
  const selected = parseConfig(source);
  assert.equal(selected.llm.base_url, 'http://172.30.96.1:8000/v1');
  assert.equal(selected.llm.provider, 'vllm');
  assert.equal(selected.llm.api_key_optional, true);
  assert.equal(selected.profiles['vllm-local'].base_url, 'http://127.0.0.1:8000/v1');
  const required = parseConfig(source.replace('  api_key_optional: true', '  api_key_optional: false'));
  assert.equal(required.llm.api_key_optional, false);
  assert.throws(() => parseConfig(source.replace('provider: vllm', 'provider: unknown')), ConfigError);
  assert.throws(() => parseConfig(source.replace('  api_key_optional: true', '  api_key_optional: yes')), ConfigError);
});

test('permission fields are validated and legacy configs preserve publishing, tests, and review defaults', () => {
  const selected = parseConfig(example.replace('enabled: true', 'enabled: false')
    .replace('internet: false', 'internet: true').replace('run_test: true', 'run_test: false')
    .replace('required: true', 'required: false'));
  assert.deepEqual(selected.publish, { enabled: false });
  assert.deepEqual(selected.tools, { internet: true, run_test: false });
  assert.deepEqual(selected.reviewer, { required: false });
  const legacy = parseConfig(example.replace(/publish:[\s\S]*$/, ''));
  assert.deepEqual(legacy.publish, { enabled: true });
  assert.deepEqual(legacy.tools, { internet: false, run_test: true });
  assert.deepEqual(legacy.reviewer, { required: true });
  for (const source of [example.replace('enabled: true', 'enabled: yes'),
    example.replace('run_test: true', 'run_test: 0'),
    example.replace('required: true', 'required: maybe')]) {
    assert.throws(() => parseConfig(source), ConfigError);
  }
});

test('canonical review/loop/context fields normalize to existing runtime limits and retain legacy review support', () => {
  const canonical = example.replace(/reviewer:\r?\n  required:[^\r\n]*\r?\n/, '') +
    'review:\n  required: false\nloop:\n  turns: 12\ncontext:\n  budget: 16000\n';
  const config = parseConfig(canonical);
  assert.deepEqual(config.review, { required: false });
  assert.deepEqual(config.reviewer, { required: false });
  assert.equal(config.loop.turns, 12);
  assert.equal(config.seat.turn_budget, 12);
  assert.equal(config.context.budget, 16000);
  assert.equal(config.seat.context_chars, 16000);
  for (const source of [
    canonical.replace('turns: 12', 'turns: 65'),
    canonical.replace('budget: 16000', 'budget: 0'),
    canonical + 'reviewer:\n  required: true\n',
  ]) {
    assert.throws(() => parseConfig(source), ConfigError);
  }
  assert.equal(parseConfig(example).reviewer.required, true);
  assert.equal(parseConfig(example).seat.turn_budget, 8);
});

test('project private settings are loaded and updated without modifying the installation config', async (t) => {
  const repoRoot = fixture(t);
  const project = join(repoRoot, 'project');
  const cwd = join(project, 'nested');
  mkdirSync(cwd, { recursive: true });
  mkdirSync(join(project, '.roster'));
  execFileSync('git', ['init', '--quiet'], { cwd: project, stdio: 'pipe' });
  const file = join(project, '.roster', 'config.yml');
  writeFileSync(file, example.replace('model: ""', 'model: project-model'));
  assert.equal(loadConfig({ repoRoot, cwd }).llm.model, 'project-model');
  await setConfigValue('model', 'updated-model', { repoRoot, cwd });
  assert.equal(loadConfig({ repoRoot, cwd }).llm.model, 'updated-model');
  assert.equal(loadConfig({ repoRoot }).llm.model, '');
  assert.equal(readFileSync(join(repoRoot, 'roster.config.example.yml'), 'utf8'), example);
  writeFileSync(file, 'invalid: config\n');
  assert.throws(() => loadConfig({ repoRoot, cwd }), ConfigError);
});
test('named profiles select endpoints and optional-key settings without secrets', () => {
  for (const [name, url, key, optional] of [
    ['vllm-local', 'http://127.0.0.1:8000/v1', 'ROSTER_API_KEY', true],
    ['ollama', 'http://127.0.0.1:11434/v1', 'ROSTER_API_KEY', undefined],
    ['lmstudio', 'http://127.0.0.1:1234/v1', 'ROSTER_API_KEY', undefined],
    ['openai', 'https://api.openai.com/v1', 'OPENAI_API_KEY', undefined],
  ]) {
    const selected = parseConfig(example.replace('profile: ""', `profile: ${name}`)
      .replace('model: ""', 'model: chosen-model'));
    assert.equal(selected.llm.profile, name);
    assert.equal(selected.llm.base_url, url);
    assert.equal(selected.llm.api_key_env, key);
    assert.equal(selected.llm.api_key_optional, optional);
    assert.equal(selected.llm.model, 'chosen-model');
  }
});

test('vllm-local accepts a served HF model handle and explicit required keys', () => {
  const selected = parseConfig(example.replace('profile: ""', 'profile: vllm-local')
    .replace('model: ""', 'model: owner/served-model')
    .replace('api_key_optional: true', 'api_key_optional: false'));
  assert.equal(selected.llm.model, 'owner/served-model');
  assert.equal(selected.llm.api_key_optional, false);
});

test('an endpoint with an empty model remains valid for explicitly gated auto-model routing', async (context) => {
  const selected = parseConfig(example.replace('profile: ""', 'profile: ollama'));
  assert.equal(selected.llm.model, '');
  assert.equal(selected.llm.base_url, 'http://127.0.0.1:11434/v1');

  const repoRoot = fixture(context);
  mkdirSync(join(repoRoot, '.roster'));
  writeFileSync(join(repoRoot, '.roster', 'config.yml'),
    example.replace('profile: ""', 'profile: ollama').replace('model: ""', 'model: earlier-model'));
  const cleared = await setConfigValue('model', '', { repoRoot });
  assert.equal(cleared.llm.model, '');
  assert.equal(loadConfig({ repoRoot }).llm.base_url, 'http://127.0.0.1:11434/v1');
});

test('private config overrides the example without resolving or logging the API key', (context) => {
  const repoRoot = fixture(context);
  mkdirSync(join(repoRoot, '.roster'));
  writeFileSync(join(repoRoot, '.roster', 'config.yml'),
    example.replace('base_url: ""', 'base_url: "http://localhost:1234/v1"')
      .replace('model: ""', 'model: local-model')
      .replace('turn_budget: 2', 'turn_budget: 4')
      .replace('turn_budget: 8', 'turn_budget: 3'));
  const original = process.env.ROSTER_API_KEY;
  process.env.ROSTER_API_KEY = 'do-not-print-this-secret';
  try {
    const config = loadConfig({ repoRoot });
    assert.equal(config.llm.base_url, 'http://localhost:1234/v1');
    assert.equal(config.planner.turn_budget, 4);
    assert.equal(config.seat.turn_budget, 3);
    assert.ok(!JSON.stringify(config).includes(process.env.ROSTER_API_KEY));
  } finally {
    if (original === undefined) delete process.env.ROSTER_API_KEY;
    else process.env.ROSTER_API_KEY = original;
  }
});

test('model and effort settings persist privately and leave other config fields intact', async (context) => {
  const repoRoot = fixture(context);
  const model = await setConfigValue('model', 'local-model', { repoRoot });
  assert.equal(model.llm.model, 'local-model');
  const effort = await setConfigValue('effort', 'h', { repoRoot });
  assert.equal(effort.llm.effort, 'h');
  assert.equal(loadConfig({ repoRoot }).llm.model, 'local-model');
  const file = join(repoRoot, '.roster', 'config.yml');
  const contents = readFileSync(file, 'utf8');
  assert.match(contents, /profiles:\n/);
  assert.match(contents, /# maximum planner responses/);
  await assert.rejects(setConfigValue('model', 'bad model', { repoRoot }), ConfigError);
  await assert.rejects(setConfigValue('effort', 'max', { repoRoot }), ConfigError);
  assert.equal(readFileSync(file, 'utf8'), contents);
  assert.deepEqual(readdirSync(join(repoRoot, '.roster')).filter((name) =>
    name.startsWith('config.yml.')), []);

  copyFileSync(new URL('../.gitignore', import.meta.url), join(repoRoot, '.gitignore'));
  execFileSync('git', ['init', '--quiet'], { cwd: repoRoot, stdio: 'pipe' });
  assert.doesNotThrow(() => execFileSync('git',
    ['check-ignore', '--quiet', '--', '.roster/config.yml'], { cwd: repoRoot, stdio: 'pipe' }));
});

test('rejects invalid schema, fields, roles, paths, tool names, and endpoint settings', () => {
  for (const [name, source] of [
    ['missing', example.replace(/  effort: m[^\r\n]*\r?\n/, '')],
    ['duplicate', example.replace('schema: 1', 'schema: 1\nschema: 1')],
    ['schema', example.replace('schema: 1', 'schema: 2')],
    ['extra', `${example}merge: true\n`],
    ['principal', example.replace('principal: coder', 'principal: merger')],
    ['App key name', example.replace('api_key_env: ROSTER_API_KEY', 'api_key_env: GITHUB_APP_PRIVATE_KEY_PATH')],
    ['unknown profile', example.replace('profile: ""', 'profile: unknown')],
    ['invalid optional key', example.replace('api_key_optional: true', 'api_key_optional: yes')],
    ['missing vllm optional key', example.replace(/    api_key_optional: true\r?\n/, '')],
    ['duplicate optional key', example.replace('    api_key_optional: true',
      '    api_key_optional: true\n    api_key_optional: false')],
    ['missing named profile', example.replace(/  lmstudio:\r?\n    base_url:.*\r?\n    api_key_env:.*\r?\n/, '')],
    ['duplicate profile field', example.replace('    api_key_env: OPENAI_API_KEY',
      '    api_key_env: OPENAI_API_KEY\n    api_key_env: OTHER_KEY')],
    ['profile credentials URL', example.replace('https://api.openai.com/v1',
      'https://user:pass@api.openai.com/v1')],
    ['profile App key', example.replace('api_key_env: OPENAI_API_KEY',
      'api_key_env: GITHUB_APP_PRIVATE_KEY_PATH')],
    ['unknown profile field', example.replace(/  ollama:\r?\n/,
      '  ollama:\n    tools: [write_file]\n')],
    ['profile and base URL', example.replace('profile: ""', 'profile: ollama')
      .replace('model: ""', 'model: chosen-model')
      .replace('base_url: ""', 'base_url: http://localhost:1234/v1')],
    ['tools', example.replace('search_text]', 'git_push]')],
    ['budget', example.replace('turn_budget: 8', 'turn_budget: 0')],
    ['context budget', example.replace('context_chars: 8000', 'context_chars: 0')],
    ['planner budget', example.replace('turn_budget: 2', 'turn_budget: 65')],
    ['planner missing budget', example.replace(/  turn_budget: 2[^\r\n]*\r?\n/, '')],
    ['planner unknown field', example.replace('planner:', 'planner:\n  tools: [write_file]')],
    ['effort', example.replace('effort: m ', 'effort: max ')],
    ['traversal', example.replace('skills: skills', 'skills: ../outside')],
    ['absolute', example.replace('skills: skills', 'skills: C:\\outside')],
    ['credentials', example.replace('base_url: ""', 'base_url: https://name:secret@api.example/v1')
      .replace('model: ""', 'model: test-model')],
  ]) {
    assert.throws(() => parseConfig(source), ConfigError, name);
  }
  assert.throws(() => parseConfig('x'.repeat(65_537)), /at most 64 KiB/);
  const legacy = example.replace(/planner:\r?\n  turn_budget: 2[^\r\n]*\r?\n/, '');
  assert.equal(parseConfig(legacy).planner.turn_budget, 1);
  const withoutProfiles = example.replace(/  profile: ""[^\r\n]*\r?\n/, '')
    .replace(/profiles:\r?\n[\s\S]*?(?=planner:)/, '');
  assert.equal(parseConfig(withoutProfiles).llm.profile, '');
  assert.equal(parseConfig(withoutProfiles).profiles.openai.api_key_env, 'OPENAI_API_KEY');
  assert.equal(parseConfig(withoutProfiles).profiles['vllm-local'].api_key_optional, true);
  const olderProfiles = example.replace(
    /  vllm-local:\r?\n    base_url:.*\r?\n    api_key_env:.*\r?\n    api_key_optional:.*\r?\n/, '');
  const legacyProfile = parseConfig(olderProfiles.replace('profile: ""', 'profile: vllm-local'));
  assert.equal(legacyProfile.llm.base_url, 'http://127.0.0.1:8000/v1');
  assert.equal(legacyProfile.llm.api_key_optional, true);
});

test('older configs retain the default context budget and can explicitly set it', () => {
  assert.equal(parseConfig(example.replace(/  context_chars:[^\r\n]*\r?\n/, '')).seat.context_chars, 8000);
  assert.equal(parseConfig(example.replace('context_chars: 8000', 'context_chars: 16000'))
    .seat.context_chars, 16000);
});

test('only a missing private config triggers the example fallback', (context) => {
  const repoRoot = fixture(context);
  const privateFile = join(repoRoot, '.roster', 'config.yml');
  mkdirSync(join(repoRoot, '.roster'));
  writeFileSync(privateFile, 'bad: input\n');
  assert.throws(() => loadConfig({ repoRoot }), ConfigError);
  rmSync(privateFile);
  writeFileSync(privateFile, Buffer.from([0xff]));
  assert.throws(() => loadConfig({ repoRoot }), /UTF-8/);
  rmSync(privateFile);
  try {
    symlinkSync(fileURLToPath(new URL('../roster.config.example.yml', import.meta.url)), privateFile);
  } catch (error) {
    if (['EPERM', 'EACCES', 'ENOTSUP'].includes(error.code)) {
      context.skip('Creating symlinks is unavailable on this system.');
      return;
    }
    throw error;
  }
  assert.throws(() => loadConfig({ repoRoot }), /non-symlink/);
});
