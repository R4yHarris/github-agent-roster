import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { checkCopilotDoctor, formatCopilotDoctor, parseCopilotDoctorArgs } from '../src/lib/copilot-doctor.mjs';

const cli = fileURLToPath(new URL('../src/cli.mjs', import.meta.url));
const guard = `import childProcess from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import https from 'node:https';
import net from 'node:net';
import tls from 'node:tls';
import {syncBuiltinESMExports} from 'node:module';
const deny = () => { throw new Error('Offline diagnostics attempted a forbidden operation'); };
for (const name of ['spawn', 'spawnSync', 'exec', 'execSync', 'execFile', 'execFileSync', 'fork']) childProcess[name] = deny;
const sourceOnly = (read) => (path, ...args) => {
  if (!/(?:\\.mjs|package\\.json)$/.test(String(path))) deny();
  return read(path, ...args);
};
fs.readFileSync = sourceOnly(fs.readFileSync);
fs.readFile = sourceOnly(fs.readFile);
fs.promises.readFile = sourceOnly(fs.promises.readFile);
for (const name of ['writeFile', 'writeFileSync', 'appendFile', 'appendFileSync', 'unlink', 'unlinkSync', 'mkdir', 'mkdirSync', 'rename', 'renameSync', 'rm', 'rmSync']) fs[name] = deny;
for (const name of ['writeFile', 'appendFile', 'unlink', 'mkdir', 'rename', 'rm']) fs.promises[name] = deny;
http.request = http.get = https.request = https.get = deny;
net.connect = net.createConnection = tls.connect = deny;
globalThis.fetch = deny;
syncBuiltinESMExports();`;
const guardUrl = `data:text/javascript,${encodeURIComponent(guard)}`;

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'roster-copilot-doctor-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const runtimePath = join(root, 'copilot.exe');
  writeFileSync(runtimePath, 'synthetic executable marker; never executed\n');
  mkdirSync(join(root, '.roster'));
  writeFileSync(join(root, '.roster', 'config.yml'), 'invalid private config marker\n');
  return { root, runtimePath };
}

function run(args, cwd) {
  // Synthetic credentials only. The doctor process is denied subprocesses,
  // private file reads, writes and fetch; ESM source loading remains available.
  const env = { SystemRoot: process.env.SystemRoot ?? '', PATH: '',
    GITHUB_APP_ID: 'app-secret-sentinel',
    GITHUB_APP_PRIVATE_KEY_PATH: 'private-key-sentinel.pem',
    GH_TOKEN: 'gh-secret-sentinel', GITHUB_TOKEN: 'github-secret-sentinel',
    COPILOT_GITHUB_TOKEN: 'copilot-secret-sentinel',
    HOME: cwd, USERPROFILE: cwd };
  return spawnSync(process.execPath, ['--import', guardUrl, cli, 'doctor', ...args],
    { cwd, env, encoding: 'utf8', timeout: 10_000 });
}

