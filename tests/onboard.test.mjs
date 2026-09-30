import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync,
  rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { PassThrough } from 'node:stream';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { loadConfig, parseConfig } from '../src/lib/config.mjs';
import { probeModels, runOnboard } from '../src/onboard/wizard.mjs';

const installation = fileURLToPath(new URL('../', import.meta.url));
const example = readFileSync(join(installation, 'roster.config.example.yml'), 'utf8');

function capture() {
  let text = '';
  return { isTTY: true, write(value) { text += String(value); }, get text() { return text; } };
}

function fixture(t, answers = []) {
  const root = mkdtempSync(join(tmpdir(), 'roster-onboard-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const cwd = join(root, 'project');
  const installationRoot = join(root, 'install');
  mkdirSync(cwd);
  mkdirSync(join(installationRoot, 'vendor', 'github-agent-contracts', 'scripts'), { recursive: true });
  writeFileSync(join(installationRoot, 'roster.config.example.yml'), example);
  writeFileSync(join(installationRoot, 'vendor', 'github-agent-contracts', 'scripts', 'agent-pr.mjs'),
    'export {};\n');
  const output = capture();
  const errorOutput = capture();
  const prompts = [];
  return {
    cwd, installationRoot, output, errorOutput, prompts,
    input: { isTTY: true }, env: {},
    question: async (prompt) => {
      prompts.push(prompt);
      assert.ok(answers.length, `Unexpected onboarding question: ${prompt}`);
      return answers.shift();
    },
  };
}

function models(...ids) {
  return async () => Response.json({ data: ids.map((id) => ({ id })) });
}

for (const platform of ['win32', 'linux', 'darwin']) {
  test(`${platform} wizard probes real model IDs and saves defaults without an Advanced section`, async (t) => {
    const options = fixture(t, ['', '2', '', '', '', '', '']);
    let calls = 0;
    const env = { GITHUB_APP_ID: 'hidden-app-id', GITHUB_APP_PRIVATE_KEY_PATH: 'hidden-key-path',
      ROSTER_API_KEY: 'hidden-api-key' };
    const result = await runOnboard({
      ...options, platform, env,
      fetchImpl: async (url, request) => {
        calls += 1;
        assert.equal(url, 'http://127.0.0.1:8000/v1/models');
        assert.equal(request.method, 'GET');
        assert.equal(request.redirect, 'error');
        assert.equal(request.headers.Authorization, undefined);
        assert.ok(request.signal instanceof AbortSignal);
        return Response.json({ data: [{ id: 'owner/first-model' }, { id: 'owner/chosen-model' }] });
      },
    });
    assert.equal(result.exitCode, 0);
    assert.equal(result.saved, true);
    assert.equal(result.modelsProbed, true);
    assert.equal(calls, 1);
    const config = loadConfig({ repoRoot: options.installationRoot, cwd: options.cwd });
    assert.deepEqual(config.llm, {
      base_url: 'http://127.0.0.1:8000/v1', model: 'owner/chosen-model',
      api_key_env: 'ROSTER_API_KEY', effort: 'm', context_max: 0,
      profile: 'vllm-local', api_key_optional: true, provider: 'vllm',
    });
    assert.deepEqual(config.publish, { enabled: true });
    assert.deepEqual(config.tools, { internet: false, run_test: true });
    assert.deepEqual(config.reviewer, { required: true });
    assert.deepEqual(config.review, { required: true });
    assert.deepEqual(config.loop, { turns: 12 });
    assert.deepEqual(config.context, { budget: 8000 });
    assert.equal(config.seat.turn_budget, 12);
    assert.doesNotMatch(readFileSync(result.configPath, 'utf8'), /^  internet:/m);
    assert.match(options.output.text, /These flags do not grant contracts policy\./);
    assert.match(options.output.text, new RegExp(`OS: ${platform}`));
    assert.match(options.output.text, /1\. Platform[\s\S]*2\. LLM endpoint[\s\S]*3\. Permissions[\s\S]*4\. Review/);
    assert.equal(options.prompts.at(-1), 'Confirm write .roster/config.yml? [yes] ');
    assert.match(options.output.text, /Available models:\n  1\. owner\/first-model\n  2\. owner\/chosen-model/);
    assert.doesNotMatch(options.output.text, /\nAdvanced\n/);
    assert.equal(options.prompts.some((prompt) => prompt.startsWith('Internet search')), false);
    assert.match(options.output.text, /Doctor\nOK Node\.js >=20/);
    assert.match(options.output.text, /FAIL agent-policy\.yml/);
    assert.match(options.output.text, /not a grant to publish/);
    assert.match(options.output.text, /App env present/);
    assert.match(options.output.text, /App private-key file is missing/);
    for (const secret of Object.values(env)) {
      assert.ok(!options.output.text.includes(secret));
      assert.ok(!readFileSync(result.configPath, 'utf8').includes(secret));
    }
    assert.match(readFileSync(join(options.cwd, '.gitignore'), 'utf8'), /\.roster\/config\.yml/);
    assert.deepEqual(readdirSync(join(options.cwd, '.roster')), ['config.yml']);
    assert.equal(options.errorOutput.text, '');
  });
}

test('WSL can use a Windows host URL and choose no permissions plus Advanced internet off', async (t) => {
  const options = fixture(t, ['http://172.30.96.1:8000/v1/', '1', 'no', 'n', 'no', 'yes', 'no', '', '', '']);
  const result = await runOnboard({
    ...options, platform: 'linux',
    fetchImpl: async (url) => {
      assert.equal(url, 'http://172.30.96.1:8000/v1/models');
      return Response.json({ data: [{ id: 'served-model' }] });
    },
  });
  assert.equal(result.config.llm.base_url, 'http://172.30.96.1:8000/v1/');
  assert.deepEqual(result.config.publish, { enabled: false });
  assert.deepEqual(result.config.tools, { internet: false, run_test: false });
  assert.deepEqual(result.config.reviewer, { required: false });
  assert.match(options.output.text, /\nAdvanced\n/);
  assert.ok(options.prompts.some((prompt) => prompt.startsWith('Internet search')));
  assert.match(options.output.text, /no live internet tool/);
  assert.doesNotMatch(options.output.text, /App identity \(environment only\)/);
  assert.match(options.output.text, /Windows host IP, not localhost/);
});

test('Advanced defaults internet on and stores effective loop/context limits after validation', async (t) => {
  const options = fixture(t, ['', '1', '', '', '', 'yes', '', '0', '16', '0', '12000', '']);
  const result = await runOnboard({ ...options, fetchImpl: models('served-model') });
  assert.equal(result.config.tools.internet, true);
  assert.equal(result.config.loop.turns, 16);
  assert.equal(result.config.context.budget, 12000);
  assert.equal(result.config.seat.turn_budget, 16);
  assert.equal(result.config.seat.context_chars, 12000);
  assert.match(options.output.text, /Enter an integer from 1 to 64/);
  assert.match(options.output.text, /Enter a positive safe integer/);
  const permissionQuestions = options.prompts.filter((prompt) =>
    /^(Allow publish|Require reviewer|Allow run_test|Show advanced)/.test(prompt));
  assert.deepEqual(permissionQuestions, [
    'Allow publish through GitHub App? [yes] ', 'Require reviewer before publish? [yes] ',
    'Allow run_test? [yes] ', 'Show advanced settings? [no] ',
  ]);
  const raw = readFileSync(result.configPath, 'utf8');
  assert.match(raw, /^review:\n  required: true$/m);
  assert.doesNotMatch(raw, /^reviewer:/m);
  assert.match(raw, /^  internet: true$/m);
});

test('model selection defaults to the first actual ID in an OpenAI models list', async (t) => {
  const options = fixture(t, ['', '', '', '', '', '', '']);
  const result = await runOnboard({ ...options, fetchImpl: models('first-model', 'second-model') });
  assert.equal(result.config.llm.model, 'first-model');
  assert.equal(result.config.llm.provider, 'vllm');
  assert.equal(result.config.llm.profile, 'vllm-local');
  assert.equal(result.config.llm.api_key_optional, true);
});

test('failed models probe still saves a supplied actual model and never invents one', async (t) => {
  const options = fixture(t, ['', '', 'unknown', 'owner/manual-model', '', '', '', '', '']);
  const result = await runOnboard({
    ...options,
    fetchImpl: async () => { throw new Error('private-fetch-secret must not be printed'); },
  });
  assert.equal(result.saved, true);
  assert.equal(result.modelsProbed, false);
  assert.equal(result.config.llm.model, 'owner/manual-model');
  assert.match(options.output.text, /Model probe failed/);
  assert.match(options.output.text, /model supplied manually/);
  assert.match(options.output.text, /App env missing[\s\S]*setx GITHUB_APP_ID/);
  assert.doesNotMatch(options.output.text, /private-fetch-secret/);
  assert.equal(options.prompts.filter((prompt) => prompt === 'Model ID [required]: ').length, 3);
});

test('models timeout is exactly 5000ms and bounds an unresponsive fetch', async () => {
  let expire;
  let signal;
  let cancelled = false;
  const timer = {};
  const pending = probeModels('http://localhost:8000/v1', {
    env: {},
    fetchImpl: async (_url, request) => {
      signal = request.signal;
      return await new Promise(() => {});
    },
    schedule(callback, delay) {
      assert.equal(delay, 5_000);
      expire = callback;
      return timer;
    },
    cancel(value) { assert.equal(value, timer); cancelled = true; },
  });
  expire();
  await assert.rejects(pending, (error) => error.message === 'timeout');
  assert.equal(signal.aborted, true);
  assert.equal(cancelled, true);
});

test('invalid or secret-like discovery data is an explicit probe failure', async () => {
  for (const payload of [{}, { data: [] }, { data: [{ id: 'unknown' }] },
    { data: [{ id: 'bad\nid' }] }, { data: [{ id: 'private-model-secret' }] }]) {
    await assert.rejects(probeModels('http://localhost:8000/v1', {
      fetchImpl: async () => Response.json(payload), env: { ROSTER_API_KEY: 'private-model-secret' },
    }), /invalid response/);
  }
  await assert.rejects(probeModels('http://localhost:8000/v1', {
    fetchImpl: async () => new Response('not-json', { status: 503 }), env: {},
  }), /HTTP 503/);
  await assert.rejects(probeModels('http://localhost:8000/v1', {
    fetchImpl: models('custom-secret-value'), env: { CUSTOM_LLM: 'custom-secret-value' },
    apiKeyEnv: 'CUSTOM_LLM',
  }), /invalid response/);
});

test('probe failures report only a safe class and still accept a supplied real model', async (t) => {
  for (const [category, fetchImpl] of [
    ['refused', async () => { throw new TypeError('private-fetch-details', {
      cause: Object.assign(new Error('private-server-details'), { code: 'ECONNREFUSED' }),
    }); }],
    ['HTTP 401', async () => new Response('private-auth-details', { status: 401 })],
    ['invalid response', async () => new Response('private-json-details', { status: 200 })],
  ]) {
    const options = fixture(t, ['', 'manual-model', '', '', '', '', '']);
    const result = await runOnboard({ ...options, fetchImpl });
    assert.equal(result.config.llm.model, 'manual-model');
    assert.ok(options.output.text.includes(`Model probe failed: ${category}\n`));
    assert.doesNotMatch(options.output.text, /private-(?:fetch|server|auth|json)-details/);
  }
});

test('URL validation retries without echoing credentials and invalid selections do not choose an invented model', async (t) => {
  const options = fixture(t, ['http://user:do-not-print@host:8000/v1',
    'http://localhost:8000/v1/chat/completions', '', '9', '1', 'maybe', 'yes', '', '', '', '']);
  const result = await runOnboard({ ...options, fetchImpl: models('served-model') });
  assert.equal(result.config.llm.base_url, 'http://127.0.0.1:8000/v1');
  assert.equal(result.config.llm.model, 'served-model');
  assert.match(options.output.text, /without credentials/);
  assert.match(options.output.text, /not a full \/models or \/chat\/completions endpoint/);
  assert.match(options.output.text, /listed model number/);
  assert.match(options.output.text, /answer yes or no/);
  assert.doesNotMatch(options.output.text, /do-not-print/);
});

test('existing config needs confirmation and keeps unrelated settings on a confirmed rerun', async (t) => {
  const kept = fixture(t, ['']);
  mkdirSync(join(kept.cwd, '.roster'));
  writeFileSync(join(kept.cwd, '.roster', 'config.yml'), example);
  const cancelled = await runOnboard({
    ...kept, fetchImpl: () => assert.fail('Kept config must not probe'),
  });
  assert.equal(cancelled.saved, false);
  assert.equal(readFileSync(join(kept.cwd, '.roster', 'config.yml'), 'utf8'), example);
  assert.equal(existsSync(join(kept.cwd, '.gitignore')), false);

  const changed = fixture(t, ['yes', '', '1', '', '', '', '', '']);
  mkdirSync(join(changed.cwd, '.roster'));
  writeFileSync(join(changed.cwd, '.roster', 'config.yml'),
    example.replace('context_chars: 8000', 'context_chars: 16000').replace('effort: m', 'effort: h'));
  const result = await runOnboard({ ...changed, fetchImpl: models('selected-model') });
  assert.equal(result.config.seat.context_chars, 16000);
  assert.equal(result.config.llm.effort, 'h');
  assert.equal(parseConfig(readFileSync(result.configPath, 'utf8')).llm.model, 'selected-model');
});

test('declining the final review writes neither config nor ignore rules', async (t) => {
  const options = fixture(t, ['', '1', '', '', '', '', 'no']);
  const result = await runOnboard({ ...options, fetchImpl: models('served-model') });
  assert.equal(result.exitCode, 0);
  assert.equal(result.saved, false);
  assert.match(options.output.text, /4\. Review[\s\S]*Model: served-model/);
  assert.equal(existsSync(join(options.cwd, '.roster', 'config.yml')), false);
  assert.equal(existsSync(join(options.cwd, '.gitignore')), false);
});

test('onboard from a nested Git directory writes ignored config at that worktree root', async (t) => {
  const options = fixture(t, ['', '1', '', '', '', '', '']);
  const nested = join(options.cwd, 'nested');
  mkdirSync(nested);
  copyFileSync(join(installation, '.gitignore'), join(options.cwd, '.gitignore'));
  const before = readFileSync(join(options.cwd, '.gitignore'), 'utf8');
  execFileSync('git', ['init', '--quiet'], { cwd: options.cwd, stdio: 'pipe' });
  const result = await runOnboard({ ...options, cwd: nested, fetchImpl: models('served-model') });
  assert.equal(result.configPath, join(options.cwd, '.roster', 'config.yml'));
  assert.equal(readFileSync(join(options.cwd, '.gitignore'), 'utf8'), before);
  assert.doesNotThrow(() => execFileSync('git', ['check-ignore', '--quiet', '.roster/config.yml'], {
    cwd: options.cwd, stdio: 'pipe',
  }));
  assert.equal(loadConfig({ repoRoot: options.installationRoot, cwd: nested }).llm.model, 'served-model');
  assert.equal(existsSync(join(nested, '.roster')), false);
});

test('a tracked private config is never overwritten by onboarding', async (t) => {
  const options = fixture(t, ['yes', '', '1', '', '', '', '', '']);
  mkdirSync(join(options.cwd, '.roster'));
  writeFileSync(join(options.cwd, '.roster', 'config.yml'), example);
  execFileSync('git', ['init', '--quiet'], { cwd: options.cwd, stdio: 'pipe' });
  execFileSync('git', ['add', '.roster/config.yml'], { cwd: options.cwd, stdio: 'pipe' });
  await assert.rejects(runOnboard({ ...options, fetchImpl: models('served-model') }), /tracked by Git/);
  assert.equal(readFileSync(join(options.cwd, '.roster', 'config.yml'), 'utf8'), example);
});

test('non-TTY onboard prints the required message and exits 2 without writing or probing', async (t) => {
  const options = fixture(t);
  const result = await runOnboard({
    ...options, input: { isTTY: false },
    fetchImpl: () => assert.fail('Non-TTY must not probe'),
  });
  assert.equal(result.exitCode, 2);
  assert.equal(options.errorOutput.text, 'roster onboard needs a terminal\n');
  assert.deepEqual(readdirSync(options.cwd), []);
  const cli = spawnSync(process.execPath, [join(installation, 'src', 'cli.mjs'), 'onboard'], {
    cwd: options.cwd, encoding: 'utf8', input: '', timeout: 10_000,
  });
  assert.ifError(cli.error);
  assert.equal(cli.status, 2);
  assert.equal(cli.stderr, 'roster onboard needs a terminal\n');
  assert.equal(cli.stdout, '');
  assert.deepEqual(readdirSync(options.cwd), []);
});

test('real readline TTY handles answers and Ctrl+C without an injected question function', async (t) => {
  for (const cancel of [false, true]) {
    const options = fixture(t);
    const input = new PassThrough();
    input.isTTY = true;
    input.setRawMode = () => {};
    const output = new PassThrough();
    output.isTTY = true;
    output.columns = 100;
    let text = '';
    output.on('data', (chunk) => { text += chunk.toString(); });
    function waitFor(fragment) {
      return new Promise((resolve, reject) => {
        if (text.includes(fragment)) return resolve();
        const timer = setTimeout(() => {
          output.off('data', check);
          reject(new Error(`Missing TTY prompt: ${fragment}`));
        }, 2_000);
        function check() {
          if (!text.includes(fragment)) return;
          clearTimeout(timer);
          output.off('data', check);
          resolve();
        }
        output.on('data', check);
      });
    }
    const done = runOnboard({ ...options, input, output, question: undefined, fetchImpl: models('served-model') });
    try {
      if (cancel) {
        await waitFor('vLLM base URL [');
        input.write('\x03');
        assert.equal((await done).exitCode, 0);
        assert.equal(existsSync(join(options.cwd, '.roster', 'config.yml')), false);
        assert.equal(existsSync(join(options.cwd, '.gitignore')), false);
        assert.match(text, /Onboarding cancelled/);
      } else {
        for (const prompt of ['vLLM base URL [', 'Select a model [', 'Allow publish through GitHub App?',
          'Require reviewer before publish?', 'Allow run_test?', 'Show advanced settings?', 'Confirm write .roster/config.yml?']) {
          await waitFor(prompt);
          input.write('\n');
        }
        assert.equal((await done).exitCode, 0);
        assert.match(text, /Saved private config/);
      }
    } finally {
      input.destroy();
      output.destroy();
    }
  }
});
