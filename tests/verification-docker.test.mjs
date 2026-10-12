import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { runDockerVerification } from '../src/runtime/verification-docker.mjs';

const image = `sha256:${'a'.repeat(64)}`;
async function fixture(context) {
  const worktree = await fs.mkdtemp(path.join(os.tmpdir(), 'roster-docker-test-'));
  context.after(() => fs.rm(worktree, { recursive: true, force: true }));
  await fs.writeFile(path.join(worktree, 'TASK.md'), 'Original Ask\n');
  await fs.writeFile(path.join(worktree, 'RESULT.md'), 'Host evidence\n');
  await fs.writeFile(path.join(worktree, '.env'), 'private-test-only-sentinel\n');
  await fs.writeFile(path.join(worktree, 'check.mjs'), 'console.log("line 4");\n');
  return { enabled: true, worktree, files: ['check.mjs'], checkCount: 1, image,
    plan: [{ script: 'check.mjs', args: [], check_id: 1, expected_stdout: 'line 4\n',
      expected_stderr: '', expected_exit: 0, evidence: 'Host-approved public operation check' }] };
}

test('immutable snapshot execution has explicit isolation, no credentials, and unique cleanup', async (context) => {
  const options = await fixture(context);
  const names = [];
  const cleaned = [];
  let snapshot;
  const result = await runDockerVerification({ ...options, runCommand: async (command, args, settings) => {
    assert.equal(command, 'docker');
    assert.equal(settings.env.GITHUB_APP_ID, undefined);
    assert.equal(settings.env.GH_TOKEN, undefined);
    assert.equal(settings.env.ROSTER_API_KEY, undefined);
    if (args[0] === 'rm') {
      cleaned.push(args[2]);
      assert.equal(settings.signal, undefined);
      return { stdout: args[2], stderr: '' };
    }
    const pairs = new Map(args.flatMap((arg, i) => arg.startsWith('--') ? [[arg, args[i + 1]]] : []));
    assert.equal(pairs.get('--pull'), 'never');
    assert.equal(pairs.get('--network'), 'none');
    assert.equal(pairs.get('--user'), '65534:65534');
    assert.equal(pairs.get('--cap-drop'), 'ALL');
    assert.equal(pairs.get('--security-opt'), 'no-new-privileges');
    assert.ok(args.includes('--read-only'));
    assert.equal(pairs.get('--memory'), '256m');
    assert.equal(pairs.get('--pids-limit'), '32');
    assert.ok(!args.includes('--privileged'));
    assert.ok(!args.includes('--env'));
    names.push(pairs.get('--name'));
    const mount = pairs.get('--mount');
    snapshot = mount.slice('type=bind,source='.length, mount.indexOf(',target='));
    assert.notEqual(snapshot, options.worktree);
    assert.deepEqual(await fs.readdir(snapshot), ['check.mjs']);
    assert.equal(await fs.readFile(path.join(snapshot, 'check.mjs'), 'utf8'), 'console.log("line 4");\n');
    return { stdout: args.includes('-e') ? 'v20.20.2\n' : 'line 4\n', stderr: '' };
  } });
  assert.equal(result.status, 'pass');
  assert.equal(result.node, 'v20.20.2');
  assert.equal(new Set(names).size, 2);
  assert.deepEqual(cleaned, names);
  await assert.rejects(fs.stat(snapshot), { code: 'ENOENT' });
});

test('execution contradictions return failing receipts, infrastructure errors never return success', async (context) => {
  const options = await fixture(context);
  const runCommand = async (command, args) => {
    if (args[0] === 'rm') return { stdout: '', stderr: '' };
    if (args.includes('-e')) return { stdout: 'v20.20.2\n', stderr: '' };
    return { stdout: 'line 3\n', stderr: '' };
  };
  assert.equal((await runDockerVerification({ ...options, runCommand })).status, 'fail');
  const cleaned = [];
  await assert.rejects(runDockerVerification({ ...options, runCommand: async (command, args) => {
    if (args[0] === 'rm') { cleaned.push(args[2]); return {}; }
    if (args.includes('-e')) return { stdout: 'v20.20.2\n', stderr: '' };
    throw Object.assign(new Error('Container failed to start'), { code: 125, stderr: 'unavailable' });
  } }), /failed to start/);
  assert.equal(cleaned.length, 2);
});

