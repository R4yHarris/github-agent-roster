import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import test from 'node:test';
import { buildPublishMessage, formatPublishCommand } from '../src/lib/publication.mjs';
import { resolveContractsPath } from '../src/lib/paths.mjs';
import { resolvePublishModel } from '../src/metrics/run.mjs';

test('the pinned contracts CLI exits nonzero without a real model before publication', () => {
  const publisher = path.join(resolveContractsPath(), 'scripts', 'agent-pr.mjs');
  for (const modelArgs of [[], ['--model', ''], ['--model', 'unknown'], ['--model', 'UNKNOWN']]) {
    const result = spawnSync(process.execPath, [publisher, '--message', 'fix: model guard', ...modelArgs], {
      encoding: 'utf8', timeout: 10_000,
      env: { ...process.env, AI_MODEL: '', GITHUB_APP_ID: '', GITHUB_APP_PRIVATE_KEY_PATH: '' },
    });
    assert.ifError(result.error);
    assert.equal(result.status, 2, result.stderr);
    assert.match(result.stderr, /Set --model or AI_MODEL.*not unknown/);
    assert.equal(result.stdout, '');
  }
});

test('resolves publication model in config, AI_MODEL, ROSTER_MODEL order', () => {
  const env = { AI_MODEL: 'GPT-6-Sol', ROSTER_MODEL: 'served-model' };
  assert.equal(resolvePublishModel({ config: { llm: { model: 'configured-model' } }, env }),
    'configured-model');
  assert.equal(resolvePublishModel({ config: { llm: { model: '' } }, env }), 'GPT-6-Sol');
  assert.equal(resolvePublishModel({ env: { ...env, AI_MODEL: '' } }), 'served-model');
  for (const model of [undefined, '', ' ', 'unknown', 'UNKNOWN', 'none', 'n/a', 'unspecified',
    'bad\nmodel', 'bad|model', '-model', 42]) {
    assert.throws(() => resolvePublishModel({ env: { AI_MODEL: model } }), /set model/);
  }
  assert.throws(() => resolvePublishModel({
    config: { llm: { model: 'unknown' } }, env,
  }), /set model/);
});

test('publish message lists files outside planned scope and rejects protected ones', () => {
  const message = buildPublishMessage({ subject: 'feat: state root', model: 'qwen3', summary: 'Adds resolver.',
    scopeFiles: ['src/cli.mjs'] });
  assert.match(message, /## Files outside planned scope\n\n- `src\/cli\.mjs`/);
  assert.doesNotMatch(buildPublishMessage({ subject: 'feat: x', model: 'qwen3', summary: 'S.' }), /outside planned scope/);
  assert.throws(() => buildPublishMessage({ subject: 'feat: x', model: 'qwen3', summary: 'S.',
    scopeFiles: ['.github/workflows/ci.yml'] }), /scope expansion/);
});

test('publish message includes a real model, test command, and non-closing issue reference', () => {
  const message = buildPublishMessage({
    subject: 'fix: refuse publish without a model id', model: 'GPT-6-Sol',
    summary: 'Abort before the SDK when model configuration is missing.', issueNumber: 42,
  });
  assert.equal(message, 'fix: refuse publish without a model id\n\n## Model\n\nGPT-6-Sol\n\n' +
    '## Summary\n\nAbort before the SDK when model configuration is missing.\n\n' +
    '### How to test\n\nRun `node --test` from the feature worktree root and review the task acceptance checks.\n\n' +
    'Refs #42');
  assert.throws(() => buildPublishMessage({
    subject: 'fix: model', model: 'real', summary: 'Fixes #42.', issueNumber: 42,
  }), /must remain open/);
  assert.throws(() => buildPublishMessage({ subject: 'fix: model', model: '', summary: 'Guard.' }), /set model/);
  assert.throws(() => buildPublishMessage({ subject: 'fix: model', model: 'real', summary: '' }), /summary/);
  assert.throws(() => buildPublishMessage({ subject: 'fix: model\ninjected', model: 'real', summary: 'Guard.' }),
    /single-line/);
});

test('an explicit test waiver is truthful rather than a claim that tests passed', () => {
  const message = buildPublishMessage({
    subject: 'docs: clarify publish', model: 'real-model', summary: 'Clarified publication.',
    testsSkipped: true,
  });
  assert.match(message, /tests: none/);
  assert.doesNotMatch(message, /tests pass|Run `node --test`/);
});

test('manual publication commands include --model and quote multiline summaries for the caller shell', () => {
  const message = 'fix: guard\n\n## Summary\n\nDon\'t expand "$value" or `commands`.';
  assert.equal(formatPublishCommand({ message, model: 'real-model', script: 'publisher.mjs', platform: 'win32' }),
    'node publisher.mjs --message \'fix: guard\n\n## Summary\n\nDon\'\'t expand "$value" or `commands`.\' ' +
    '--model real-model --merge-when-green');
  assert.equal(formatPublishCommand({ message, model: 'real-model', script: 'publisher.mjs', platform: 'linux' }),
    'node publisher.mjs --message \'fix: guard\n\n## Summary\n\nDon\'\\\'\'t expand "$value" or `commands`.\' ' +
    '--model real-model --merge-when-green');
});
