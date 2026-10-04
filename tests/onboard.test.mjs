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
import { checkDoctor } from '../src/lib/doctor.mjs';
import { formatFleet, loadFleet } from '../src/lib/fleet.mjs';
import { probeModelDetails, probeModels, runOnboard } from '../src/onboard/wizard.mjs';

const installation = fileURLToPath(new URL('../', import.meta.url));
const example = readFileSync(join(installation, 'roster.config.example.yml'), 'utf8');
const sglangModels = JSON.parse(readFileSync(new URL('./fixtures/models-sglang.json', import.meta.url), 'utf8'));

function capture() {
  let text = '';
  return { isTTY: true, write(value) { text += String(value); }, get text() { return text; } };
}

function fixture(t, answers = [], contextMax = 0, continueSetup = true) {
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
      if (prompt.startsWith('Model context tokens')) return String(contextMax);
      if (prompt === 'Add more endpoints later with roster fleet add. Continue? [yes] ') {
        return continueSetup ? '' : 'no';
      }
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
        assert.equal(request.headers.Authorization, `Bearer ${env.ROSTER_API_KEY}`);
        assert.ok(request.signal instanceof AbortSignal);
        return Response.json({ data: [{ id: 'owner/first-model' }, { id: 'owner/chosen-model' }] });
      },
    });
    assert.equal(result.exitCode, 1, 'Opted-in publish prerequisites are missing in this fixture');
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
    assert.deepEqual(config.context, { budget: 200000 });
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
    assert.deepEqual(readdirSync(join(options.cwd, '.roster')).sort(), ['config.yml', 'fleet.yml']);
    const fleet = await loadFleet({ cwd: options.cwd });
    assert.equal(fleet.profiles[0].id, 'default');
    assert.equal(fleet.profiles[0].base_url, config.llm.base_url);
    assert.equal(fleet.profiles[0].model, config.llm.model);
    assert.equal(fleet.profiles[0].context_max, 0);
    assert.equal(fleet.profiles[0].concurrency, 1);
    assert.match(options.output.text, /Saved fleet:/);
    assert.equal(options.errorOutput.text, '');
  });
}

test('simulated Enter accepts bracket defaults including Continue and writes the default profile', async (t) => {
  for (const blank of ['', ' \t ']) {
    for (const advanced of [false, true]) {
      const options = fixture(t);
      const prompts = [];
      const result = await runOnboard({
        ...options, fetchImpl: models('served-model'),
        question: async (prompt) => {
          prompts.push(prompt);
          return advanced && prompt === 'Show advanced settings? [no] ' ? 'yes' : blank;
        },
      });
      assert.equal(result.saved, true);
      assert.equal(result.config.llm.base_url, 'http://127.0.0.1:8000/v1');
      assert.equal(result.config.llm.model, 'served-model');
      assert.equal(result.config.llm.context_max, 0);
      assert.equal(result.config.publish.enabled, true);
      assert.equal(result.config.review.required, true);
      assert.equal(result.config.tools.run_test, true);
      assert.equal(result.config.tools.internet, advanced);
      assert.equal(result.config.loop.turns, 12);
      assert.equal(result.config.context.budget, advanced ? 8000 : 200000);
      assert.equal(prompts.filter((prompt) =>
        prompt === 'Add more endpoints later with roster fleet add. Continue? [yes] ').length, 1);
      assert.equal(prompts.filter((prompt) => prompt === 'Confirm write .roster/config.yml? [yes] ').length, 1);
      assert.equal(prompts.some((prompt) => prompt.startsWith('Max tool turns')), advanced);
      assert.deepEqual((await loadFleet({ cwd: options.cwd })).profiles, [{
        id: 'default', base_url: 'http://127.0.0.1:8000/v1', model: 'served-model',
        provider: 'vllm', context_max: 0, concurrency: 1, hardware: 'unspecified',
        notes: 'Default endpoint selected during onboarding.',
      }]);
      assert.doesNotMatch(options.output.text, /Please answer yes or no|Enter an integer|Enter a listed model/);
    }
  }
});

