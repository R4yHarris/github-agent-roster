import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { setImmediate as nextTurn } from 'node:timers/promises';
import test from 'node:test';
import { runBuiltinAsk, runBuiltinIssue, runBuiltinTask } from '../src/lib/builtin.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { formatConfig } from '../src/lib/config.mjs';
import { openProvenanceStore } from '../src/lib/provenance-store.mjs';
import { fixture, llmConfig, git } from './helpers/builtin.mjs';

test('task timeout retains cancellation provenance and ownership until preparation stops', async (t) => {
  const options = fixture(t);
  const head = git(options.target, 'rev-parse', 'HEAD');
  const config = { ...llmConfig, seat: { ...llmConfig.seat, task_timeout_ms: 2000 } };
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let prepared;
  let cleanupFinished = false;
  await assert.rejects(runBuiltinIssue(42, { ...options, config, log: () => {},
    fetchImpl: () => assert.fail('Timed-out preparation must not contact a model'),
    onPrepared: async (ready) => {
      prepared = ready;
      t.mock.timers.tick(2000);
      await nextTurn();
      assert.ok(existsSync(path.join(options.target, '.git', 'roster', 'wave-runs', 'locks', 'issue-42.lock')));
      cleanupFinished = true;
    },
  }), { code: 'ROSTER_TASK_TIMEOUT', timeoutMs: 2000 });
  assert.ok(prepared);
  assert.equal(cleanupFinished, true);
  assert.equal(existsSync(path.join(options.target, '.git', 'roster', 'wave-runs', 'locks', 'issue-42.lock')), false);
  assert.equal(git(options.target, 'rev-parse', 'HEAD'), head);
  assert.equal(readFileSync(path.join(options.target, 'README.md'), 'utf8'), '# Example\n');
  const { records } = await openProvenanceStore(path.join(options.base, 'machine', 'provenance')).readAll();
  assert.deepEqual(records.map((record) => record.event).sort(), ['cancellation', 'started']);
  assert.equal(records.find((record) => record.event === 'cancellation').payload.outcome, 'timed_out');
  assert.equal(existsSync(path.join(prepared.worktreePath, 'REVIEW.md')), false);
});

for (const entry of ['issue', 'ask', 'task']) {
  test(`${entry} entry cancels a pending model on its budget without retries, review or publication`, async (t) => {
    const options = fixture(t);
    const config = { ...llmConfig, seat: { ...llmConfig.seat, task_timeout_ms: 5000 } };
    mkdirSync(path.join(options.target, '.roster'));
    writeFileSync(path.join(options.target, '.roster', 'config.yml'), formatConfig(config));
    t.mock.timers.enable({ apis: ['setTimeout'] });
    let requests = 0;
    let stopped = false;
    const fetchImpl = async (_url, { signal }) => {
      requests += 1;
      return new Promise((resolve) => {
        signal.addEventListener('abort', () => {
          stopped = true;
          resolve(Response.json({ choices: [] }));
        }, { once: true });
        t.mock.timers.tick(5000);
      });
    };
    const execution = { ...options, fetchImpl, log: () => {},
      env: { ...options.env, GITHUB_APP_ID: '1', GITHUB_APP_PRIVATE_KEY_PATH: path.join(options.base, 'test-only.pem') },
      publish: true, publisher: () => assert.fail('Timeout must not publish') };
    let pending;
    if (entry === 'issue') pending = runBuiltinIssue(42, execution);
    else if (entry === 'ask') pending = runBuiltinAsk('Add a one-line Status section to README.md.', execution);
    else {
      writeFileSync(path.join(options.target, 'TASK.md'), planStub(options.issue.body, {
        title: options.issue.title, reference: 'local:deadline',
      }).task);
      pending = runBuiltinTask({ ...execution, cwd: options.target });
    }
    await assert.rejects(pending, { code: 'ROSTER_TASK_TIMEOUT', timeoutMs: 5000 });
    assert.equal(requests, 1);
    assert.equal(stopped, true);
    assert.equal(existsSync(path.join(options.target, 'REVIEW.md')), false);
    const { records } = await openProvenanceStore(path.join(options.base, 'machine', 'provenance')).readAll();
    assert.ok(!records.some((record) => record.event === 'completed'));
    if (entry !== 'task') assert.ok(records.some((record) =>
      record.event === 'cancellation' && record.payload.outcome === 'timed_out'));
  });
}
