import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { PassThrough } from 'node:stream';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { runFleet } from '../src/lib/fleet-cli.mjs';
import { formatFleet, loadFleet } from '../src/lib/fleet.mjs';
import { runFleetAssist } from '../src/onboard/fleet-assist.mjs';

const installation = fileURLToPath(new URL('../', import.meta.url));
const example = readFileSync(join(installation, 'roster.config.example.yml'), 'utf8');
const configSource = example.replace('base_url: ""', 'base_url: https://interview.example.invalid/v1')
  .replace('model: ""', 'model: interview-model').replace('profile: ""', 'profile: vllm-local');
const baseline = { id: 'default', base_url: 'https://interview.example.invalid/v1', model: 'interview-model',
  provider: 'vllm', context_max: 0, concurrency: 1, hardware: 'unspecified', notes: '' };
const proposed = { id: 'new-endpoint', base_url: 'https://new.example.invalid/v1', model: 'served-model',
  provider: 'vllm', context_max: 32768, concurrency: 2, hardware: 'test-gpu',
  task_class: ['fix', 'test'], notes: 'Operator-supplied note.' };
const answers = () => ['new-endpoint', 'https://new.example.invalid', 'served-model',
  'test-gpu', '32768', '2', 'fix,test', 'Operator-supplied note.'];

function capture() {
  let text = '';
  return { isTTY: true, write(value) { text += String(value); }, get text() { return text; } };
}

function fixture(t, replies) {
  const cwd = mkdtempSync(join(tmpdir(), 'roster-fleet-assist-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  mkdirSync(join(cwd, '.roster'));
  const configPath = join(cwd, '.roster', 'config.yml');
  const fleetPath = join(cwd, '.roster', 'fleet.yml');
  writeFileSync(configPath, configSource);
  writeFileSync(fleetPath, formatFleet({ profiles: [baseline] }));
  const output = capture();
  const errorOutput = capture();
  const prompts = [];
  return { cwd, installationRoot: installation, input: { isTTY: true }, output, errorOutput,
    env: { ROSTER_API_KEY: 'test-default-key' }, configPath, fleetPath, prompts,
    question: async (prompt) => {
      prompts.push(prompt);
      assert.ok(replies.length, `Unexpected interview prompt: ${prompt}`);
      return replies.shift();
    } };
}

function chatMock(profile = proposed, { fail = false, toolCall = false } = {}) {
  const calls = [];
  const fetchImpl = async (url, request) => {
    calls.push({ url, request });
    if (url.endsWith('/models')) {
      assert.equal(url, 'https://new.example.invalid/v1/models');
      assert.equal(request.method, 'GET');
      return Response.json({ data: [{ id: 'served-model' }, { id: 'other-model' }] });
    }
    assert.equal(url, 'https://interview.example.invalid/v1/chat/completions');
    const body = JSON.parse(request.body);
    assert.equal(body.model, 'interview-model');
    assert.equal(body.tools, undefined);
    assert.match(body.messages[0].content, /Fleet endpoint interview[\s\S]*Hard rule: do not invent URLs or model IDs/);
    assert.ok(!request.body.includes('test-default-key'));
    if (fail) return new Response('private-server-error', { status: 503 });
    if (toolCall) return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
      role: 'assistant', tool_calls: [{ id: 'write', type: 'function',
        function: { name: 'write_file', arguments: '{"path":"src/unsafe.mjs","content":"bad"}' } }],
    } }] });
    const payload = JSON.parse(body.messages[1].content);
    const content = payload.stage === 'proposal' ? { profile } : { question: `What is ${payload.field}?` };
    return Response.json({ choices: [{ finish_reason: 'stop',
      message: { role: 'assistant', content: JSON.stringify(content) } }] });
  };
  return { calls, fetchImpl };
}

test('onboarded model interviews one field at a time and no/blank never saves a proposed profile', async (t) => {
  for (const confirmation of ['no', '']) {
    const options = fixture(t, [...answers(), confirmation, 'done']);
    const before = readFileSync(options.fleetPath, 'utf8');
    const mock = chatMock();
    const result = await runFleet(['assist'], { ...options, fetchImpl: mock.fetchImpl });
    assert.equal(result.exitCode, 0);
    assert.deepEqual(result.savedProfiles, []);
    assert.equal(readFileSync(options.fleetPath, 'utf8'), before);
    assert.equal(readFileSync(options.configPath, 'utf8'), configSource);
    assert.equal(existsSync(join(options.cwd, '.gitignore')), false);
    assert.match(options.output.text, /Proposed profile:\nprofiles:[\s\S]*new-endpoint/);
    assert.match(options.output.text, /Profile not written/);
    assert.ok(options.prompts.includes('Write this profile? [no] '));
    const payloads = mock.calls.filter(({ url }) => url.endsWith('/chat/completions'))
      .map(({ request }) => JSON.parse(JSON.parse(request.body).messages[1].content));
    assert.deepEqual(payloads.slice(0, 8).map(({ field }) => field),
      ['id', 'base_url', 'model', 'hardware', 'context_max', 'concurrency', 'task_class', 'notes']);
    assert.equal(payloads[8].stage, 'proposal');
  }
});

