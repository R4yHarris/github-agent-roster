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
import { buildRun } from '../src/metrics/run.mjs';

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
    services: { repositoryRoot: () => cwd,
      publicationTask: ({ task, env }) => task || env.AI_TASK || 'feat-ghcp-metadata', ...services },
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

test('/run sends live events to its stderr writer before the final summary is ready', async () => {
  let finish;
  const pending = new Promise((resolve) => { finish = resolve; });
  const shell = dispatcher({
    services: { runBuiltinIssue: async (_issue, { log, errorOutput }) => {
      errorOutput.write('2026-09-30T22:00:00.000Z start seat planner session=roster-92-planner\n');
      await pending;
      log('Final run summary.');
      return { failed: false, command: null, issue: { number: 92 }, task: 'issue-92' };
    } },
  });
  const running = shell.dispatch('/run 92');
  assert.match(shell.errorOutput.text, /start seat planner/);
  assert.equal(shell.output.text, '');
  finish();
  assert.equal(await running, true);
  assert.match(shell.output.text, /Final run summary/);
  assert.doesNotMatch(shell.output.text, /start seat planner/);
});

test('/log N tails matching local logs without dispatching a run or network call', async () => {
  const shell = dispatcher({ services: {
    readIssueLogs: async (options) => {
      assert.equal(options.issue, 92);
      assert.equal(options.limit, 50);
      return [{ path: 'issue-seat.log', lines: ['2026-09-30T22:00:00.000Z seat coder mode llm'] }];
    },
    runBuiltinIssue: () => assert.fail('Log command must not run a seat'),
  } });
  await shell.dispatch('/log 92');
  assert.match(shell.output.text, /issue-seat.log[\s\S]*seat coder mode llm/);
  await assert.rejects(shell.dispatch('/log invalid'), /Use \/log N/);
});

test('planning-only handoff requires another explicit run and cannot be published with a review bypass', async () => {
  const shell = dispatcher({ services: { runBuiltinIssue: async () => ({
    planningOnly: true, command: null, issue: { number: 92 }, task: 'issue-92',
  }) } });
  await shell.dispatch('/run 92');
  assert.match(shell.output.text, /TASK validates[\s\S]*\/run 92 to start coder/);
  await assert.rejects(shell.dispatch('/publish --skip-review'), /Planning-only TASK is not code/);
});

test('feature/initiative shell runs show PLAN rather than a coder handoff and cannot bypass publication', async () => {
  for (const askKind of ['feature', 'initiative']) {
    const shell = dispatcher({ services: { runBuiltinIssue: async () => ({
      askKind, planningOnly: true, planPath: 'PLAN.md', command: null, issue: { number: 92 },
    }) } });
    await shell.dispatch('/run 92');
    assert.match(shell.output.text, /PLAN ready[\s\S]*each slice[\s\S]*will not run coder/);
    assert.doesNotMatch(shell.output.text, /TASK validates|\/run 92 to start coder|Use \/publish/);
    await assert.rejects(shell.dispatch('/publish --skip-review'), /Planning-only PLAN/);
    assert.equal(await shell.dispatch('/quit'), false);
  }
});

test('clarification is visible in the shell and never offers publication', async () => {
  const shell = dispatcher({ services: {
    submitAsk: async () => ({ mode: 'clarify', askKind: 'clarify', clarification: 'Name one outcome and allowed files.' }),
    runBuiltinIssue: async () => ({ planningOnly: true, askKind: 'clarify', command: null,
      clarification: 'Name one outcome and allowed files.', issue: { number: 92 } }),
  } });
  await shell.dispatch('/ask Improve things');
  await shell.dispatch('/run 92');
  assert.match(shell.output.text, /Ask kind: clarify[\s\S]*Name one outcome/);
  assert.doesNotMatch(shell.output.text, /Use \/publish|TASK: undefined|TASK validates/);
  await assert.rejects(shell.dispatch('/publish --skip-review'), /clarification is not code/);
});

test('a failed new planning run cannot leave an old publishable run selected', async () => {
  const shell = dispatcher({ services: { runBuiltinIssue: async () => { throw new Error('PLAN validation failed'); } } });
  shell.state.lastRun = { issue: { number: 41 }, command: 'previous publish command' };
  await assert.rejects(shell.dispatch('/run 92'), /PLAN validation failed/);
  assert.equal(shell.state.lastRun.planningOnly, true);
  assert.equal(shell.state.lastRun.failed, true);
  await assert.rejects(shell.dispatch('/publish --skip-review'), /Planning-only PLAN or clarification/);
});