test('nonzero Node exit is observed and stderr is compared, not discarded', async (context) => {
  const options = await fixture(context);
  const result = await runDockerVerification({ ...options, plan: [{ ...options.plan[0],
    expected_stdout: '', expected_stderr: 'SyntaxError: line 4\n', expected_exit: 1 }],
  runCommand: async (command, args) => {
    if (args[0] === 'rm') return {};
    if (args.includes('-e')) return { stdout: 'v20.20.2\n', stderr: '' };
    throw Object.assign(new Error('Node rejected input'), { code: 1, stdout: '', stderr: 'SyntaxError: line 4\n' });
  } });
  assert.equal(result.status, 'pass');
  assert.equal(result.receipt.observations[0].actual_exit, 1);
});

test('timeouts and cancellation clean named containers and fail explicitly', async (context) => {
  const options = await fixture(context);
  for (const cancel of [false, true]) {
    const controller = new AbortController();
    const cleaned = [];
    await assert.rejects(runDockerVerification({ ...options, signal: controller.signal,
      runCommand: async (command, args, settings) => {
        if (args[0] === 'rm') {
          cleaned.push(args[2]);
          assert.equal(settings.signal, undefined);
          return {};
        }
        if (cancel) controller.abort(new Error('Explicit cancellation'));
        throw Object.assign(new Error('Test timeout'), { code: 'ETIMEDOUT', killed: true });
      } }), cancel ? /Explicit cancellation/ : /Test timeout/);
    assert.equal(cleaned.length, 1);
  }
});

test('cleanup failure is explicit, wrong Node version and changed subject are refused', async (context) => {
  const options = await fixture(context);
  await assert.rejects(runDockerVerification({ ...options, runCommand: async (command, args) => {
    if (args[0] === 'rm') throw new Error('cleanup denied');
    return { stdout: 'v20.20.2\n', stderr: '' };
  } }), /container cleanup failed/);
  await assert.rejects(runDockerVerification({ ...options, runCommand: async (command, args) =>
    args[0] === 'rm' ? {} : { stdout: 'v22.0.0\n', stderr: '' } }), /verified Linux Node20/);
  await assert.rejects(runDockerVerification({ ...options, runCommand: async (command, args) => {
    if (args[0] === 'rm') return {};
    if (args.includes('-e')) return { stdout: 'v20.20.2\n', stderr: '' };
    await fs.writeFile(path.join(options.worktree, 'check.mjs'), 'Changed after execution');
    return { stdout: 'line 4\n', stderr: '' };
  } }), /subject changed during execution/);
});

test('opt-in, immutable image, timeout, check coverage and script authority are required', async (context) => {
  const options = await fixture(context);
  for (const changes of [
    { enabled: false }, { image: 'node:20' }, { timeoutMs: 30001 }, { checkCount: 2 },
    { plan: [] }, { plan: [{ ...options.plan[0], script: '../outside.mjs' }] },
    { plan: [{ ...options.plan[0], script: '-e' }] },
    { plan: [{ ...options.plan[0], args: ['bad\nargument'] }] },
  ]) {
    await assert.rejects(runDockerVerification({ ...options, ...changes,
      runCommand: async () => assert.fail('Invalid plan must not invoke Docker') }));
  }
});

test('opt-in real Docker Node20 readonly, network and credential controls', {
  skip: !process.env.ROSTER_VERIFICATION_TEST_IMAGE,
}, async (context) => {
  const options = await fixture(context);
  await fs.writeFile(path.join(options.worktree, 'check.mjs'), `
import assert from 'node:assert/strict';
import fs from 'node:fs';
import net from 'node:net';
assert.equal(process.getuid(),65534);
assert.equal(process.env.GITHUB_APP_ID,undefined);
assert.equal(process.env.GH_TOKEN,undefined);
assert.equal(fs.existsSync('/product/.env'),false);
assert.throws(()=>fs.writeFileSync('/product/new.txt','denied'));
assert.throws(()=>fs.writeFileSync('/root/new.txt','denied'));
await new Promise((resolve,reject)=>{
 const socket=net.connect({host:'1.1.1.1',port:443});
 socket.setTimeout(500,()=>{socket.destroy();resolve()});
 socket.on('error',()=>resolve());
 socket.on('connect',()=>{socket.destroy();reject(new Error('External network unexpectedly reachable'))});
});
console.log('isolated');
`);
  const result = await runDockerVerification({ ...options,
    image: process.env.ROSTER_VERIFICATION_TEST_IMAGE,
    plan: [{ ...options.plan[0], expected_stdout: 'isolated\n' }],
  });
  assert.equal(result.status, 'pass', JSON.stringify(result));
  assert.match(result.node, /^v20\./);
  const rejected = await runDockerVerification({ ...options,
    image: process.env.ROSTER_VERIFICATION_TEST_IMAGE,
    plan: [{ ...options.plan[0], expected_stdout: 'different expected output\n' }],
  });
  assert.equal(rejected.status, 'fail');
  assert.notEqual(rejected.receipt.observations[0].actual_output,
    rejected.receipt.observations[0].expected_output);
});