test('only explicit user yes appends a validated profile and leaves the default config unchanged', async (t) => {
  const options = fixture(t, [...answers(), 'maybe', 'yes', '/quit']);
  const mock = chatMock();
  const result = await runFleetAssist({ ...options, fetchImpl: mock.fetchImpl });
  assert.deepEqual(result.savedProfiles, ['new-endpoint']);
  assert.deepEqual((await loadFleet({ cwd: options.cwd })).profiles, [baseline, proposed]);
  assert.equal(readFileSync(options.configPath, 'utf8'), configSource);
  assert.match(options.output.text, /Please answer yes or no; nothing has been written/);
  assert.match(options.output.text, /Default config unchanged/);
});

test('invalid user URL is rejected without probing it or passing it to the default model', async (t) => {
  const options = fixture(t, ['new-endpoint', 'https://user:private-value@new.example.invalid/v1', 'done']);
  const before = readFileSync(options.fleetPath, 'utf8');
  const mock = chatMock();
  await runFleetAssist({ ...options, fetchImpl: mock.fetchImpl });
  assert.match(options.output.text, /without credentials/);
  assert.doesNotMatch(options.output.text, /private-value/);
  assert.equal(mock.calls.filter(({ url }) => url.endsWith('/models')).length, 0);
  assert.ok(mock.calls.every(({ request }) => !request.body.includes('private-value')));
  assert.equal(readFileSync(options.fleetPath, 'utf8'), before);
});

test('invented model proposal values cannot override recorded user/probe facts', async (t) => {
  const options = fixture(t, [...answers(), 'yes', 'done']);
  const mock = chatMock({ ...proposed, base_url: 'https://invented.example.invalid/v1', model: 'invented-model' });
  await runFleetAssist({ ...options, fetchImpl: mock.fetchImpl });
  assert.match(options.output.text, /invented values are refused/);
  assert.match(options.output.text, /validated template profile from recorded answers/);
  assert.doesNotMatch(options.output.text, /invented\.example\.invalid|invented-model/);
  assert.deepEqual((await loadFleet({ cwd: options.cwd })).profiles, [baseline, proposed]);
});

test('failed default endpoint and forbidden tool calls visibly fall back to template questions', async (t) => {
  for (const failed of [{ fail: true }, { toolCall: true }]) {
    const options = fixture(t, [...answers(), 'yes', 'done']);
    const mock = chatMock(proposed, failed);
    await runFleetAssist({ ...options, fetchImpl: mock.fetchImpl });
    assert.match(options.output.text, /Model interview failed/);
    assert.match(options.output.text, /Falling back to template questions without the model/);
    assert.doesNotMatch(options.output.text, /private-server-error/);
    assert.equal(mock.calls.filter(({ url }) => url.endsWith('/chat/completions')).length, 1);
    assert.deepEqual((await loadFleet({ cwd: options.cwd })).profiles, [baseline, proposed]);
    assert.equal(existsSync(join(options.cwd, 'src', 'unsafe.mjs')), false);
  }
});

test('unserved model IDs are refused and secret answers never reach model prompts or the catalog', async (t) => {
  const options = fixture(t, ['test-default-key', 'new-endpoint', 'https://new.example.invalid',
    'unserved-model', 'served-model', 'test-gpu', '32768', '2', 'fix,test', '', 'no', 'done']);
  const mock = chatMock({ ...proposed, notes: '' });
  await runFleetAssist({ ...options, fetchImpl: mock.fetchImpl });
  assert.match(options.output.text, /credentials, or secrets/);
  assert.match(options.output.text, /Choose an ID returned by this endpoint models list/);
  assert.ok(mock.calls.every(({ request }) => !request.body || !request.body.includes('test-default-key')));
  assert.equal((await loadFleet({ cwd: options.cwd })).profiles.length, 1);
});

test('a failed models probe records only the manually supplied model and reports the failure', async (t) => {
  const options = fixture(t, [...answers(), 'yes', 'done']);
  const mock = chatMock();
  await runFleetAssist({
    ...options, fetchImpl: async (url, request) => url.endsWith('/models')
      ? new Response('private-probe-error', { status: 503 }) : mock.fetchImpl(url, request),
  });
  assert.match(options.output.text, /Models probe failed: HTTP 503/);
  assert.doesNotMatch(options.output.text, /private-probe-error/);
  assert.deepEqual((await loadFleet({ cwd: options.cwd })).profiles, [baseline, proposed]);
});

