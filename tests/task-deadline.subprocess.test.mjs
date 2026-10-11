import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { existsSync, writeFileSync } from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import { promisify } from 'node:util';
import test from 'node:test';
import { planStub } from '../src/planner/stub.mjs';
import { fixture, llmConfig } from './helpers/builtin.mjs';

const execute = promisify(execFile);

test('real task subprocess cancels stalled localhost HTTP without leaving review or live connections', async (t) => {
  const options = fixture(t);
  let requests = 0;
  const sockets = new Set();
  const server = http.createServer(() => { requests += 1; });
  server.on('connection', (socket) => {
    sockets.add(socket);
    socket.once('close', () => sockets.delete(socket));
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(async () => {
    for (const socket of sockets) socket.destroy();
    await new Promise((resolve) => server.close(resolve));
  });
  writeFileSync(path.join(options.target, 'TASK.md'), planStub(options.issue.body, {
    title: options.issue.title, reference: 'local:deadline',
  }).task);
  const config = { ...llmConfig, seat: { ...llmConfig.seat, task_timeout_ms: 10000 },
    llm: { ...llmConfig.llm, base_url: `http://127.0.0.1:${server.address().port}/v1` } };
  const module = new URL('../src/lib/builtin.mjs', import.meta.url).href;
  const { stdout } = await execute(process.execPath, ['--input-type=module', '--eval', `
    import { runBuiltinTask } from ${JSON.stringify(module)};
    try {
      await runBuiltinTask({ cwd: process.cwd(), repoRoot: process.env.FIXTURE_ROSTER_ROOT,
        config: JSON.parse(process.env.FIXTURE_CONFIG), log: () => {}, errorOutput: { write() {} } });
      throw new Error('Stalled task unexpectedly completed');
    } catch (error) {
      if (error.code !== 'ROSTER_TASK_TIMEOUT') throw error;
      console.log(JSON.stringify({ code: error.code, timeoutMs: error.timeoutMs,
        verified: error.result?.excellence?.pass ?? false }));
    }
  `], { cwd: options.target, env: { ...options.env,
    FIXTURE_ROSTER_ROOT: options.repoRoot, FIXTURE_CONFIG: JSON.stringify(config) },
    encoding: 'utf8', timeout: 30000 });
  assert.deepEqual(JSON.parse(stdout), { code: 'ROSTER_TASK_TIMEOUT', timeoutMs: 10000, verified: false });
  assert.equal(requests, 1);
  assert.equal(existsSync(path.join(options.target, 'REVIEW.md')), false);
  assert.equal(sockets.size, 0);
});