test('invalid nonempty yes/no input retries but Enter then accepts Continue yes', async (t) => {
  const options = fixture(t);
  let continueQuestions = 0;
  const result = await runOnboard({
    ...options, fetchImpl: models('served-model'), question: async (prompt) => {
      if (prompt === 'Add more endpoints later with roster fleet add. Continue? [yes] ') {
        continueQuestions += 1;
        if (continueQuestions === 1) return 'maybe';
      }
      return '';
    },
  });
  assert.equal(result.saved, true);
  assert.equal(continueQuestions, 2);
  assert.equal((options.output.text.match(/Please answer yes or no/g) ?? []).length, 1);
  assert.equal((await loadFleet({ cwd: options.cwd })).profiles[0].id, 'default');
});

test('required model input has no bracket default when discovery fails', async (t) => {
  const options = fixture(t);
  let modelQuestions = 0;
  const result = await runOnboard({
    ...options, fetchImpl: async () => new Response('', { status: 503 }),
    question: async (prompt) => {
      if (prompt === 'Model ID [required]: ') {
        modelQuestions += 1;
        return modelQuestions === 1 ? '' : 'operator-model';
      }
      return '';
    },
  });
  assert.equal(modelQuestions, 2);
  assert.equal(result.saved, true);
  assert.equal(result.config.llm.model, 'operator-model');
  assert.match(options.output.text, /Enter the actual single-line served model ID/);
});

test('SGLang max_model_len sets the selected model context_max without asking for tokens', async (t) => {
  const options = fixture(t, ['', '2', 'no', '', '', '', ''], 16384);
  let requests = 0;
  const result = await runOnboard({ ...options, fetchImpl: async () => {
    requests += 1;
    return Response.json(sglangModels);
  } });
  assert.equal(result.saved, true);
  assert.equal(result.exitCode, 0);
  assert.equal(requests, 1);
  assert.equal(result.config.llm.model, 'chosen-model');
  assert.equal(result.config.llm.context_max, 1048576);
  assert.equal(result.config.seat.context_chars, 200000);
  assert.equal(result.fleet.profiles[0].context_max, 1048576);
  assert.equal(loadConfig({ repoRoot: options.installationRoot, cwd: options.cwd }).llm.context_max, 1048576);
  assert.equal((await loadFleet({ cwd: options.cwd })).profiles[0].context_max, 1048576);
  assert.equal(options.prompts.some((prompt) => prompt.startsWith('Model context tokens')), false);
  assert.match(options.output.text, /Using context_max=1048576 reported by \/v1\/models for chosen-model/);
});

test('a selected model without context metadata still asks even if another model reports a limit', async (t) => {
  const options = fixture(t, ['', '1', 'no', '', '', '', ''], 16384);
  const result = await runOnboard({ ...options, fetchImpl: async () => Response.json({
    data: [{ id: 'missing-context-model' }, { id: 'other-model', max_model_len: 1048576 }],
  }) });
  assert.equal(result.config.llm.model, 'missing-context-model');
  assert.equal(result.config.llm.context_max, 16384);
  assert.equal(result.fleet.profiles[0].context_max, 16384);
  assert.equal(options.prompts.filter((prompt) => prompt.startsWith('Model context tokens')).length, 1);
  assert.doesNotMatch(options.output.text, /Using context_max=/);
});

test('nonpositive, fractional, string, or unsafe API context metadata keeps the token question', async (t) => {
  for (const value of [0, -1, 1048576.5, '1048576', null, Number.MAX_SAFE_INTEGER + 1]) {
    const options = fixture(t, ['', '1', 'no', '', '', '', ''], 16384);
    const result = await runOnboard({ ...options, fetchImpl: async () => Response.json({
      data: [{ id: 'served-model', max_model_len: value }],
    }) });
    assert.equal(result.config.llm.context_max, 16384);
    assert.equal(options.prompts.filter((prompt) => prompt.startsWith('Model context tokens')).length, 1);
  }
});

