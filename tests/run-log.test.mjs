import assert from 'node:assert/strict';
import { existsSync, linkSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { createRunLog, readIssueLogs, readLastRunLog, RunLogError } from '../src/lib/run-log.mjs';

function fixture(t) {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-run-log-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  let text = '';
  return { repoRoot, session: 'roster-42-coder', env: {},
    errorOutput: { write(value) { text += String(value); } }, get text() { return text; } };
}

const config = { llm: { base_url: '', model: '' } };

test('seat events are timestamped, streamed immediately, persisted identically, and measure elapsed milliseconds', async (t) => {
  const options = fixture(t);
  let elapsed = 100;
  const logger = await createRunLog({ ...options,
    now: () => new Date('2026-09-30T22:00:00.000Z'), clock: () => elapsed });
  await logger.seat('planner', 'roster-42-planner', config, async (onEvent) => {
    assert.match(options.text, /start seat planner session=roster-42-planner/);
    assert.equal(readFileSync(logger.path, 'utf8'), options.text);
    await onEvent({ type: 'wrote', path: 'RECIPE.yml' });
    await onEvent({ type: 'wrote', path: 'TASK.md' });
    elapsed += 25;
    return { mode: 'stub' };
  });
  assert.equal(readFileSync(logger.path, 'utf8'), options.text);
  assert.match(options.text, /seat planner elapsed_ms=25 mode=stub\n$/);
  assert.ok(options.text.trimEnd().split('\n').every((line) => line.startsWith('2026-09-30T22:00:00.000Z ')));
  const status = await readLastRunLog({ repoRoot: options.repoRoot, session: options.session, env: {} });
  assert.equal(status.lastSeat, 'planner');
  assert.equal(status.lastLine, '2026-09-30T22:00:00.000Z seat planner elapsed_ms=25 mode=stub');
});

test('only metadata fields are logged, known secrets are redacted, and newline injection stays one line', async (t) => {
  const options = fixture(t);
  const secret = 'test-only-api-secret';
  const logger = await createRunLog({ ...options, env: { ROSTER_API_KEY: secret } });
  await logger.seat('coder', options.session, {
    llm: { model: 'served-model', base_url: 'http://example.test:8000/private-path' },
  }, async (onEvent) => {
    await onEvent({ type: 'tool', name: 'write_file', path: `src/${secret}\nfile.mjs`,
      content: 'PRIVATE_FILE_BODY', prompt: 'PRIVATE_PROMPT', completion: 'PRIVATE_COMPLETION' });
    await onEvent({ type: 'http', phase: 'ok', status: 200, headers: { Authorization: secret }, body: 'PRIVATE_BODY' });
    return { mode: 'llm' };
  });
  const contents = readFileSync(logger.path, 'utf8');
  assert.equal(contents, options.text);
  assert.match(contents, /model="served-model" host="example\.test:8000"/);
  assert.match(contents, /tool write_file path="src\/\[redacted\]\?file\.mjs"/);
  for (const value of [secret, '/private-path', 'PRIVATE_FILE_BODY', 'PRIVATE_PROMPT', 'PRIVATE_COMPLETION', 'PRIVATE_BODY', 'Authorization']) {
    assert.ok(!contents.includes(value));
  }
});

test('failed seats retain a class and elapsed time without persisting an exception message', async (t) => {
  const options = fixture(t);
  const logger = await createRunLog(options);
  await assert.rejects(logger.seat('planner', 'roster-42-planner', config, async () => {
    throw new TypeError('PRIVATE_ERROR_WITH_PROMPT');
  }), /PRIVATE_ERROR_WITH_PROMPT/);
  assert.match(options.text, /seat planner error class=TypeError/);
  assert.match(options.text, /seat planner elapsed_ms=\d+ mode=stub/);
  assert.doesNotMatch(options.text, /PRIVATE_ERROR_WITH_PROMPT/);
});

test('a new logger appends the next run to the session instead of truncating earlier events', async (t) => {
  const options = fixture(t);
  const first = await createRunLog(options);
  await first.seat('planner', 'roster-42-planner', config, async () => ({ mode: 'stub' }));
  const before = readFileSync(first.path, 'utf8');
  const second = await createRunLog(options);
  await second.seat('reviewer', 'roster-42-reviewer', config, async () => ({ queried: false }));
  assert.ok(readFileSync(first.path, 'utf8').startsWith(before));
  assert.equal((await readLastRunLog({ repoRoot: options.repoRoot, session: options.session })).lastSeat, 'reviewer');
});

test('bounded log-tail reads ignore an in-flight partial line and refuse transcript-shaped records', async (t) => {
  const options = fixture(t);
  const directory = path.join(options.repoRoot, '.roster', 'runs');
  mkdirSync(directory, { recursive: true });
  const file = path.join(directory, `${options.session}.log`);
  const line = '2026-09-30T22:00:00.000Z seat reviewer mode stub';
  writeFileSync(file, 'old text\n'.repeat(20000) + `${line}\npartial`);
  assert.equal((await readLastRunLog(options)).lastLine, line);
  writeFileSync(file, '2026-09-30T22:00:00.000Z seat coder prompt PRIVATE_PROMPT\n');
  await assert.rejects(readLastRunLog(options), /invalid metadata/);
});

test('run logs refuse hard links, symlinks, and unsafe session paths rather than writing outside the repository', async (t) => {
  const options = fixture(t);
  await assert.rejects(createRunLog({ ...options, session: '../unsafe' }), /session must be an opaque/);
  const logger = await createRunLog(options);
  await logger.seat('coder', options.session, config, async () => ({ mode: 'stub' }));
  const before = readFileSync(logger.path, 'utf8');
  const link = path.join(options.repoRoot, 'linked.log');
  linkSync(logger.path, link);
  await assert.rejects(logger.seat('coder', options.session, config, async () => ({ mode: 'stub' })),
    /regular, single-link file/);
  assert.equal(readFileSync(link, 'utf8'), before);
  const other = fixture(t);
  try {
    symlinkSync(options.repoRoot, path.join(other.repoRoot, '.roster'), process.platform === 'win32' ? 'junction' : 'dir');
  } catch (error) {
    if (['EPERM', 'EACCES', 'ENOTSUP'].includes(error.code)) {
      t.skip('Creating symlinks is unavailable on this system.');
      return;
    }
    throw error;
  }
  await assert.rejects(createRunLog(other), /symlinks/);
  assert.equal(existsSync(path.join(options.repoRoot, 'runs')), false);
});

test('issue tail reads all matching seat logs and excludes another issue', async (t) => {
  const options = fixture(t);
  for (const [session, seat] of [['roster-42-planner', 'planner'], ['roster-42-coder', 'coder'],
    ['roster-43-coder', 'coder']]) {
    const logger = await createRunLog({ ...options, session });
    await logger.seat(seat, session, config, async () => ({ mode: 'stub' }));
  }
  const logs = await readIssueLogs({ repoRoot: options.repoRoot, issue: 42, limit: 2 });
  assert.equal(logs.length, 2);
  assert.ok(logs.every((log) => log.session.startsWith('roster-42-') && log.lines.length === 2));
  await assert.rejects(readIssueLogs({ repoRoot: options.repoRoot, issue: 42, limit: 201 }), /tail limit/);
});

test('log append errors remain explicit and are never success-shaped fallbacks', async (t) => {
  const options = fixture(t);
  const logger = await createRunLog(options);
  mkdirSync(logger.path);
  await assert.rejects(logger.seat('coder', options.session, config, () => assert.fail('No seat should run')),
    (error) => error instanceof RunLogError && error.code === 'ROSTER_RUN_LOG');
  assert.equal(options.text, '');
});