test('no opt-in skips metadata inspection and never reports enabled or qualified', () => {
  const result = checkCopilotDoctor({ runtimePath: 'ignored-runtime.exe', model: 'exact-model',
    inspect: () => assert.fail('Opt-out must not inspect'), isTTY: false });
  assert.equal(result.ok, false);
  assert.equal(result.enabled, false);
  assert.equal(result.qualified, false);
  assert.equal(result.evidence, 'offline-declared');
  assert.equal(result.checks.length, 8);
  const output = formatCopilotDoctor(result);
  assert.match(output, /Copilot remains off/);
  assert.match(output, /inspection skipped/);
  assert.match(output, /No interactive terminal/);
  assert.match(output, /#571/);
});

test('present runtime file is metadata only; auth, entitlement and capability remain blocked', (t) => {
  const { runtimePath } = fixture(t);
  let inspected = 0;
  const result = checkCopilotDoctor({ optIn: true, runtimePath, model: 'org/exact-model:1',
    isTTY: true, inspect(path) { inspected += 1; assert.equal(path, runtimePath); return lstatSync(path); } });
  assert.equal(inspected, 1);
  assert.equal(result.checks[1].ok, true);
  assert.equal(result.checks[2].ok, false);
  assert.equal(result.enabled, false);
  assert.equal(result.ok, false);
  assert.equal(result.checks[4].ok, false);
  assert.equal(result.checks[6].ok, false);
  const output = formatCopilotDoctor(result);
  assert.match(output, /identity, version, platform support and containment are unqualified/);
  assert.match(output, /Authentication is unverified/);
  assert.match(output, /does not prove access/);
  assert.ok(!JSON.stringify(result).includes(runtimePath));
  assert.ok(!output.includes('org/exact-model:1'));
});

for (const [name, inspect, expected] of [
  ['missing file', () => { throw Object.assign(new Error('token=provider-secret'), { code: 'ENOENT' }); }, /Runtime file is missing/],
  ['missing parent', () => { throw Object.assign(new Error('token=provider-secret'), { code: 'ENOTDIR' }); }, /Runtime file is missing/],
  ['permission denied', () => { throw Object.assign(new Error('token=provider-secret'), { code: 'EACCES' }); }, /metadata permission denied/],
  ['permission denied Windows', () => { throw Object.assign(new Error('token=provider-secret'), { code: 'EPERM' }); }, /metadata permission denied/],
  ['directory', () => ({ isFile: () => false, isSymbolicLink: () => false }), /not a regular nonsymlink file/],
  ['symlink', () => ({ isFile: () => true, isSymbolicLink: () => true }), /not a regular nonsymlink file/],
]) {
  test(`runtime ${name} fails explicitly and redacts filesystem error bodies`, () => {
    const result = checkCopilotDoctor({ optIn: true, runtimePath: 'secret-path.pem', inspect });
    const output = formatCopilotDoctor(result);
    assert.equal(result.checks[1].ok, false);
    assert.match(output, expected);
    assert.doesNotMatch(JSON.stringify(result) + output, /provider-secret|secret-path\.pem/);
  });
}

test('missing runtime path is unverified rather than installation discovery', () => {
  const result = checkCopilotDoctor({ optIn: true, inspect: () => assert.fail('No path was supplied') });
  assert.match(formatCopilotDoctor(result), /No runtime path supplied/);
  assert.equal(result.checks[1].ok, false);
});

const unsafeRuntimePaths = [
  '\\\\secret-server\\share\\copilot.exe', '//secret-server/share/copilot.exe',
  '\\/secret-server/share/copilot.exe', '/\\secret-server\\share\\copilot.exe',
  '\\\\?\\UNC\\secret-server\\share\\copilot.exe', '\\\\?\\C:\\secret-path\\copilot.exe',
  '\\\\.\\secret-device', '\\??\\C:\\secret-path\\copilot.exe',
  'file:///secret-path/copilot.exe', 'https://secret-server/copilot.exe',
  'smb://secret-server/share/copilot.exe', 'data:secret-value',
];

test('network/device/URI runtime paths are rejected before inspect even without opt-in', () => {
  for (const optIn of [false, true]) {
    for (const runtimePath of unsafeRuntimePaths) {
      assert.throws(() => checkCopilotDoctor({ optIn, runtimePath,
        inspect: () => assert.fail('Unsafe paths must never reach filesystem inspection') }), (error) => {
        assert.ok(error instanceof TypeError);
        assert.match(error.message, /network, device namespace and URI paths are not allowed/);
        assert.doesNotMatch(error.message, /secret-server|secret-path|secret-device|secret-value/);
        return true;
      });
    }
  }
});

test('all ASCII C0 and DEL runtime characters are rejected before inspect for opt-in and opt-out', () => {
  for (const optIn of [false, true]) {
    for (const code of [...Array.from({ length: 32 }, (_, index) => index), 127]) {
      assert.throws(() => checkCopilotDoctor({ optIn, runtimePath: `secret${String.fromCharCode(code)}path`,
        inspect: () => assert.fail('Control characters must never reach filesystem inspection') }), (error) => {
        assert.ok(error instanceof TypeError);
        assert.match(error.message, /without ASCII control characters/);
        assert.doesNotMatch(error.message, /secret/);
        return true;
      });
    }
  }
});

test('normal local relative and absolute paths preserve literal internal spaces without claiming locality proof', () => {
  for (const runtimePath of ['copilot.exe', '.\\local runtime\\copilot.exe',
    'C:\\local runtime\\copilot.exe', 'C:/local runtime/copilot.exe',
    'C:local runtime\\copilot.exe', '/local runtime/copilot']) {
    const inspected = [];
    const result = checkCopilotDoctor({ optIn: true, runtimePath, inspect(path) {
      inspected.push(path);
      return { isFile: () => true, isSymbolicLink: () => false };
    } });
    assert.deepEqual(inspected, [runtimePath]);
    assert.equal(result.qualified, false);
    assert.match(formatCopilotDoctor(result), /Mounted-drive locality is not proven/);
  }
});

for (const [authState, expected] of [
  ['signed-out', /Declared signed out/], ['expired', /Declared expired/],
  ['revoked', /Declared revoked/], ['policy-disabled', /Declared Copilot policy disabled/],
  ['permission-denied', /Declared permission denied/],
]) {
  test(`declared ${authState} fixture is actionable, not live authentication evidence`, () => {
    const result = checkCopilotDoctor({ optIn: true, authState,
      providerBody: 'token=provider-secret', token: 'raw-secret' });
    assert.equal(result.ok, false);
    assert.equal(result.checks[4].ok, false);
    const output = formatCopilotDoctor(result);
    assert.match(output, expected);
    assert.doesNotMatch(JSON.stringify(result) + output, /provider-secret|raw-secret/);
  });
}

for (const authSource of ['environment', 'stored-oauth', 'gh-fallback', 'separate-inference']) {
  test(`declared ${authSource} auth source never authorizes or switches inference`, () => {
    const result = checkCopilotDoctor({ optIn: true, authSource });
    assert.equal(result.checks[5].ok, false);
    assert.equal(result.enabled, false);
    const output = formatCopilotDoctor(result);
    assert.match(output, /environment tokens take precedence/);
    assert.match(output, /stored CLI OAuth and possible gh fallback/);
    assert.match(output, /Never forward ambient tokens, GITHUB_APP_\*/);
    assert.match(output, /Logout alone does not remove an environment-token override/);
  });
}

for (const modelState of ['unavailable', 'policy-disabled']) {
  test(`declared ${modelState} model does not fall back or imply entitlement`, () => {
    const result = checkCopilotDoctor({ optIn: true, model: 'exact-model', modelState });
    assert.equal(result.checks[6].ok, false);
    assert.equal(result.ok, false);
    assert.match(formatCopilotDoctor(result), modelState === 'unavailable'
      ? /do not silently select another model/ : /Roster cannot enable the model/);
  });
}

test('invalid diagnostics options and unexpected inspection errors fail explicitly without raw values', () => {
  for (const options of [{ optIn: 'yes' }, { isTTY: 1 }, { authState: 'provider-secret' },
    { authSource: 'provider-secret' }, { modelState: 'provider-secret' }, { runtimePath: '' },
    { runtimePath: '\nsecret' }, { inspect: null },
    ...['unknown', 'none', 'n/a', ' model ', 'Bearer secret', ''].map((model) => ({ model }))]) {
    assert.throws(() => checkCopilotDoctor(options), (error) => {
      assert.ok(error instanceof TypeError);
      assert.doesNotMatch(error.message, /provider-secret|Bearer secret/);
      return true;
    });
  }
  assert.throws(() => checkCopilotDoctor({ optIn: true, runtimePath: 'private-path',
    inspect() { throw new Error('private-path token=provider-secret'); } }),
  /^Error: Runtime metadata inspection failed; review the supplied path and filesystem access\.$/);
  assert.throws(() => formatCopilotDoctor({ ok: true }), /Expected blocked/);
});

test('strict shared option parser accepts only one explicit diagnostic choice per flag', () => {
  assert.deepEqual(parseCopilotDoctorArgs([]), { optIn: false });
  assert.deepEqual(parseCopilotDoctorArgs(['--runtime', 'C:\\fixture path\\copilot.exe',
    '--opt-in', '--model', 'exact-model']), {
    optIn: true, runtimePath: 'C:\\fixture path\\copilot.exe', model: 'exact-model',
  });
  for (const args of [['--warm'], ['--login'], ['--opt-in', '--opt-in'],
    ['--runtime'], ['--model', '--opt-in'], ['--runtime', ''], ['--model', 'id', '--model', 'id'],
    ['--opt-in', 'secret-code']]) {
    assert.throws(() => parseCopilotDoctorArgs(args), /Use roster doctor --copilot/);
  }
});

test('CLI non-TTY opt-out and opt-in never launch/read auth/probe/write defaults', (t) => {
  const { root, runtimePath } = fixture(t);
  const configFile = join(root, '.roster', 'config.yml');
  const before = readFileSync(configFile, 'utf8');
  const beforeFiles = readdirSync(root);
  for (const args of [['--copilot'], ['--copilot', '--opt-in', '--runtime', runtimePath,
    '--model', 'exact-model'], ['--copilot', '--opt-in', '--runtime', join(root, 'missing.exe')]]) {
    const result = run(args, root);
    assert.ifError(result.error);
    assert.equal(result.status, 1, result.stderr);
    assert.equal(result.stderr, '');
    assert.match(result.stdout, /Copilot offline diagnostics: BLOCKED/);
    assert.match(result.stdout, /No interactive terminal/);
    assert.match(result.stdout, /Authentication is unverified/);
    assert.doesNotMatch(result.stdout, /app-secret-sentinel|private-key-sentinel|gh-secret-sentinel|github-secret-sentinel|copilot-secret-sentinel/);
  }
  assert.equal(readFileSync(configFile, 'utf8'), before);
  assert.deepEqual(readdirSync(root), beforeFiles);
});

test('CLI rejects login/warm mixed with offline diagnostics without printing option bodies', (t) => {
  const { root } = fixture(t);
  for (const args of [['--copilot', '--login', 'secret-code'], ['--copilot', '--warm'],
    ['--copilot', '--model', 'Bearer secret']]) {
    const result = run(args, root);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Use roster doctor --copilot|Requested model must be/);
    assert.equal(result.stdout, '');
    assert.doesNotMatch(result.stderr, /secret-code|Bearer secret/);
  }
});