test('concurrent catalog edits are refused instead of overwriting another operator profile', async (t) => {
  const options = fixture(t, answers());
  const original = options.question;
  const external = { ...proposed, id: 'external-profile' };
  await assert.rejects(runFleetAssist({
    ...options, fetchImpl: chatMock().fetchImpl,
    question: async (prompt) => {
      if (prompt === 'Write this profile? [no] ') {
        writeFileSync(options.fleetPath, formatFleet({ profiles: [baseline, external] }));
        return 'yes';
      }
      return original(prompt);
    },
  }), /changed during setup/);
  assert.deepEqual((await loadFleet({ cwd: options.cwd })).profiles, [baseline, external]);
});

test('assistance requires project onboarding config, never an installation or AI_MODEL fallback', async (t) => {
  const options = fixture(t, []);
  rmSync(options.configPath);
  await assert.rejects(runFleetAssist({
    ...options, env: { AI_MODEL: 'not-the-onboarded-model' },
    fetchImpl: () => assert.fail('Missing project config must not call an endpoint'),
  }), /requires \.roster\/config\.yml/);
  writeFileSync(options.configPath, example);
  await assert.rejects(runFleetAssist({
    ...options, fetchImpl: () => assert.fail('Stub configuration must not call an endpoint'),
  }), /requires an onboarded vLLM/);
});

test('non-TTY fleet assist exits 2 without loading config, probing or writing files', async (t) => {
  const options = fixture(t, []);
  const before = readFileSync(options.fleetPath, 'utf8');
  const result = await runFleetAssist({
    ...options, input: { isTTY: false },
    fetchImpl: () => assert.fail('Non-TTY must not contact an endpoint'),
  });
  assert.equal(result.exitCode, 2);
  assert.equal(options.errorOutput.text, 'roster fleet assist needs a terminal\n');
  const cli = spawnSync(process.execPath, [join(installation, 'src', 'cli.mjs'), 'fleet', 'assist'], {
    cwd: options.cwd, encoding: 'utf8', input: '', timeout: 10_000,
  });
  assert.ifError(cli.error);
  assert.equal(cli.status, 2);
  assert.equal(cli.stderr, 'roster fleet assist needs a terminal\n');
  assert.equal(readFileSync(options.fleetPath, 'utf8'), before);
});

test('native TTY can stop with /quit or Ctrl+C without any unconfirmed write', async (t) => {
  for (const response of ['/quit\n', '\x03']) {
    const options = fixture(t, []);
    const input = new PassThrough();
    input.isTTY = true;
    input.setRawMode = () => {};
    const output = new PassThrough();
    output.isTTY = true;
    output.columns = 80;
    let text = '';
    let send;
    let timer;
    const prompted = new Promise((resolve, reject) => {
      timer = setTimeout(() => reject(new Error('Native interview did not ask for an ID')), 2_000);
      send = () => { clearTimeout(timer); resolve(); };
    });

    output.on('data', (chunk) => {
      text += chunk.toString();
      if (text.includes('What is id?')) send();
    });
    const before = readFileSync(options.fleetPath, 'utf8');
    const done = runFleetAssist({ ...options, question: undefined, input, output, fetchImpl: chatMock().fetchImpl });
    try {
      await prompted;
      input.write(response);
      assert.equal((await done).exitCode, 0);
      assert.equal(readFileSync(options.fleetPath, 'utf8'), before);
    } finally {
      clearTimeout(timer);
      input.destroy();
      output.destroy();
    }
  }
});

test('Ctrl+C cancels an in-flight default-model request without leaving a timer or a write', async (t) => {
  const options = fixture(t, []);
  const input = new PassThrough();
  input.isTTY = true;
  input.setRawMode = () => {};
  const output = new PassThrough();
  output.isTTY = true;
  let started;
  const pending = new Promise((resolve) => { started = resolve; });
  let signal;
  const done = runFleetAssist({
    ...options, question: undefined, input, output,
    fetchImpl: async (_url, request) => {
      signal = request.signal;
      started();
      return await new Promise(() => {});
    },
  });
  try {
    await pending;
    input.write('\x03');
    assert.equal((await done).exitCode, 0);
    assert.equal(signal.aborted, true);
    assert.deepEqual((await loadFleet({ cwd: options.cwd })).profiles, [baseline]);
  } finally {
    input.destroy();
    output.destroy();
  }
});