test('host verifier sources use a separate readonly mount and frozen plan inputs', async (context) => {
  const options = await fixture(context);
  const verifiers = [{ name: 'oracle.mjs', source: "console.log('line 4');\n" }];
  const plan = [{ ...options.plan[0], script: 'verifier:oracle.mjs' }];
  const directories = new Set();
  const result = await runDockerVerification({ ...options, verifiers, plan,
    runCommand: async (command, args) => {
      if (args[0] === 'rm') return {};
      const mounts = args.flatMap((arg, index) => arg === '--mount' ? [args[index + 1]] : []);
      assert.equal(mounts.length, 2);
      for (const mount of mounts) {
        assert.ok(mount.endsWith(',readonly'));
        directories.add(mount.slice('type=bind,source='.length, mount.indexOf(',target=')));
      }
      const oracleMount = mounts.find((mount) => mount.includes('target=/verifier,'));
      const oracleRoot = oracleMount.slice('type=bind,source='.length, oracleMount.indexOf(',target='));
      assert.equal(await fs.readFile(path.join(oracleRoot, 'oracle.mjs'), 'utf8'), "console.log('line 4');\n");
      if (args.includes('-e')) {
        verifiers[0].source = 'throw new Error("mutated");';
        plan[0].script = '-e';
        plan[0].expected_stdout = 'forged';
        return { stdout: 'v20.20.2\n', stderr: '' };
      }
      assert.deepEqual(args.slice(-2), ['--', '/verifier/oracle.mjs']);
      return { stdout: 'line 4\n', stderr: '' };
    } });
  assert.equal(result.status, 'pass');
  for (const directory of directories) await assert.rejects(fs.stat(directory), { code: 'ENOENT' });
});

test('invalid host verifier source and script selection refuse before Docker invocation', async (context) => {
  const options = await fixture(context);
  const source = "console.log('ok');";
  for (const verifiers of [
    null, [{ name: '../oracle.mjs', source }], [{ name: 'oracle.mjs', source: '\0' }],
    [{ name: 'oracle.js', source }], [{ name: 'oracle.mjs', source, unexpected: true }],
    [{ name: 'oracle.mjs', source: 'x'.repeat(65_537) }],
    Array.from({ length: 17 }, (_, index) => ({ name: `v${index}.mjs`, source })),
    [{ name: 'oracle.mjs', source }, { name: 'oracle.mjs', source }],
    Array.from({ length: 5 }, (_, index) => ({ name: `v${index}.mjs`, source: 'x'.repeat(65_536) })),
  ]) await assert.rejects(runDockerVerification({ ...options, verifiers,
    runCommand: async () => assert.fail('Invalid host verifier must never execute') }));
  await assert.rejects(runDockerVerification({ ...options,
    plan: [{ ...options.plan[0], script: 'verifier:absent.mjs' }],
    runCommand: async () => assert.fail('Missing host verifier must never execute') }));
});

test('opt-in real Docker oracle assertions inspect approved products independently of coder output', {
  skip: !process.env.ROSTER_VERIFICATION_TEST_IMAGE,
}, async (context) => {
  const options = await fixture(context);
  await fs.writeFile(path.join(options.worktree, 'check.mjs'), 'export const line = 3;\n');
  const settings = { ...options, image: process.env.ROSTER_VERIFICATION_TEST_IMAGE,
    verifiers: [{ name: 'oracle.mjs', source: `
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {line} from '/product/check.mjs';
assert.throws(()=>fs.writeFileSync('/verifier/oracle.mjs','forged'));
assert.equal(line,4);
console.log('line 4');
` }], plan: [{ ...options.plan[0], script: 'verifier:oracle.mjs' }] };
  const rejected = await runDockerVerification(settings);
  assert.equal(rejected.status, 'fail');
  assert.equal(rejected.receipt.observations[0].actual_exit, 1);
  await fs.writeFile(path.join(options.worktree, 'check.mjs'), 'export const line = 4;\n');
  const accepted = await runDockerVerification(settings);
  assert.equal(accepted.status, 'pass', JSON.stringify(accepted));
  assert.notEqual(accepted.subject.digest, rejected.subject.digest);
});
