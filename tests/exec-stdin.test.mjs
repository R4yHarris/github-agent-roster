import assert from 'node:assert/strict';
import test from 'node:test';
import { execute } from '../src/runtime/exec.mjs';

test('execute closes child stdin so a stdin-reading CLI finishes instead of hanging', async () => {
  const script = "let n=0;process.stdin.on('data',c=>n+=c.length).on('end',()=>console.log('eof '+n));";
  const { stdout } = await execute(process.execPath, ['-e', script], { encoding: 'utf8', timeout: 10_000 });
  assert.equal(stdout.trim(), 'eof 0');
});

test('execute rejects with exit code, stdout and stderr like execFile', async () => {
  await assert.rejects(
    execute(process.execPath, ['-e', "console.log('out');console.error('err');process.exit(3)"], { encoding: 'utf8' }),
    (error) => error.code === 3 && error.stdout.trim() === 'out' && error.stderr.trim() === 'err');
});
