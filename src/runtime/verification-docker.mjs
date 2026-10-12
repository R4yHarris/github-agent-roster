import { execFile } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { promises as fs } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { captureVerificationSubject, judgeVerificationReceipt } from './independent-verification.mjs';

const execute = promisify(execFile);
const outputLimit = 64 * 1024;
const versionScript = 'if(process.platform!=="linux"||process.versions.node.split(".")[0]!=="20")process.exit(1);console.log(process.version)';

function validatePlan(plan, files, checkCount) {
  if (!Array.isArray(plan) || !plan.length || plan.length > 16) {
    throw new TypeError('Docker verification requires 1-16 host-approved observations');
  }
  const observations = plan.map((entry, index) => {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry) ||
        Object.keys(entry).sort().join(',') !== 'args,check_id,evidence,expected_exit,expected_stderr,expected_stdout,script' ||
        typeof entry.script !== 'string' || !files.includes(entry.script) || !/\.[cm]?js$/.test(entry.script) ||
        !Array.isArray(entry.args) || entry.args.length > 16 ||
        entry.args.some((arg) => typeof arg !== 'string' || arg.length > 1000 || /[\0\r\n]/.test(arg)) ||
        typeof entry.expected_stdout !== 'string' || typeof entry.expected_stderr !== 'string' ||
        entry.expected_stdout.length + entry.expected_stderr.length > 1500) {
      throw new TypeError('Docker verification observations require a snapshotted Node script and bounded host-approved arguments/outputs');
    }
    return {
      id: index + 1, check_id: entry.check_id,
      expected_output: JSON.stringify({ stdout: entry.expected_stdout, stderr: entry.expected_stderr }),
      actual_output: JSON.stringify({ stdout: entry.expected_stdout, stderr: entry.expected_stderr }),
      expected_exit: entry.expected_exit, actual_exit: entry.expected_exit, evidence: entry.evidence,
    };
  });
  judgeVerificationReceipt({ schema_version: 1, subject_digest: 'a'.repeat(64), observations },
    { subjectDigest: 'a'.repeat(64), checkCount });
  return observations;
}

function dockerEnvironment() {
  const names = ['PATH', 'Path', 'PATHEXT', 'SystemRoot', 'SYSTEMROOT', 'ComSpec', 'COMSPEC', 'windir'];
  return Object.fromEntries(names.filter((name) => process.env[name] !== undefined)
    .map((name) => [name, process.env[name]]));
}

// Only a trusted host may supply the plan. Nothing is loaded or executed from model instructions.
export async function runDockerVerification({
  enabled, worktree, files, checkCount, plan, image, dockerPath = 'docker',
  timeoutMs = 30_000, signal, runCommand = execute,
} = {}) {
  if (enabled !== true) throw new Error('Docker verification requires explicit host opt-in');
  if (typeof image !== 'string' || !/^sha256:[a-f0-9]{64}$/.test(image)) {
    throw new TypeError('Docker verification requires an immutable locally acquired sha256 image ID');
  }
  if (typeof dockerPath !== 'string' || !dockerPath.trim() ||
      !Number.isSafeInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 30_000) {
    throw new TypeError('Docker verification requires an executable and a timeout of 1-30000 ms');
  }
  signal?.throwIfAborted();
  const subject = await captureVerificationSubject({ worktree, files });
  const normalizedFiles = subject.files.map((entry) => entry.path);
  const observations = validatePlan(plan, normalizedFiles, checkCount);
  const snapshot = await fs.mkdtemp(path.join(os.tmpdir(), 'roster-verification-'));
  const options = { env: dockerEnvironment(), encoding: 'utf8', windowsHide: true, maxBuffer: outputLimit };
  let result;
  let failure;
  try {
    for (const entry of subject.files) {
      if (entry.digest === null) continue;
      const bytes = await fs.readFile(path.join(worktree, entry.path));
      if (createHash('sha256').update(bytes).digest('hex') !== entry.digest) {
        throw new Error('Docker verification product changed while creating its snapshot');
      }
      const destination = path.join(snapshot, entry.path);
      await fs.mkdir(path.dirname(destination), { recursive: true, mode: 0o755 });
      await fs.writeFile(destination, bytes, { flag: 'wx', mode: 0o644 });
    }
    await fs.chmod(snapshot, 0o755);
    const isolation = [
      '--pull', 'never', '--network', 'none', '--read-only', '--cap-drop', 'ALL',
      '--security-opt', 'no-new-privileges', '--user', '65534:65534',
      '--memory', '256m', '--memory-swap', '256m', '--cpus', '1', '--pids-limit', '32',
      '--ulimit', 'nofile=256:256', '--tmpfs', '/tmp:rw,noexec,nosuid,size=16m,mode=1777',
      '--mount', `type=bind,source=${snapshot},target=/product,readonly`,
      '--workdir', '/product', '--entrypoint', 'node',
    ];
    const run = async (args, { mustSucceed = false } = {}) => {
      signal?.throwIfAborted();
      const name = `roster-verify-${randomUUID()}`;
      let execution;
      let executionError;
      try {
        try {
          const output = await runCommand(dockerPath, ['run', '--name', name, ...isolation, image, ...args],
            { ...options, timeout: timeoutMs, signal });
          execution = { exit: 0, stdout: String(output.stdout ?? ''), stderr: String(output.stderr ?? '') };
        } catch (error) {
          signal?.throwIfAborted();
          if (!Number.isInteger(error.code) || error.code < 1 || error.code > 124 || error.killed ||
              mustSucceed) throw error;
          execution = { exit: error.code, stdout: String(error.stdout ?? ''), stderr: String(error.stderr ?? '') };
        }
      } catch (error) {
        executionError = error;
      }
      try {
        // Cleanup has its own deadline and never inherits a cancelled execution signal.
        await runCommand(dockerPath, ['rm', '-f', name], { ...options, timeout: 10_000 });
      } catch (cleanupError) {
        throw new AggregateError([...(executionError ? [executionError] : []), cleanupError],
          'Docker verification container cleanup failed; inspect the named container before retrying');
      }
      if (executionError) throw executionError;
      return execution;
    };
    const version = await run(['--input-type=commonjs', '-e', versionScript], { mustSucceed: true });
    if (!/^v20\.\d+\.\d+\r?\n$/.test(version.stdout) || version.stderr) {
      throw new Error('Docker verification requires a verified Linux Node20 image');
    }
    for (const [index, entry] of plan.entries()) {
      const executed = await run(['--', `/product/${entry.script}`, ...entry.args]);
      if (executed.stdout.length + executed.stderr.length > 1500) {
        throw new Error('Docker verification output exceeds the receipt limit');
      }
      observations[index].actual_exit = executed.exit;
      observations[index].actual_output = JSON.stringify({ stdout: executed.stdout, stderr: executed.stderr });
    }
    signal?.throwIfAborted();
    const current = await captureVerificationSubject({ worktree, files: normalizedFiles });
    if (current.digest !== subject.digest) throw new Error('Docker verification subject changed during execution');
    const receipt = { schema_version: 1, subject_digest: subject.digest, observations };
    const verdict = judgeVerificationReceipt(receipt, { subjectDigest: subject.digest, checkCount });
    result = { receipt, status: verdict.status, image, node: version.stdout.trim(), subject };
  } catch (error) {
    failure = error;
  }
  try {
    await fs.rm(snapshot, { recursive: true, maxRetries: 5, retryDelay: 100 });
  } catch (cleanupError) {
    throw new AggregateError([...(failure ? [failure] : []), cleanupError],
      'Docker verification snapshot cleanup failed');
  }
  if (failure) throw failure;
  return result;
}