test('model details retain only positive context limits while the ID-only probe stays compatible', async () => {
  const fetchImpl = async () => Response.json(sglangModels);
  assert.deepEqual(await probeModelDetails('http://localhost:8000/v1', { env: {}, fetchImpl }), [
    { id: 'first-model', context_max: 8192 }, { id: 'chosen-model', context_max: 1048576 },
  ]);
  assert.deepEqual(await probeModels('http://localhost:8000/v1', { env: {}, fetchImpl }),
    ['first-model', 'chosen-model']);
  for (const field of ['max_model_len', 'max_context_length', 'max_context_len',
    'context_length', 'context_window', 'context_max']) {
    const models = await probeModelDetails('http://localhost:8000/v1', {
      env: {}, fetchImpl: async () => Response.json({ data: [{ id: 'served-model', [field]: 1048576,
        private_metadata: 'test-only-secret' }] }),
    });
    assert.deepEqual(models, [{ id: 'served-model', context_max: 1048576 }]);
    assert.equal(Object.isFrozen(models[0]), true);
  }
  assert.deepEqual(await probeModelDetails('http://localhost:8000/v1', {
    env: {}, fetchImpl: async () => Response.json({ data: [
      { id: 'served-model' }, { id: 'served-model', max_model_len: 1048576 }, { id: 'other-model', max_tokens: 1000 },
    ] }),
  }), [{ id: 'served-model', context_max: 1048576 }, { id: 'other-model' }]);
});

test('WSL can use a Windows host URL and choose no permissions plus Advanced internet off', async (t) => {
  const options = fixture(t, ['http://172.30.96.1:8000/v1/', '1', 'no', 'n', 'no', 'yes', 'no', '', '', '']);
  const result = await runOnboard({
    ...options, platform: 'linux',
    fetchImpl: async (url) => {
      assert.equal(url, 'http://172.30.96.1:8000/v1/models');
      return Response.json({ data: [{ id: 'served-model' }] });
    },
  });
  assert.equal(result.config.llm.base_url, 'http://172.30.96.1:8000/v1');
  assert.deepEqual(result.config.publish, { enabled: false });
  assert.deepEqual(result.config.tools, { internet: false, run_test: false });
  assert.deepEqual(result.config.reviewer, { required: false });
  assert.equal(result.exitCode, 0, 'Run-only setup does not need publishing policy or App credentials');
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
  assert.equal(signal, undefined);
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
  for (const fetchImpl of [
    async () => { throw new TypeError('private-fetch-details', {
      cause: Object.assign(new Error('private-server-details'), { code: 'ECONNREFUSED' }),
    }); },
    async () => new Response('private-auth-details', { status: 401 }),
    async () => new Response('private-json-details', { status: 200 }),
  ]) {
    const options = fixture(t, ['', 'manual-model', '', '', '', '', '']);
    const result = await runOnboard({ ...options, fetchImpl });
    assert.equal(result.config.llm.model, 'manual-model');
    assert.match(options.output.text, /Models probe: failed; model supplied manually/);
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
    example.replace('context_chars: 200000', 'context_chars: 16000').replace('effort: m', 'effort: h'));
  const result = await runOnboard({ ...changed, fetchImpl: models('selected-model') });
  assert.equal(result.config.seat.context_chars, 16000);
  assert.equal(result.config.llm.effort, 'h');
  assert.equal(parseConfig(readFileSync(result.configPath, 'utf8')).llm.model, 'selected-model');
});

test('declining the final review writes neither config nor ignore rules', async (t) => {
  const options = fixture(t, ['', '1', '', '', '', '', 'no']);
  const result = await runOnboard({
    ...options, fetchImpl: models('served-model'),
    doctor: () => assert.fail('Doctor must not run before confirmation'),
  });
  assert.equal(result.exitCode, 0);
  assert.equal(result.saved, false);
  assert.match(options.output.text, /4\. Review[\s\S]*Model: served-model/);
  assert.equal(existsSync(join(options.cwd, '.roster', 'config.yml')), false);
  assert.equal(existsSync(join(options.cwd, '.roster', 'fleet.yml')), false);
  assert.equal(existsSync(join(options.cwd, '.gitignore')), false);
});

