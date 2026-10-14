import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, linkSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { PassThrough } from 'node:stream';
import { setTimeout as wait } from 'node:timers/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { answerRepositoryQuestion, repositoryQuestionText } from '../src/lib/repository-question.mjs';
import { submitAsk, formatAsk } from '../src/lib/ask.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { createDispatcher, startRepl } from '../src/repl.mjs';

const sourceRoot = fileURLToPath(new URL('../', import.meta.url));
const config = parseConfig(readFileSync(path.join(sourceRoot, 'roster.config.example.yml'), 'utf8'));
const live = { ...config, llm: { ...config.llm, base_url: 'http://localhost:8000/v1', model: 'question-fixture' } };
const question = (scope = ['src/api.mjs'], text = 'How does the bounded API work?') => text + '\n\n## Files allowed\n' + scope.map((file) => '- ' + file).join('\n');
function fixture(t) {
  const cwd = mkdtempSync(path.join(process.cwd(), '.repository-question-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  mkdirSync(path.join(cwd, 'src'));
  writeFileSync(path.join(cwd, 'src', 'api.mjs'), 'export const value = 42;\n');
  return { cwd, config, env: {} };
}
const output = () => ({ text: '', write(text) { this.text += text; } });

test('offline bounded question returns real line evidence without creating task or state', async (t) => {
  const options = fixture(t);
  const before = readdirSync(options.cwd);
  const result = await answerRepositoryQuestion(question(), options);
  assert.equal(result.mode, 'question');
  assert.match(result.answer, /Source src\/api.mjs/);
  assert.match(result.answer, /1: export const value = 42/);
  assert.match(result.answer, /interpretation is unavailable/);
  assert.deepEqual(readdirSync(options.cwd), before);
  assert.equal(existsSync(path.join(options.cwd, 'TASK.md')), false);
});

test('natural CLI intake questions never create issues or infer source permission from URLs/mentions', async (t) => {
  const options = fixture(t);
  for (const ask of ['How does src/api.mjs work?', 'How does https://example.test/src/api.mjs work?']) {
    const result = await submitAsk(ask, { ...options, repoRoot: options.cwd, runCommand: async () => assert.fail('no GitHub command') });
    assert.match(formatAsk(result), /No source scope was granted/);
    assert.doesNotMatch(result.answer, /1: export/);
  }
  const scoped = await submitAsk(question(), { ...options, repoRoot: options.cwd, runCommand: async () => assert.fail('no issue') });
  assert.match(formatAsk(scoped), /1: export/);
});

test('one tool-free inference receives only redacted granted evidence; model output is not execution authority', async (t) => {
  const options = fixture(t); let calls = 0;
  const secret = 'private-question-credential-012345678901';
  writeFileSync(path.join(options.cwd, 'src', 'api.mjs'), 'export const value = 42; // ' + secret + '\n');
  const result = await answerRepositoryQuestion(question(), { ...options, config: live, env: { ROSTER_API_KEY: secret },
    fetchImpl: async (_url, request) => {
      calls++;
      const body = JSON.parse(request.body);
      assert.equal(body.tools, undefined);
      assert.equal(body.max_tokens, 1024);
      assert.match(body.messages[1].content, /Source src\/api.mjs/);
      assert.equal(body.messages[1].content.includes(secret), false);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'src/api.mjs:1 declares 42. ' + secret + '\u001b[31m' } }] });
    } });
  assert.equal(calls, 1);
  assert.equal(result.answer.includes(secret), false);
  assert.equal(result.answer.includes('\u001b'), false);
  assert.match(result.answer, /model interpretation/);
  assert.deepEqual(readdirSync(options.cwd), ['src']);
});

for (const kind of ['tool', 'empty', 'cancel']) {
  test('refuse unsafe/non-answer inference: ' + kind, async (t) => {
    const options = fixture(t); const controller = new AbortController();
    await assert.rejects(answerRepositoryQuestion(question(), { ...options, config: live, signal: controller.signal,
      fetchImpl: async () => {
        if (kind === 'cancel') controller.abort(new Error('cancelled'));
        return Response.json({ choices: [{ finish_reason: kind === 'tool' ? 'tool_calls' : 'stop',
          message: kind === 'tool' ? { role: 'assistant', tool_calls: [] } : { role: 'assistant', content: kind === 'empty' ? '' : 'answer' } }] });
      } }), /read-only|plain-text|cancel/i);
    assert.deepEqual(readdirSync(options.cwd), ['src']);
  });
}

for (const scope of [['../escape.mjs'], ['.env'], ['TASK.md'], ['src/*.mjs'], ['https://example.test/api.mjs']]) {
  test('scope refusal preserves no-write boundary: ' + scope, async (t) => {
    const options = fixture(t);
    await assert.rejects(answerRepositoryQuestion(question(scope), options), /scope|Files allowed|safe relative|private|public|path/i);
  });
}

