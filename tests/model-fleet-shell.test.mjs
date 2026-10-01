import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { PassThrough } from 'node:stream';
import { setTimeout as wait } from 'node:timers/promises';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { formatFleet } from '../src/lib/fleet.mjs';
import { createDispatcher, startRepl } from '../src/repl.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example);
const profile = { id: 'spark-4', base_url: 'https://spark.example.invalid/v1', model: 'deepseek-v4.1-flash',
  provider: 'vllm', context_max: 1048576, concurrency: 1, hardware: 'spark-4', notes: '512GB class' };

function fixture(t) {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-model-shell-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(path.join(root, '.roster'));
  writeFileSync(path.join(root, '.roster', 'config.yml'), example);
  const fleetPath = path.join(root, '.roster', 'fleet.yml');
  writeFileSync(fleetPath, formatFleet({ profiles: [profile] }));
  let text = '';
  const shell = createDispatcher({ cwd: root, config, env: {}, services: {
    repositoryRoot: () => root, repositoryBranch: () => 'main',
  }, output: { write(value) { text += value; } }, errorOutput: { write(value) { text += value; } } });
  return { root, fleetPath, configPath: path.join(root, '.roster', 'config.yml'),
    ...shell, get text() { return text; } };
}

test('model and effort selection are session-only; only model --save writes the private model', async (t) => {
  const shell = fixture(t);
  const initial = readFileSync(shell.configPath, 'utf8');
  await shell.dispatch('/model session-model');
  await shell.dispatch('/effort x');
  await shell.dispatch('/model');
  await shell.dispatch('/effort status');
  assert.equal(shell.state.config.llm.model, 'session-model');
  assert.equal(shell.state.config.llm.effort_override, 'x');
  assert.equal(readFileSync(shell.configPath, 'utf8'), initial);
  assert.match(shell.text, /Model: session-model\nHost:/);
  await shell.dispatch('/model saved-model --save');
  const saved = parseConfig(readFileSync(shell.configPath, 'utf8'));
  assert.equal(saved.llm.model, 'saved-model');
  assert.equal(saved.llm.effort, config.llm.effort);
  await shell.dispatch('/model clear');
  assert.equal(shell.state.config.llm.model, '');
  assert.equal(shell.state.routeNext, true);
  assert.equal(parseConfig(readFileSync(shell.configPath, 'utf8')).llm.model, 'saved-model');
});

test('fleet use updates only session endpoint, model and capacity and provider prints no key', async (t) => {
  const shell = fixture(t);
  const before = readFileSync(shell.fleetPath, 'utf8');
  const configBefore = readFileSync(shell.configPath, 'utf8');
  await shell.dispatch('/fleet');
  await shell.dispatch('/fleet use spark-4');
  await shell.dispatch('/provider');
  assert.equal(shell.state.config.llm.model, profile.model);
  assert.equal(shell.state.config.llm.base_url, profile.base_url);
  assert.equal(shell.state.display.contextMax, 1048576);
  assert.equal(readFileSync(shell.fleetPath, 'utf8'), before);
  assert.equal(readFileSync(shell.configPath, 'utf8'), configBefore);
  assert.match(shell.text, /spark-4 \| spark\.example\.invalid \| deepseek-v4\.1-flash \| ctx 1048576/);
  assert.match(shell.text, /Profile: spark-4\nHost: spark\.example\.invalid/);
});

test('probe uses a read-only models GET and prints ids/context without secrets or writes', async (t) => {
  const root = fixture(t);
  const env = { ROSTER_API_KEY: 'secret-probe-marker' };
  let text = '';
  let requests = 0;
  const shell = createDispatcher({ cwd: root.root, config, env,
    services: { repositoryRoot: () => root.root, repositoryBranch: () => 'main',
      probeModelDetails: async (base, options) => {
        assert.equal(base, profile.base_url);
        assert.equal(options.env, env);
        requests += 1;
        return [{ id: 'public-model', context_max: 1048576 }, { id: 'secret-probe-marker', context_max: 8192 }];
      } },
    output: { write(value) { text += value; } }, errorOutput: { write(value) { text += value; } } });
  const before = readFileSync(root.fleetPath, 'utf8');
  const configBefore = readFileSync(root.configPath, 'utf8');
  await shell.dispatch('/fleet use spark-4');
  await shell.dispatch('/fleet probe');
  assert.equal(requests, 1);
  assert.match(text, /public-model \| ctx 1048576/);
  assert.doesNotMatch(text, /secret-probe-marker/);
  assert.equal(readFileSync(root.fleetPath, 'utf8'), before);
  assert.equal(readFileSync(root.configPath, 'utf8'), configBefore);
});

test('probe --set-model saves only an explicitly selected listed model', async (t) => {
  const root = fixture(t);
  const shell = createDispatcher({ cwd: root.root, config, env: {},
    services: { repositoryRoot: () => root.root, repositoryBranch: () => 'main',
      probeModelDetails: async () => [{ id: 'listed-model', context_max: 8192 }] },
    output: { write() {} }, errorOutput: { write() {} } });
  await shell.dispatch('/fleet use spark-4');
  await shell.dispatch('/fleet probe --set-model listed-model');
  assert.equal(parseConfig(readFileSync(root.configPath, 'utf8')).llm.model, 'listed-model');
  await assert.rejects(shell.dispatch('/fleet probe --set-model missing-model'), /Choose a listed model/);
});

test('fleet add forwards existing non-TTY flags including quoted hardware unchanged', async () => {
  let received;
  const shell = createDispatcher({ config, env: {}, services: {
    repositoryBranch: () => 'main', repositoryRoot: () => process.cwd(),
    runFleet: async (args, options) => { received = args; assert.equal(options.input.isTTY, false); },
  }, input: { isTTY: false }, output: { write() {} }, errorOutput: { write() {} } });
  await shell.dispatch('/fleet add --id spark --base-url https://spark.example.invalid --model public-model --hardware "512GB class"');
  assert.deepEqual(received, ['add', '--id', 'spark', '--base-url', 'https://spark.example.invalid',
    '--model', 'public-model', '--hardware', '512GB class']);
});

test('interactive fleet questions use the current readline without executing answers as asks', async (t) => {
  const root = fixture(t);
  const input = new PassThrough();
  input.isTTY = true;
  input.setRawMode = () => {};
  const output = new PassThrough();
  output.isTTY = true;
  output.columns = 120;
  let text = '';
  let answer;
  output.on('data', (data) => { text += data.toString(); });
  const done = startRepl({ input, output, errorOutput: output, cwd: root.root, config, env: {},
    services: { repositoryRoot: () => root.root, repositoryBranch: () => 'main',
      runBuiltinAsk: () => assert.fail('Fleet answers must not be submitted as asks'),
      runFleet: async (_args, options) => { answer = await options.question('Select model: '); } } });
  for (let index = 0; index < 100 && !text.includes('roster> '); index += 1) await wait(10);
  input.write('/fleet add --id spark --base-url https://spark.example.invalid\n');
  for (let index = 0; index < 100 && !text.includes('Select model: '); index += 1) await wait(10);
  assert.ok(text.includes('Select model: '));
  input.write('public-model\n');
  for (let index = 0; index < 100 && !answer; index += 1) await wait(10);
  assert.equal(answer, 'public-model');
  input.write('/quit\n');
  assert.equal(await done, 0);
  input.destroy();
  output.destroy();
  const history = path.join(root.root, '.roster', 'history');
  if (existsSync(history)) assert.doesNotMatch(readFileSync(history, 'utf8'), /^public-model$/m);
});