test('/run reports a failed planner stub without throwing and leaves the shell usable', async () => {
  const shell = dispatcher({
    services: { runBuiltinIssue: async (_issue, { log }) => {
      log('Planning failed: malformed tool calls; RECIPE/TASK stubs written.');
      return { failed: true, command: null, issue: { number: 92 }, task: 'issue-92' };
    } },
  });
  assert.equal(await shell.dispatch('/run 92'), true);
  assert.equal(shell.state.lastRun.failed, true);
  assert.equal(await shell.dispatch('/help'), true);
  assert.match(shell.output.text, /Planning failed; stubs are unverified/);
  assert.doesNotMatch(shell.output.text, /Use \/publish/);
  assert.match(shell.output.text, /Commands:/);
  assert.equal(await shell.dispatch('/quit'), false);
});

test('slash ask prints the created issue URL when gh is available', async () => {
  const { dispatch, state, output } = dispatcher({
    services: { submitAsk: async () => ({
      mode: 'issue', number: 42, url: 'https://github.com/example/project/issues/42',
    }) },
  });
  await dispatch('/ask Add status to README.');
  assert.equal(state.lastAsk.number, 42);
  assert.equal(output.text, 'Ask kind: slice\nIssue: https://github.com/example/project/issues/42\n');
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
  assert.match(withoutApp.output.text, /AI_PROVIDER=github-copilot\n/);
  assert.match(withoutApp.output.text, /AI_CONTEXT_USED=\n/);
  assert.match(withoutApp.output.text, /AI_CONTEXT_OUT=\n/);
  assert.match(withoutApp.output.text, /AI_TASK=feat-ghcp-metadata\n/);

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
  assert.equal(calls[2][1].run, withApp.state.lastRun.runs.coder);
  assert.match(calls[2][1].runLine, /\|roster-42-coder\|issue-42$/);
  assert.match(withApp.output.text, /left it open for human AI-Eval/);
  assert.match(withApp.output.text, /roster eval roster-42-coder accept 1 n --minutes M/);
  await assert.rejects(withApp.dispatch('/publish'), /already published/);
});

test('/publish --model declares GHCP without changing the configured served model', async () => {
  const activeConfig = { ...config, llm: { ...config.llm, model: 'configured-served-model', effort: 'h' } };
  const shell = dispatcher({
    config: activeConfig, env: { AI_MODEL: 'unknown', AI_PROVIDER: 'openai',
      AI_CONTEXT_USED: '1000000', AI_CONTEXT_OUT: '999' },
    services: { resolveContractsPath: () => 'contracts' },
  });
  await shell.dispatch('/publish --model GPT-6.1-Sol fix: metadata --skip-review');
  assert.equal(shell.state.config.llm.model, 'configured-served-model');
  assert.match(shell.output.text, /--model GPT-6\.1-Sol --merge-when-green/);
  assert.match(shell.output.text, /AI_PROVIDER=github-copilot\n/);
  assert.match(shell.output.text, /AI_MODEL_VERSION=-\n/);
  assert.match(shell.output.text, /AI_EFFORT=-\n/);
  assert.match(shell.output.text, /AI_CONTEXT_USED=\n/);
  assert.match(shell.output.text, /AI_CONTEXT_OUT=\n/);
  assert.doesNotMatch(shell.output.text, /AI_CONTEXT_USED=1000000|AI_CONTEXT_OUT=999/);
});