test('CLI rejects unsafe runtime paths before lstat with redacted explicit errors', (t) => {
  const { root } = fixture(t);
  const denyInspectGuard = `${guard}
fs.lstatSync = () => { throw new Error('Unsafe runtime path reached inspect'); };
syncBuiltinESMExports();`;
  for (const optIn of [false, true]) {
    for (const runtimePath of [unsafeRuntimePaths[0], unsafeRuntimePaths[1],
      unsafeRuntimePaths[4], unsafeRuntimePaths[6], unsafeRuntimePaths[8],
      'secret\tpath', 'secret\x1bpath', 'secret\x7fpath']) {
      const result = spawnSync(process.execPath, ['--import',
        `data:text/javascript,${encodeURIComponent(denyInspectGuard)}`, cli, 'doctor', '--copilot',
        ...(optIn ? ['--opt-in'] : []), '--runtime', runtimePath],
      { cwd: root, env: { SystemRoot: process.env.SystemRoot ?? '', PATH: '', HOME: root, USERPROFILE: root },
        encoding: 'utf8', timeout: 10_000 });
      assert.ifError(result.error);
      assert.equal(result.status, 1);
      assert.equal(result.stdout, '');
      assert.match(result.stderr, /Runtime path must be/);
      assert.doesNotMatch(result.stderr, /secret|reached inspect/);
    }
  }
});
