import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { PassThrough } from 'node:stream';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { parseConfig } from '../src/lib/config.mjs';
import { buildPublishMessage } from '../src/lib/publication.mjs';
import { createDispatcher, startRepl } from '../src/repl.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const cli = join(root, 'src', 'cli.mjs');
const config = parseConfig(readFileSync(join(root, 'roster.config.example.yml'), 'utf8'));
const cwd = join(tmpdir(), 'roster-repl-project');

function capture() {
  let text = '';
  return { write(value) { text += String(value); }, get text() { return text; } };
}

function dispatcher({ env = {}, services = {}, config: activeConfig = config } = {}) {
  const output = capture();
  const errorOutput = capture();
  const commands = createDispatcher({
    cwd, repoRoot: root, config: activeConfig, env, output, errorOutput,
    services: { repositoryRoot: () => cwd, ...services },
  });
  return { ...commands, output, errorOutput };
}

test('slash dispatcher calls existing services and keeps one run in the shell', async () => {
  const calls = [];
  let activeConfig = config;
  const vault = {
    async list() { calls.push(['vault-list']); return ['ROSTER_TOKEN']; },
    async set(name, value) { calls.push(['vault-set', name, value]); },
    async get(name) { calls.push(['vault-get', name]); return 'private-value'; },
  };
  const { dispatch, state, output, errorOutput, banner } = dispatcher({
    services: {
      submitAsk: async (ask) => {
        calls.push(['ask', ask]);
        return { mode: 'draft', askPath: 'ask.md', recipePath: 'RECIPE.yml',
          taskPath: 'TASK.md', command: 'gh issue create --body-file ask.md' };
      },
      runBuiltinIssue: async (issue, options) => {
        calls.push(['run', issue, options.publish, options.autoModel, options.config.llm.model,
          options.config.llm.effort]);
        options.log('Worktree: issue-42\n' +
          'node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "feat: issue 42" --merge-when-green');
        return {
          issue: { number: 42 }, repoRoot: cwd, task: 'issue-42',
          worktreePath: join(cwd, '.worktrees', 'issue-42'),
          command: 'node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "feat: issue 42" --merge-when-green',
        };
      },
      readStatus: async (options) => {
        calls.push(['status', options.issue, options.offline]);
        return { issue: { number: 42 }, openPr: null, offline: options.offline,
          worktreePath: join(cwd, '.worktrees', 'issue-42'), worktreeExists: true };
      },
      formatStatus: (status) => `Issue: #${status.issue.number}\nOpen PR: none\n` +
        `Worktree: ${status.worktreePath}\n`,
      recordEvaluation: async (target, verdict, difficulty, again, options) => {
        calls.push(['eval', target, verdict, difficulty, again, options.cwd]);
        return { session: target };
      },
      resolveContractsPath: () => 'contracts',
      loadMetrics: (options) => {
        calls.push(['stats', options.ref ?? 'HEAD']);
        return [];
      },
      summarizeMetrics: (records) => records,
      formatMetrics: () => 'No AI-Run records found.\n',
      loadAvailableMetrics: () => {
        calls.push(['recommend-metrics']);
        return [];
      },
      routeTask: async ({ taskClass }) => {
        calls.push(['recommend', taskClass]);
        return null;
      },
      formatRoute: () => 'insufficient data\n',
      createFileVault: () => vault,
      setConfigValue: async (field, value) => {
        calls.push(['set-config', field, value]);
        activeConfig = { ...activeConfig, llm: { ...activeConfig.llm, [field]: value } };
        return activeConfig;
      },
    },
  });

  assert.equal(banner, 'roster-repl-project | seat coder | runtime builtin | llm stub');
  assert.equal(await dispatch('/ask Add a status section.'), true);
  await dispatch('/model local-model');
  await dispatch('/effort h');
  assert.equal(await dispatch('/run --issue 42'), true);
  assert.equal(state.config.llm.model, 'local-model');
  assert.equal(state.config.llm.effort, 'h');
  assert.equal(state.lastRun.task, 'issue-42');
  await dispatch('/status');
  await dispatch('/status --offline');
  await dispatch('/eval roster-42-coder accept 3 n');
  await dispatch('/stats HEAD');
  await dispatch('/recommend feat');
  await dispatch('/vault');
  await dispatch('/vault set ROSTER_TOKEN');
  assert.equal(state.pendingSecret, 'ROSTER_TOKEN');
  await dispatch('private-value');
  await dispatch('/vault get ROSTER_TOKEN');
  assert.equal(state.pendingSecret, null);
  await dispatch('/help');
  assert.equal(await dispatch('/unknown'), true);
  assert.equal(await dispatch('plain text'), true);
  assert.equal(await dispatch('/quit'), false);

  assert.deepEqual(calls, [
    ['ask', 'Add a status section.'],
    ['set-config', 'model', 'local-model'],
    ['set-config', 'effort', 'h'],
    ['run', '42', false, false, 'local-model', 'h'],
    ['status', 42, false],
    ['status', 42, true],
    ['eval', 'roster-42-coder', 'accept', '3', 'n', cwd],
    ['stats', 'HEAD'],
    ['recommend-metrics'],
    ['recommend', 'feat'],
    ['vault-list'],
    ['vault-set', 'ROSTER_TOKEN', 'private-value'],
    ['vault-get', 'ROSTER_TOKEN'],
  ]);
  assert.match(output.text, /Worktree: issue-42/);
  assert.match(output.text, /Model: local-model/);
  assert.match(output.text, /Effort: h/);
  assert.match(output.text, /Next: gh issue create --body-file ask\.md/);
  assert.match(output.text, /--message "feat: issue 42" --merge-when-green/);
  assert.doesNotMatch(output.text, /--merge-when-green --merge-when-green/);
  assert.match(output.text, /Issue: #42/);
  assert.match(output.text, /Worktree: .+issue-42/);
  assert.match(output.text, /Commands:\n/);
  assert.ok(!output.text.includes('private-value'));
  assert.match(output.text, /ROSTER_TOKEN is stored \(value hidden/);
  assert.match(errorOutput.text, /Unknown command: \/unknown/);
  assert.match(errorOutput.text, /Unknown command: plain/);
});

test('/eval passes quoted feedback and actual minutes to the human-only writer', async () => {
  const shell = dispatcher({
    services: {
      recordEvaluation: async (target, verdict, difficulty, again, options) => {
        assert.deepEqual([target, verdict, difficulty, again], ['roster-42-coder', 'rework', '4', 'n']);
        assert.equal(options.minutes, 25);
        assert.equal(options.comment, 'Keep the  regression test.');
        assert.deepEqual(options.env, {});
        return { session: target };
      },
    },
  });
  await shell.dispatch('/eval roster-42-coder rework 4 n --minutes 25 --comment "Keep the  regression test."');
});

test('/recommend forwards requested capacity and displays the actual config default on insufficient data', async () => {
  const shell = dispatcher({
    config: { ...config, llm: { ...config.llm, model: 'fallback-model', effort: 'h' } },
    services: {
      loadAvailableMetrics: () => [],
      routeTask: async ({ taskClass, difficulty }) => {
        assert.equal(taskClass, 'fix');
        assert.equal(difficulty, 4);
        return null;
      },
    },
  });
  await shell.dispatch('/recommend fix --difficulty 4');
  assert.equal(shell.output.text, 'insufficient data; config default: fallback-model effort=h\n');
});

test('/model clear and /run --auto-model opt into routing without persisting a selection', async () => {
  const calls = [];
  const shell = dispatcher({
    services: {
      setConfigValue: async (field, value) => {
        calls.push(['set', field, value]);
        return { ...config, llm: { ...config.llm, model: value } };
      },
      runBuiltinIssue: async (issue, options) => {
        calls.push(['run', issue, options.autoModel, options.config.llm.model]);
        return { issue: { number: 42 }, task: 'issue-42', repoRoot: cwd,
          worktreePath: join(cwd, '.worktrees', 'issue-42') };
      },
    },
  });
  await shell.dispatch('/model clear');
  await shell.dispatch('/run 42 --auto-model');
  assert.deepEqual(calls, [['set', 'model', ''], ['run', '42', true, '']]);
  assert.match(shell.output.text, /Model: \(unset\)/);
});

test('slash ask prints the created issue URL when gh is available', async () => {
  const { dispatch, state, output } = dispatcher({
    services: { submitAsk: async () => ({
      mode: 'issue', number: 42, url: 'https://github.com/example/project/issues/42',
    }) },
  });
  await dispatch('/ask Add status to README.');
  assert.equal(state.lastAsk.number, 42);
  assert.equal(output.text, 'Issue: https://github.com/example/project/issues/42\n');
});

test('banner reports the configured LLM endpoint when not using the stub', () => {
  const llm = parseConfig(readFileSync(join(root, 'roster.config.example.yml'), 'utf8')
    .replace('base_url: ""', 'base_url: http://localhost:1234/v1')
    .replace('model: ""', 'model: local-model'));
  const { banner } = createDispatcher({
    cwd, repoRoot: root, config: llm, env: {},
    services: { repositoryRoot: () => cwd },
  });
  assert.equal(banner, 'roster-repl-project | seat coder | runtime builtin | llm http://localhost:1234/v1');
});

test('/publish prints the SDK command without App env and uses the reviewed worktree with App env', async () => {
  const withoutApp = dispatcher({
    env: { AI_MODEL: 'GPT-6-Sol' },
    services: { resolveContractsPath: () => 'contracts' },
  });
  await assert.rejects(withoutApp.dispatch('/publish'), /passing REVIEW\.md/);
  assert.equal(withoutApp.output.text, '');
  await withoutApp.dispatch('/publish --skip-review');
  assert.match(withoutApp.output.text,
    /--message '<conventional subject>[\s\S]+--model GPT-6-Sol --merge-when-green/);
  assert.match(withoutApp.output.text, /## Model\n\nGPT-6-Sol\n\n## Summary/);
  assert.match(withoutApp.output.text, /node --test/);

  const calls = [];
  const withApp = dispatcher({
    env: { GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'key.pem' },
    services: {
      runBuiltinIssue: async () => ({
        issue: { number: 42 }, repoRoot: cwd, task: 'issue-42',
        worktreePath: join(cwd, '.worktrees', 'issue-42'),
        runs: { coder: { line: '1|local|local-model@-|m|3/-|2|roster-42-coder|issue-42',
          env: { AI_MODEL: 'local-model' } } },
        result: { summary: 'Updated the reviewed README.' },
      }),
      prepareBuiltinPublication: async (run, options) => {
        calls.push(['prepare', run.task, options.env.GITHUB_APP_ID]);
        assert.equal(options.skipReview, true);
        return { contractsPath: 'contracts', worktreePath: run.worktreePath,
          publishEnv: { GITHUB_APP_ID: '123', AI_MODEL: 'local-model', AI_SESSION: 'roster-42-coder' } };
      },
      publisher: async (options) => {
        calls.push(['publisher', options]);
        return { mergedPullRequest: 7 };
      },
      issueCommenter: async (options) => { calls.push(['comment', options]); },
    },
  });
  await withApp.dispatch('/run 42');
  await withApp.dispatch('/publish --skip-review');
  assert.equal(withApp.state.lastRun.task, 'issue-42');
  assert.equal(withApp.state.published, true);
  assert.deepEqual(calls[0], ['prepare', 'issue-42', '123']);
  assert.equal(calls[1][1].cwd, join(cwd, '.worktrees', 'issue-42'));
  assert.equal(calls[1][1].model, 'local-model');
  assert.equal(calls[1][1].message, buildPublishMessage({
    subject: 'feat: issue 42', model: 'local-model', summary: 'Updated the reviewed README.', issueNumber: 42,
    seats: 'planner, coder, reviewer (gate bypassed with --skip-review)',
  }));
  assert.equal(calls[1][1].env.AI_SESSION, 'roster-42-coder');
  assert.equal(calls[2][0], 'comment');
  assert.equal(calls[2][1].pullNumber, 7);
  assert.equal(calls[2][1].model, 'local-model');
  assert.match(calls[2][1].runLine, /\|roster-42-coder\|issue-42$/);
  assert.match(withApp.output.text, /left it open for human AI-Eval/);
  await assert.rejects(withApp.dispatch('/publish'), /already published/);
});

test('/publish blocks a failed in-session review until --skip-review is explicit', async () => {
  const shell = dispatcher({
    env: { AI_MODEL: 'review-model' },
    services: {
      resolveContractsPath: () => 'contracts',
      runBuiltinIssue: async () => ({
        issue: { number: 42 }, repoRoot: cwd, task: 'issue-42',
        worktreePath: join(cwd, '.worktrees', 'issue-42'),
        result: { summary: 'Changed README and ran tests.' },
        review: { verdict: 'fail', reasons: ['Acceptance evidence is incomplete.'] },
      }),
      publisher: () => assert.fail('No SDK invocation is allowed without App credentials'),
    },
  });
  await shell.dispatch('/run 42');
  await assert.rejects(shell.dispatch('/publish'), /passing REVIEW\.md/);
  assert.doesNotMatch(shell.output.text, /node vendor\/github-agent-contracts\/scripts\/agent-pr/);
  await shell.dispatch('/publish --skip-review');
  assert.match(shell.output.text, /## Seats\n\nplanner, coder, reviewer \(gate bypassed with --skip-review\)/);
  assert.match(shell.output.text, /--model review-model --merge-when-green/);
});

test('onboarding publication permissions are enforced by the REPL, not just recorded', async () => {
  const disabled = dispatcher({
    config: { ...config, publish: { enabled: false }, reviewer: { required: false } },
    env: { AI_MODEL: 'served-model', GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'key.pem' },
    services: { publisher: () => assert.fail('Disabled publication must not invoke the SDK') },
  });
  await assert.rejects(disabled.dispatch('/publish fix: metadata --skip-review'),
    /Publishing is disabled by publish\.enabled/);
  assert.equal(disabled.output.text, '');
  const optional = dispatcher({
    config: { ...config, review: { required: false } },
    env: { AI_MODEL: 'served-model' },
    services: { resolveContractsPath: () => 'contracts' },
  });
  await optional.dispatch('/publish fix: metadata');
  assert.match(optional.output.text, /--model served-model --merge-when-green/);
});

test('an issue publish without a confirmed merge leaves the issue untouched', async () => {
  const shell = dispatcher({
    env: { GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'key.pem' },
    services: {
      runBuiltinIssue: async () => ({
        issue: { number: 42 }, repoRoot: cwd, task: 'issue-42',
        worktreePath: join(cwd, '.worktrees', 'issue-42'),
        result: { summary: 'Updated reviewed code.' },
      }),
      prepareBuiltinPublication: async (run) => ({
        contractsPath: 'contracts', worktreePath: run.worktreePath, publishEnv: { AI_MODEL: 'local-model' },
      }),
      publisher: async () => ({ mergedPullRequest: null }),
      issueCommenter: () => assert.fail('Unmerged PR must not comment on its issue'),
    },
  });

  await shell.dispatch('/run 42');
  await assert.rejects(shell.dispatch('/publish --skip-review'), /did not confirm a merged PR/);
  assert.equal(shell.state.published, false);
});

test('/publish without a run forwards config, then AI_MODEL, then ROSTER_MODEL explicitly to agent-pr', async () => {
  for (const [model, supplied, expected] of [
    ['configured-model', 'unknown', 'configured-model'], ['', 'GPT-6-Sol', 'GPT-6-Sol'], ['', '', 'served-model'],
  ]) {
    const shell = dispatcher({
      config: { ...config, llm: { ...config.llm, model, effort: 'h' } },
      env: { GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'key.pem',
        AI_MODEL: supplied, ROSTER_MODEL: 'served-model', ROSTER_API_KEY: 'private-value' },
      services: {
        resolveContractsPath: () => 'contracts',
        publisher: async ({ env, model: publishModel, message }) => {
          assert.equal(env.AI_MODEL, expected);
          assert.equal(publishModel, expected);
          assert.ok(message.includes(`## Model\n\n${expected}`));
          assert.match(message, /## Summary\n\nmetadata/);
          assert.match(message, /node --test/);
          assert.equal(env.AI_PROVIDER, 'local');
          assert.equal(env.AI_MODEL_VERSION, '-');
          assert.equal(env.AI_EFFORT, 'h');
          assert.equal(env.ROSTER_API_KEY, undefined);
        },
      },
    });
    await shell.dispatch('/publish fix: metadata --skip-review');
    assert.equal(shell.state.published, true);
  }
});

test('a merged PR with local cleanup failure comments without a duplicate publish', async () => {
  let commented = 0;
  const shell = dispatcher({
    env: { GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'key.pem' },
    services: {
      runBuiltinIssue: async () => ({
        issue: { number: 42 }, repoRoot: cwd, task: 'issue-42',
        worktreePath: join(cwd, '.worktrees', 'issue-42'),
        result: { summary: 'Updated reviewed code.' },
      }),
      prepareBuiltinPublication: async (run) => ({
        contractsPath: 'contracts', worktreePath: run.worktreePath, publishEnv: { AI_MODEL: 'local-model' },
      }),
      publisher: async () => {
        const error = new Error('Local cleanup failed');
        error.mergedPullRequest = 7;
        throw error;
      },
      issueCommenter: async ({ pullNumber, model }) => {
        assert.equal(pullNumber, 7);
        assert.equal(model, 'local-model');
        commented += 1;
      },
    },
  });
  await shell.dispatch('/run 42');
  await assert.rejects(shell.dispatch('/publish --skip-review'), /Local cleanup failed/);
  assert.equal(commented, 1);
  assert.equal(shell.state.published, true);
  await assert.rejects(shell.dispatch('/publish'), /already published/);
});

test('imported contracts publisher receives merge-when-green and reports HTTP 422 without fallback', async (t) => {
  const contracts = mkdtempSync(join(tmpdir(), 'roster-repl-contracts-'));
  t.after(() => rmSync(contracts, { recursive: true, force: true }));
  const scripts = join(contracts, 'scripts');
  mkdirSync(scripts);
  const script = join(scripts, 'agent-pr.mjs');
  writeFileSync(script, `export async function main(argv, { cwd, env, stdout }) {
    stdout.write(JSON.stringify({
      argv, cwd, keyPath: env.GITHUB_APP_PRIVATE_KEY_PATH, apiKey: env.ROSTER_API_KEY ?? null,
    }) + '\\n');
    return 0;
  }\n`);
  const env = { GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'key.pem',
    AI_MODEL: 'GPT-6-Sol', AI_PROVIDER: 'github-copilot', ROSTER_API_KEY: 'not-forwarded' };
  const options = {
    env,
    services: { resolveContractsPath: () => contracts },
  };
  const success = dispatcher(options);
  await success.dispatch('/publish docs: interactive roster shell --skip-review');
  const result = JSON.parse(success.output.text);
  assert.deepEqual(result.argv,
    ['--message', buildPublishMessage({
      subject: 'docs: interactive roster shell', model: 'GPT-6-Sol', summary: 'interactive roster shell',
    }), '--model', 'GPT-6-Sol', '--merge-when-green']);
  assert.equal(result.cwd, cwd);
  assert.equal(result.keyPath, resolve(cwd, 'key.pem'));
  assert.equal(result.apiKey, null);

  writeFileSync(script, `export async function main(_argv, { stderr }) {
    stderr.write('GitHub API request failed (HTTP 422).\\n');
    return 1;
  }\n`);
  const otherContracts = mkdtempSync(join(tmpdir(), 'roster-repl-denied-'));
  t.after(() => rmSync(otherContracts, { recursive: true, force: true }));
  mkdirSync(join(otherContracts, 'scripts'));
  writeFileSync(join(otherContracts, 'scripts', 'agent-pr.mjs'),
    readFileSync(script, 'utf8'));
  const denied = dispatcher({
    env,
    services: { resolveContractsPath: () => otherContracts },
  });
  await assert.rejects(denied.dispatch('/publish docs: interactive roster shell --skip-review'),
    /Checks permission is not accepted on the installation/);
  assert.match(denied.errorOutput.text, /HTTP 422/);

  const partial = dispatcher({ env: { GITHUB_APP_ID: '123' } });
  await assert.rejects(partial.dispatch('/publish docs: shell'), /Set both GITHUB_APP_ID/);
  assert.equal(partial.output.text, '');
});

test('/publish aborts with set model before calling the publisher or printing an unsafe command', async () => {
  for (const app of [false, true]) {
    for (const model of ['', 'unknown']) {
      const shell = dispatcher({
        env: { AI_MODEL: model, ...(app ? {
          GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'key.pem',
        } : {}) },
        services: {
          resolveContractsPath: () => 'unusable-contracts',
          publisher: () => assert.fail('No publisher call is allowed without a real model'),
        },
      });
      await assert.rejects(shell.dispatch('/publish fix: missing model --skip-review'), /set model/);
      assert.equal(shell.output.text, '');
      assert.equal(shell.state.published, false);
    }
  }
});

test('TTY shell prints the banner and exits zero on /quit and Ctrl+C', async () => {
  async function runLine(line) {
    const input = new PassThrough();
    input.isTTY = true;
    input.setRawMode = () => {};
    const output = new PassThrough();
    output.isTTY = true;
    output.columns = 80;
    const errorOutput = new PassThrough();
    let text = '';
    output.on('data', (chunk) => { text += chunk.toString('utf8'); });
    const done = startRepl({
      input, output, errorOutput, cwd, repoRoot: root, config,
      env: {}, services: { repositoryRoot: () => cwd },
    });
    input.write(line);
    const code = await done;
    input.destroy();
    return { code, text };
  }
  const quit = await runLine('/quit\n');
  assert.equal(quit.code, 0);
  assert.match(quit.text, /roster-repl-project \| seat coder \| runtime builtin \| llm stub/);
  assert.match(quit.text, /roster> /);

  const interrupt = await runLine('\x03');
  assert.equal(interrupt.code, 0);
});

test('TTY vault entry hides the secret while storing it through the vault library', async () => {
  const input = new PassThrough();
  input.isTTY = true;
  input.setRawMode = () => {};
  const output = new PassThrough();
  output.isTTY = true;
  output.columns = 80;
  let text = '';
  let stored;
  output.on('data', (chunk) => { text += chunk.toString('utf8'); });
  function waitFor(fragment) {
    return new Promise((resolve, reject) => {
      if (text.includes(fragment)) return resolve();
      const timeout = setTimeout(() => {
        output.off('data', check);
        reject(new Error(`Shell did not print ${fragment}`));
      }, 2_000);
      function check() {
        if (!text.includes(fragment)) return;
        clearTimeout(timeout);
        output.off('data', check);
        resolve();
      }
      output.on('data', check);
    });
  }
  const done = startRepl({
    input, output, cwd, repoRoot: root, config, env: {},
    services: {
      repositoryRoot: () => cwd,
      createFileVault: () => ({
        async set(name, value) { stored = { name, value }; },
      }),
    },
  });
  input.write('/vault set ROSTER_TOKEN\n');
  await waitFor('Secret (input hidden): ');
  input.write('private-value\n');
  await waitFor('Stored secret ROSTER_TOKEN.');
  input.write('/quit\n');
  assert.equal(await done, 0);
  assert.deepEqual(stored, { name: 'ROSTER_TOKEN', value: 'private-value' });
  assert.ok(!text.includes('private-value'));
  input.destroy();
});

test('pasting a vault command and secret together still hides the secret', async () => {
  const input = new PassThrough();
  input.isTTY = true;
  input.setRawMode = () => {};
  const output = new PassThrough();
  output.isTTY = true;
  output.columns = 80;
  let text = '';
  let stored;
  output.on('data', (chunk) => { text += chunk.toString('utf8'); });
  const done = startRepl({
    input, output, cwd, repoRoot: root, config, env: {},
    services: {
      repositoryRoot: () => cwd,
      createFileVault: () => ({
        async set(name, value) { stored = { name, value }; },
      }),
    },
  });
  input.write('/vault set ROSTER_TOKEN\nprivate-value\n/quit\n');
  assert.equal(await done, 0);
  assert.deepEqual(stored, { name: 'ROSTER_TOKEN', value: 'private-value' });
  assert.ok(!text.includes('private-value'));
  input.destroy();
});

test('empty argv with non-TTY stdin prints usage and exits 2 without starting the loop', () => {
  const result = spawnSync(process.execPath, [cli], {
    cwd: root, input: '', encoding: 'utf8', timeout: 10_000,
  });
  assert.ifError(result.error);
  assert.equal(result.status, 2, result.stderr);
  assert.match(result.stdout, /^Usage:\n/);
  assert.doesNotMatch(result.stdout, /roster> /);
});