for (const kind of ['hardlink', 'junction', 'large', 'encoding', 'file-cap', 'budget']) {
  test('bounded source/storage refusal: ' + kind, async (t) => {
    const options = fixture(t); let scope = ['src/api.mjs'];
    if (kind === 'hardlink') linkSync(path.join(options.cwd, 'src', 'api.mjs'), path.join(options.cwd, 'copy.mjs'));
    if (kind === 'junction') { symlinkSync(path.join(options.cwd, 'src'), path.join(options.cwd, 'linked'), process.platform === 'win32' ? 'junction' : 'dir'); scope = ['linked/**']; }
    if (kind === 'encoding') writeFileSync(path.join(options.cwd, 'src', 'api.mjs'), Buffer.from([0xc3,0x28]));
    if (kind === 'large') writeFileSync(path.join(options.cwd, 'src', 'api.mjs'), 'x'.repeat(65537));
    if (kind === 'missing') scope = ['src/missing.mjs'];
    if (kind === 'file-cap') { for (let i=0;i<9;i++) writeFileSync(path.join(options.cwd,'src', 'file'+i+'.mjs'), 'public'); scope = ['src/**']; }
    if (kind === 'budget') options.config = { ...config, seat: { ...config.seat, context_chars: 10 } };
    await assert.rejects(answerRepositoryQuestion(question(scope), options), /single-link|symlink|bounded|UTF-8|ENOENT|eight source|budget/i);
  });
}

test('directory grant supplies public excerpts only and marks omitted lines unknown', async (t) => {
  const options = fixture(t);
  writeFileSync(path.join(options.cwd, 'src', '.env'), 'private');
  writeFileSync(path.join(options.cwd, 'src', 'api.mjs'), Array.from({length:60}, (_,i) => 'line '+(i+1)).join('\n'));
  const result = await answerRepositoryQuestion(question(['src/**']), options);
  assert.match(result.answer, /48: line 48/);
  assert.doesNotMatch(result.answer, /49: line 49|private/);
  assert.match(result.answer, /omitted lines.*unknown/);
});

test('real CLI process restart gives same offline source answer without GitHub or state', async (t) => {
  const options = fixture(t);
  execFileSync('git', ['init', '-q', options.cwd], { stdio: 'pipe' });
  mkdirSync(path.join(options.cwd, '.roster'));
  const configPath = path.join(options.cwd, '.roster', 'config.yml');
  const offlineConfig = readFileSync(path.join(sourceRoot, 'roster.config.example.yml'), 'utf8');
  writeFileSync(configPath, offlineConfig);
  const preload = path.join(options.cwd, 'deny-requests.mjs');
  writeFileSync(preload, "import child from 'node:child_process';\n" +
    "import fs from 'node:fs';\n" +
    "import path from 'node:path';\n" +
    "import { syncBuiltinESMExports } from 'node:module';\n" +
    `const installedConfig = ${JSON.stringify(path.join(sourceRoot, '.roster', 'config.yml'))};\n` +
    "const read = fs.readFileSync;\n" +
    "fs.readFileSync = function(file, ...args) {\n" +
    "  if (typeof file === 'string' && path.resolve(file) === installedConfig) throw new Error('Offline question must not read installed model configuration');\n" +
    "  return read.call(this, file, ...args);\n" +
    "};\n" +
    "globalThis.fetch = () => { throw new Error('Offline question must not request a model'); };\n" +
    "for (const name of ['execFile', 'execFileSync', 'spawn', 'spawnSync']) {\n" +
    "  const original = child[name];\n" +
    "  child[name] = function(program, ...args) {\n" +
    "    if (/^(?:.*[\\\\/])?gh(?:\\.exe)?$/i.test(program)) throw new Error('Offline question must not request GitHub');\n" +
    "    return original.call(this, program, ...args);\n" +
    "  };\n" +
    "}\nsyncBuiltinESMExports();\n");
  const before = readdirSync(options.cwd).sort();
  const env = { PATH: process.env.PATH, SystemRoot: process.env.SystemRoot, USERPROFILE: process.env.USERPROFILE };
  for (const command of ['question', 'ask', 'question']) {
    const result = spawnSync(process.execPath, ['--import', pathToFileURL(preload).href,
      path.join(sourceRoot, 'src', 'cli.mjs'), command, ...(command === 'question' ? ['--files', 'src/api.mjs', '--', 'How does this work?'] : [question()])],
      { cwd: options.cwd, env, encoding: 'utf8', timeout: 30000 });
    assert.equal(result.status, 0, result.stderr + result.stdout);
    assert.match(result.stdout, /1: export const value = 42/);
  }
  assert.deepEqual(readdirSync(options.cwd).sort(), before);
  assert.deepEqual(readdirSync(path.join(options.cwd, '.roster')), ['config.yml']);
  assert.equal(readFileSync(configPath, 'utf8'), offlineConfig);
});