test('onboarding seeds known context and preserves other registered endpoints', async (t) => {
  const options = fixture(t, ['', '1', 'no', '', '', '', ''], 65536);
  mkdirSync(join(options.cwd, '.roster'));
  const other = { id: 'other-endpoint', base_url: 'https://other.example.invalid/v1',
    model: 'owner/other-model', provider: 'vllm', context_max: 32768, concurrency: 2,
    hardware: 'example-gpu', notes: '' };
  writeFileSync(join(options.cwd, '.roster', 'fleet.yml'), formatFleet({ profiles: [other] }));
  const result = await runOnboard({ ...options, fetchImpl: models('served-model') });
  assert.equal(result.config.llm.context_max, 65536);
  assert.equal(result.fleet.profiles[0].context_max, 65536);
  assert.equal(result.fleet.profiles[0].id, 'default');
  assert.deepEqual(result.fleet.profiles[1], other);
  assert.deepEqual(await loadFleet({ cwd: options.cwd }), result.fleet);
});

test('declining the fleet Continue question writes no private settings or ignore rules', async (t) => {
  const options = fixture(t, ['', '1'], 0, false);
  const result = await runOnboard({ ...options, fetchImpl: models('served-model') });
  assert.equal(result.saved, false);
  assert.equal(result.exitCode, 0);
  assert.equal(existsSync(join(options.cwd, '.roster')), false);
  assert.equal(existsSync(join(options.cwd, '.gitignore')), false);
  assert.equal(options.prompts.at(-1), 'Add more endpoints later with roster fleet add. Continue? [yes] ');
});

test('confirmed onboarding runs doctor in-process after saving and reports required failures', async (t) => {
  const options = fixture(t, ['', '1', '', '', '', '', '']);
  let calls = 0;
  const result = await runOnboard({
    ...options, fetchImpl: models('served-model'),
    doctor(args) {
      calls += 1;
      assert.equal(existsSync(join(options.cwd, '.roster', 'config.yml')), true);
      assert.equal(args.cwd, options.cwd);
      return checkDoctor(args);
    },
  });
  assert.equal(calls, 1);
  assert.equal(result.saved, true);
  assert.equal(result.exitCode, 1);
  assert.equal(result.doctor.ok, false);
  assert.match(options.output.text, /FAIL GITHUB_APP_ID/);
  assert.equal(loadConfig({ repoRoot: options.installationRoot, cwd: options.cwd }).llm.model, 'served-model');
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

test('real readline TTY accepts LF/CRLF Enter defaults and Ctrl+C without an injected question function', async (t) => {
  for (const [cancel, lineEnding] of [[false, '\n'], [false, '\r\n'], [true, '\n']]) {
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
        assert.equal(existsSync(join(options.cwd, '.roster', 'fleet.yml')), false);
        assert.equal(existsSync(join(options.cwd, '.gitignore')), false);
        assert.match(text, /Onboarding cancelled/);
      } else {
        for (const prompt of ['vLLM base URL [', 'Select a model [', 'Model context tokens',
          'Add more endpoints later with roster fleet add. Continue? [yes]', 'Allow publish through GitHub App?',
          'Require reviewer before publish?', 'Allow run_test?', 'Show advanced settings?', 'Confirm write .roster/config.yml?']) {
          await waitFor(prompt);
          input.write((prompt === 'Allow publish through GitHub App?' ? 'no' : '') + lineEnding);
        }
        assert.equal((await done).exitCode, 0);
        assert.match(text, /Saved private config/);
        assert.equal((await loadFleet({ cwd: options.cwd })).profiles[0].id, 'default');
        assert.equal((await loadFleet({ cwd: options.cwd })).profiles[0].model, 'served-model');
        assert.doesNotMatch(text, /Please answer yes or no/);
      }
    } finally {
      input.destroy();
      output.destroy();
    }
  }
});
