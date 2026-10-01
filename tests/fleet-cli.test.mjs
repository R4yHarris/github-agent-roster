import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { loadConfig } from '../src/lib/config.mjs';
import { runFleet } from '../src/lib/fleet-cli.mjs';
import { formatFleet, loadFleet } from '../src/lib/fleet.mjs';

const installation = fileURLToPath(new URL('../', import.meta.url));
const example = readFileSync(join(installation, 'roster.config.example.yml'), 'utf8');
const sglangModels = JSON.parse(readFileSync(new URL('./fixtures/models-sglang.json', import.meta.url), 'utf8'));
const first = { id: 'first', base_url: 'https://first.example.invalid/v1', model: 'owner/first',
  provider: 'vllm', context_max: 32768, concurrency: 1, hardware: 'test-gpu', notes: '' };
const second = { ...first, id: 'second', base_url: 'https://second.example.invalid/v1', model: 'owner/second' };

function fixture(t, profiles = []) {
  const cwd = mkdtempSync(join(tmpdir(), 'roster-fleet-cli-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  mkdirSync(join(cwd, '.roster'));
  writeFileSync(join(cwd, 'roster.config.example.yml'), example);
  const file = join(cwd, '.roster', 'fleet.yml');
  writeFileSync(file, formatFleet({ profiles }));
  let text = '';
  return { cwd, installationRoot: cwd, env: {}, input: { isTTY: false },
    output: { isTTY: false, write(value) { text += String(value); } },
    get text() { return text; }, file };
}

const models = (...ids) => async () => Response.json({ data: ids.map((id) => ({ id })) });

test('non-TTY add registers explicit endpoint/model/capacity without changing the default', async (t) => {
  const options = fixture(t);
  let requests = 0;
  const result = await runFleet(['add', '--id', 'new-profile', '--base-url', 'https://new.example.invalid',
    '--model', 'owner/new', '--context', '65536', '--concurrency', '2',
    '--hardware', 'test-gpu', '--task-class', 'feat,fix'], {
    ...options, fetchImpl: async (url, request) => {
      requests += 1;
      assert.equal(url, 'https://new.example.invalid/v1/models');
      assert.equal(request.method, 'GET');
      return Response.json({ data: [{ id: 'owner/new' }, { id: 'owner/alternative' }] });
    },
  });
  assert.equal(requests, 1);
  assert.equal(result.profile.model, 'owner/new');
  assert.equal(result.profile.context_max, 65536);
  assert.equal(result.profile.concurrency, 2);
  assert.deepEqual(result.profile.task_class, ['feat', 'fix']);
  assert.match(options.text, /owner\/alternative/);
  assert.match(options.text, /Default config unchanged/);
  assert.equal(existsSync(join(options.cwd, '.roster', 'config.yml')), false);
  assert.equal((await loadFleet({ cwd: options.cwd })).profiles.length, 1);
  assert.match(readFileSync(join(options.cwd, '.gitignore'), 'utf8'), /\.roster\/fleet\.yml/);
});

test('missing non-TTY arguments, duplicate IDs, and missing fleet IDs fail before a probe', async (t) => {
  const options = fixture(t, [first]);
  const fetchImpl = () => assert.fail('Invalid registration or lookup must not contact a model endpoint');
  for (const args of [
    ['add', '--id', 'x', '--base-url', 'https://x.example.invalid', '--context', '32768'],
    ['probe', 'first', '--set-model'],
    ['probe', 'missing'],
    ['default', 'missing'],
    ['add', '--id', 'first', '--base-url', first.base_url, '--model', first.model, '--context', '32768'],
    ['add', '__proto__', 'value', '--id', 'x', '--base-url', 'https://x.example.invalid'],
  ]) {
    await assert.rejects(runFleet(args, { ...options, fetchImpl }), /requires|not found|already exists|Use roster fleet/);
  }
  assert.equal((await loadFleet({ cwd: options.cwd })).profiles.length, 1);
});

test('interactive add selects a served ID and requires a supplied context limit', async (t) => {
  const options = fixture(t);
  const answers = ['2', '32768'];
  const prompts = [];
  const result = await runFleet(['add', '--id', 'interactive', '--base-url', 'https://x.example.invalid/v1'], {
    ...options, input: { isTTY: true }, output: { ...options.output, isTTY: true },
    fetchImpl: models('first-model', 'chosen-model'),
    question: async (prompt) => { prompts.push(prompt); return answers.shift(); },
  });
  assert.equal(result.profile.model, 'chosen-model');
  assert.equal(result.profile.context_max, 32768);
  assert.equal(result.profile.concurrency, 1);
  assert.deepEqual(prompts, ['Select a model [1]: ', 'Model context limit in tokens [required]: ']);
});

test('fleet add uses reported context without prompting in TTY and non-TTY modes', async (t) => {
  for (const tty of [true, false]) {
    const options = fixture(t);
    const prompts = [];
    let requests = 0;
    const result = await runFleet(['add', '--id', 'sglang', '--base-url', 'https://sglang.example.invalid',
      ...(tty ? [] : ['--model', 'chosen-model'])], {
      ...options, input: { isTTY: tty }, output: { ...options.output, isTTY: tty },
      fetchImpl: async () => { requests += 1; return Response.json(sglangModels); },
      question: async (prompt) => { prompts.push(prompt); return '2'; },
    });
    assert.equal(requests, 1);
    assert.equal(result.profile.model, 'chosen-model');
    assert.equal(result.profile.context_max, 1048576);
    assert.equal((await loadFleet({ cwd: options.cwd })).profiles[0].context_max, 1048576);
    assert.deepEqual(prompts, tty ? ['Select a model [1]: '] : []);
  }
});

test('local Ollama registration GETs models and stores max_model_len beside the unchanged Spark default', async (t) => {
  const spark = { ...first, id: 'default', model: 'deepseek-v4.1-flash',
    base_url: 'http://spark.example.invalid:8000/v1', context_max: 1048576, hardware: 'Spark' };
  const options = fixture(t, [spark]);
  await runFleet(['default', 'default'], options);
  const configPath = join(options.cwd, '.roster', 'config.yml');
  const before = readFileSync(configPath, 'utf8');
  let requests = 0;
  const result = await runFleet(['add', '--id', 'ollama-local', '--base-url', 'http://127.0.0.1:11434/v1',
    '--model', 'local-test-model', '--hardware', 'local-host'], {
    ...options, fetchImpl: async (url, request) => {
      requests += 1;
      assert.equal(url, 'http://127.0.0.1:11434/v1/models');
      assert.equal(request.method, 'GET');
      assert.equal(request.headers.Authorization, undefined);
      return Response.json({ data: [{ id: 'local-test-model', max_model_len: 1048576 }] });
    },
  });
  assert.equal(requests, 1);
  assert.equal(result.profile.context_max, 1048576);
  assert.equal(result.profile.base_url, 'http://127.0.0.1:11434/v1');
  assert.equal(readFileSync(configPath, 'utf8'), before);
  assert.deepEqual((await loadFleet({ cwd: options.cwd })).profiles.map(({ id }) => id),
    ['default', 'ollama-local']);
  assert.equal(loadConfig({ repoRoot: options.cwd, cwd: options.cwd }).llm.model, spark.model);
});

test('fleet add without discovered context retains explicit limits and refuses unknown non-TTY capacity', async (t) => {
  const options = fixture(t);
  await assert.rejects(runFleet(['add', '--id', 'missing', '--base-url', 'https://x.example.invalid',
    '--model', 'chosen-model'], { ...options, fetchImpl: models('chosen-model') }),
  /requires --context when \/v1\/models does not report/);
  const result = await runFleet(['add', '--id', 'explicit', '--base-url', 'https://x.example.invalid',
    '--model', 'chosen-model', '--context', '32768'], {
    ...options, fetchImpl: async () => Response.json(sglangModels),
  });
  assert.equal(result.profile.context_max, 32768);
});

test('fleet probe reports discovered context read-only and explicit refresh synchronizes the active default', async (t) => {
  const options = fixture(t, [first]);
  await runFleet(['default', 'first'], options);
  const before = readFileSync(options.file, 'utf8');
  const beforeConfig = readFileSync(join(options.cwd, '.roster', 'config.yml'), 'utf8');
  const fetchImpl = async () => Response.json({
    data: [{ id: first.model, max_model_len: 1048576 }, { id: 'new-model', max_context_length: 65536 }],
  });
  const probe = await runFleet(['probe', 'first'], { ...options, fetchImpl });
  assert.equal(probe.context_max, 1048576);
  assert.equal(probe.profile.context_max, first.context_max);
  assert.deepEqual(probe.models, [first.model, 'new-model']);
  assert.match(options.text, /context_max=1048576/);
  assert.equal(readFileSync(options.file, 'utf8'), before);
  assert.equal(readFileSync(join(options.cwd, '.roster', 'config.yml'), 'utf8'), beforeConfig);
  await runFleet(['probe', 'first', '--set-model', 'new-model'], { ...options, fetchImpl });
  assert.equal((await loadFleet({ cwd: options.cwd })).profiles[0].context_max, 65536);
  const config = loadConfig({ repoRoot: options.cwd, cwd: options.cwd });
  assert.equal(config.llm.model, 'new-model');
  assert.equal(config.llm.context_max, 65536);
});

test('probe prints model inventory without changing files unless set-model is explicit', async (t) => {
  const options = fixture(t, [first]);
  const before = readFileSync(options.file, 'utf8');
  await runFleet(['probe', 'first'], { ...options, fetchImpl: models('owner/first', 'owner/new') });
  assert.equal(readFileSync(options.file, 'utf8'), before);
  assert.match(options.text, /Saved model unchanged: owner\/first/);
  await assert.rejects(runFleet(['probe', 'first', '--set-model', 'missing-model'], {
    ...options, fetchImpl: models('owner/new'),
  }), /not in the endpoint models list/);
  assert.equal(readFileSync(options.file, 'utf8'), before);
  await runFleet(['probe', 'first', '--set-model', 'owner/new'], { ...options, fetchImpl: models('owner/new') });
  assert.equal((await loadFleet({ cwd: options.cwd })).profiles[0].model, 'owner/new');
});

test('default selects the endpoint, and explicit model refresh keeps an active default in sync', async (t) => {
  const options = fixture(t, [first, second]);
  writeFileSync(join(options.cwd, '.roster', 'config.yml'), example.replace('enabled: true', 'enabled: false'));
  await runFleet(['default', 'second'], options);
  let config = loadConfig({ repoRoot: options.cwd, cwd: options.cwd });
  assert.equal(config.llm.model, second.model);
  assert.equal(config.llm.base_url, second.base_url);
  assert.equal(config.llm.context_max, second.context_max);
  assert.equal(config.publish.enabled, false);
  await runFleet(['probe', 'second', '--set-model', 'owner/updated'], {
    ...options, fetchImpl: models('owner/updated'),
  });
  config = loadConfig({ repoRoot: options.cwd, cwd: options.cwd });
  assert.equal(config.llm.model, 'owner/updated');
  assert.equal((await loadFleet({ cwd: options.cwd })).profiles[1].model, 'owner/updated');
  assert.equal(config.publish.enabled, false);
});

test('removing the last or active profile is refused; other profiles can be removed', async (t) => {
  const options = fixture(t, [first, second]);
  await runFleet(['default', 'first'], options);
  await assert.rejects(runFleet(['remove', 'first'], options), /Select another fleet default/);
  await runFleet(['remove', 'second'], options);
  assert.equal((await loadFleet({ cwd: options.cwd })).profiles.length, 1);
  await assert.rejects(runFleet(['remove', 'first'], options), /last fleet profile/);
});

test('invalid capacity or unavailable model cannot be saved as a fleet profile', async (t) => {
  const options = fixture(t);
  for (const extra of [['--model', 'unserved', '--context', '32768'],
    ['--model', 'served', '--context', '0'], ['--model', 'served', '--context', '32768', '--concurrency', '0']]) {
    await assert.rejects(runFleet(['add', '--id', 'invalid', '--base-url', 'https://x.example.invalid',
      ...extra], { ...options, fetchImpl: models('served') }), /not in|positive/);
  }
  assert.equal((await loadFleet({ cwd: options.cwd })).profiles.length, 0);
});

test('fleet list is exposed by the roster bin and performs no model or publishing request', (t) => {
  const options = fixture(t, [first]);
  const result = spawnSync(process.execPath, [join(installation, 'src', 'cli.mjs'), 'fleet', 'list'], {
    cwd: options.cwd, encoding: 'utf8', input: '', timeout: 10_000,
  });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /ID\tMODEL\tBASE_URL/);
  assert.match(result.stdout, /first\towner\/first/);
  const invalid = spawnSync(process.execPath, [join(installation, 'src', 'cli.mjs'), 'fleet',
    'add', '--id', 'new', '--base-url', 'http://localhost:8000/v1'], {
    cwd: options.cwd, encoding: 'utf8', input: '', timeout: 10_000,
  });
  assert.equal(invalid.status, 1);
  assert.match(invalid.stderr, /Non-TTY fleet add requires/);
});