test('fresh shell explicit/natural questions read sources without invoking seats; stop cancels response', async (t) => {
  const options = fixture(t); const writes = output();
  const shell = createDispatcher({ ...options, output: writes, errorOutput: output(), services: {
    repositoryRoot: () => options.cwd, runBuiltinAsk: () => assert.fail('question must not run seats'),
  } });
  await shell.dispatch('/question --files src/api.mjs -- How does the API work?');
  await shell.dispatch('/ask ' + question());
  assert.match(writes.text, /1: export const value/);
  assert.equal(shell.state.lastRun, null);
  const cancelled = output(); let started; const ready = new Promise((resolve) => { started = resolve; });
  const active = createDispatcher({ ...options, output: cancelled, errorOutput: output(), services: {
    repositoryRoot: () => options.cwd,
    answerRepositoryQuestion: async ({}, request) => { started(); await new Promise((resolve) => request.signal.addEventListener('abort', resolve)); return {answer:'must not print'}; },
  } });
  const pending = active.dispatch('/question ' + question());
  await ready; await active.dispatch('/stop');
  await assert.rejects(pending, /cancel/i);
  assert.doesNotMatch(cancelled.text, /must not print/);
});

test('absent source is explicit unknown with actionable follow-up, never inference', async (t) => {
  const options = fixture(t);
  const result = await answerRepositoryQuestion(question(['src/missing.mjs']), { ...options, config: live,
    fetchImpl: () => assert.fail('absent evidence must not invoke model') });
  assert.match(result.answer, /Unavailable source src\/missing.mjs: absent; behavior unknown/);
  assert.match(result.answer, /Narrow or correct/);
});

test('explicit question command never derives execution authority from change wording', async (t) => {
  const options = fixture(t); const writes = output();
  const shell = createDispatcher({ ...options, output: writes, errorOutput: output(), services: {
    repositoryRoot: () => options.cwd, runBuiltinAsk: () => assert.fail('no change execution'),
  } });
  await shell.dispatch('/question ' + question(['src/api.mjs'], 'How could I change the API?'));
  assert.match(writes.text, /1: export/);
  assert.deepEqual(readdirSync(options.cwd), ['src']);
});

test('directory traversal and model output have explicit caps before completion', async (t) => {
  const options = fixture(t);
  for (let i=0;i<129;i++) mkdirSync(path.join(options.cwd,'src','dir'+i));
  await assert.rejects(answerRepositoryQuestion(question(['src/**']), options), /128 entries/);
  await assert.rejects(answerRepositoryQuestion(question(), { ...options, config: live,
    fetchImpl: async () => Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'x'.repeat(8193) } }] }) }), /bounded output/);
});

test('private endpoint keys in original question and stdout remain redacted', async (t) => {
  const options = fixture(t); const secret = 'question-original-private-value-012345678901';
  const result = await answerRepositoryQuestion('How does ' + secret + ' work?', {
    ...options, env: { CUSTOM_KEY: secret }, config: { ...config, llm: { ...config.llm, api_key_env: 'CUSTOM_KEY' } } });
  assert.equal(result.answer.includes(secret), false);
  assert.match(result.answer, /No source scope was granted/);
});

test('human outcome/scope/acceptance contract stays an executable slice whatever its question title', async (t) => {
  const options = fixture(t); let issueWrites = 0;
  const ask = question() + '\n\n## Acceptance checks\n- The requested API behavior is implemented';
  const result = await submitAsk(ask, { ...options, repoRoot: options.cwd, runCommand: async (program, args) => {
    if (program === 'git') return 'https://github.com/example/project.git';
    if (args[0] === '--version') return 'gh version fixture';
    issueWrites++; return 'https://github.com/example/project/issues/42';
  } });
  assert.equal(result.mode, 'issue');
  assert.equal(result.askKind, 'slice');
  assert.equal(issueWrites, 1);
});

test('streaming single-line REPL question answers from sources across fresh sessions without seats', async (t) => {
  const options = fixture(t);
  for (let index=0;index<2;index++) {
    const input = new PassThrough(); const output = new PassThrough(); let text = '';
    output.on('data', (data) => { text += data; });
    const done = startRepl({ ...options, input, output, errorOutput: output,
      services: { repositoryRoot: () => options.cwd, repositoryBranch: () => 'main',
        runBuiltinAsk: () => assert.fail('streaming question started seats') } });
    input.write('/question --files src/api.mjs -- How does the API work?\n');
    for (let step=0;step<200 && !text.includes('1: export const value');step++) await wait(10);
    assert.match(text, /1: export const value = 42/);
    input.write('/quit\n'); await done;
  }
  assert.equal(existsSync(path.join(options.cwd,'TASK.md')), false);
});

test('single-line scope parser refuses missing/duplicated declarations', () => {
  assert.throws(() => repositoryQuestionText(['--files','src/api.mjs','How?']), /Use question/);
  assert.throws(() => repositoryQuestionText(['--files','src/api.mjs','--',question()]), /declared once/);
});
