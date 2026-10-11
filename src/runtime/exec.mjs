import { execFile } from 'node:child_process';

// Like promisify(execFile) but closes the child's stdin so a spawned CLI that reads
// stdin to EOF cannot hang a test run until the timeout.
export function execute(file, args, options) {
  return new Promise((resolve, reject) => {
    const child = execFile(file, args, options, (error, stdout, stderr) => {
      if (error) reject(Object.assign(error, { stdout, stderr }));
      else resolve({ stdout, stderr });
    });
    child.stdin?.end();
  });
}