test('/publish preserves measured seat attribution over a GHCP model flag and stale session declarations', async () => {
  const activeConfig = { ...config, llm: { ...config.llm,
    model: 'later-request-alias', provider: 'vllm', context_max: 8192 } };
  const run = buildRun({ config: activeConfig, response: {
    model: 'actual-response-model', usage: { prompt_tokens: 100, completion_tokens: 40 },
  }, session: 'roster-42-coder', task: 'issue-42', env: {} });
  const shell = dispatcher({
    config: activeConfig, env: { AI_MODEL: 'GPT-6.1-Sol', AI_PROVIDER: 'github-copilot',
      AI_CONTEXT_USED: '1000000', AI_CONTEXT_MAX: '1000000', AI_CONTEXT_OUT: '999' },
    services: {
      resolveContractsPath: () => 'contracts',
      publicationTask: () => assert.fail('Measured seat must not derive GHCP task'),
      runBuiltinIssue: async () => ({
        issue: { number: 42 }, repoRoot: cwd, task: 'issue-42',
        worktreePath: join(cwd, '.worktrees', 'issue-42'),
        runs: { coder: run }, result: { summary: 'Reviewed measured changes.' },
      }),
    },
  });
  await shell.dispatch('/run 42');
  await shell.dispatch('/publish --model GPT-6.1-Sol --skip-review');
  assert.match(shell.output.text, /--model actual-response-model --merge-when-green/);
  assert.match(shell.output.text, /AI_PROVIDER=local\n/);
  assert.match(shell.output.text, /AI_CONTEXT_USED=100\n/);
  assert.match(shell.output.text, /AI_CONTEXT_OUT=40\n/);
  assert.match(shell.output.text, /AI_CONTEXT_MAX=8192\n/);
  assert.match(shell.output.text, /AI_SESSION=roster-42-coder\n/);
  assert.doesNotMatch(shell.output.text, /1000000|GHCP used\/out are/);
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

test('/publish without a run declares GHCP instead of borrowing the configured endpoint metadata', async () => {
  for (const [model, supplied, flag, expected] of [
    ['configured-model', 'GPT-6.1-Sol', '', 'GPT-6.1-Sol'],
    ['configured-model', 'unknown', ' --model explicit-copilot-model', 'explicit-copilot-model'],
    ['', 'GPT-6.1-Sol', '', 'GPT-6.1-Sol'],
  ]) {
    const shell = dispatcher({
      config: { ...config, llm: { ...config.llm, model, effort: 'h', provider: 'openai', context_max: 8192 } },
      env: { GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'key.pem',
        AI_MODEL: supplied, ROSTER_MODEL: 'served-model', ROSTER_API_KEY: 'private-value',
        AI_PROVIDER: 'local', AI_EFFORT: 'max', AI_CONTEXT_MAX: '1000000',
        AI_CONTEXT_USED: '1000000', AI_CONTEXT_OUT: '999', AI_MODEL_VERSION: 'stale',
        AI_SESSION: 'roster-stale-coder' },
      services: {
        resolveContractsPath: () => 'contracts',
        publisher: async ({ env, model: publishModel, message }) => {
          assert.equal(env.AI_MODEL, expected);
          assert.equal(publishModel, expected);
          assert.ok(message.includes(`## Model\n\n${expected}`));
          assert.match(message, /## Summary\n\nmetadata/);
          assert.match(message, /node --test/);
          assert.equal(env.AI_PROVIDER, 'github-copilot');
          assert.equal(env.AI_MODEL_VERSION, '-');
          assert.equal(env.AI_EFFORT, 'x');
          assert.equal(env.AI_CONTEXT_MAX, '1000000');
          assert.equal(Object.hasOwn(env, 'AI_CONTEXT_USED'), false);
          assert.equal(Object.hasOwn(env, 'AI_CONTEXT_OUT'), false);
          assert.match(env.AI_SESSION, /^ghcp-\d+$/);
          assert.equal(env.AI_TASK, 'feat-ghcp-metadata');
          assert.match(message, /GHCP used\/out are `-` \(unknown\)/);
          assert.equal(env.ROSTER_API_KEY, undefined);
        },
      },
    });
    await shell.dispatch(`/publish fix: metadata${flag} --skip-review`);
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
      provider: env.AI_PROVIDER, version: env.AI_MODEL_VERSION,
      used: env.AI_CONTEXT_USED ?? null, out: env.AI_CONTEXT_OUT ?? null,
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
      subject: 'docs: interactive roster shell', model: 'GPT-6-Sol', summary: 'interactive roster shell', ghcp: true,
    }), '--model', 'GPT-6-Sol', '--merge-when-green']);
  assert.equal(result.cwd, cwd);
  assert.equal(result.keyPath, resolve(cwd, 'key.pem'));
  assert.equal(result.apiKey, null);
  assert.equal(result.provider, 'github-copilot');
  assert.equal(result.version, '-');
  assert.equal(result.used, null);
  assert.equal(result.out, null);

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
