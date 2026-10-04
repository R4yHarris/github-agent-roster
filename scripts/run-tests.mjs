import { spawn } from 'node:child_process';
import os from 'node:os';

const jobs = String(Math.max(1, Math.floor(os.availableParallelism() / 2)));
const child = spawn(process.execPath, ['--test', '--test-concurrency', jobs], { stdio: 'inherit' });
child.on('exit', (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  process.exit(code ?? 1);
});
