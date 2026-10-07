import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, relative, sep } from 'node:path';
import {
  MIGRATION_ERROR_CODES,
  MIGRATION_ID,
  MigrationError,
  detectActiveRun,
  migrate,
  migrationLayout,
  readMigrationRecord,
} from '../src/lib/repo-migrate.mjs';

const SECRET_SENTINEL = 'test-only-private-api-key';
// Credential-shaped prefixes are assembled at runtime so no literal
// credential-shaped value appears in source.
const CREDENTIAL_SHAPES = [['s', 'k-'], ['gh', 'p_'], ['github', '_pat_']]
  .map(([head, tail]) => new RegExp(`${head}${tail}[A-Za-z0-9_]{8,}`));

async function workspace(t) {
  const root = await fs.mkdtemp(join(tmpdir(), 'repo-migrate-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  return root;
}

// Relative path -> contents for every regular file under root. The migration
// directory (record + backup manifest) can be excluded for rollback checks.
async function tree(root, { skipMigrationDir = false } = {}) {
  const result = {};
  const stack = [root];
  while (stack.length > 0) {
    const dir = stack.pop();
    for (const entry of await fs.readdir(dir, { withFileTypes: true })) {
      const absolute = join(dir, entry.name);
      const rel = relative(root, absolute).split(sep).join('/');
      if (entry.isDirectory()) {
        if (skipMigrationDir && rel === '.roster-state/migration') continue;
        stack.push(absolute);
      } else if (entry.isFile()) {
        result[rel] = await fs.readFile(absolute, 'utf8');
      }
    }
  }
  return result;
}

async function present(path) {
  return fs.stat(path).then(() => true, () => false);
}

/** Representative pre-split layout: private config beside per-worktree state. */
async function legacyLayout(root) {
  const legacy = join(root, '.roster');
  await fs.mkdir(join(legacy, 'checkpoints', 'issue-7'), { recursive: true });
  await fs.mkdir(join(legacy, 'runs'), { recursive: true });
  await fs.mkdir(join(legacy, 'memory'), { recursive: true });
  await fs.writeFile(join(legacy, 'config.yml'), `api_key: ${SECRET_SENTINEL}\nmodel: test-model\n`);
  await fs.writeFile(join(legacy, 'fleet.yml'), 'profiles: {}\n');
  await fs.writeFile(join(legacy, 'checkpoints', 'issue-7', 'state.json'), '{"step":3}\n');
  await fs.writeFile(join(legacy, 'runs', 'runs.jsonl'), '{"run":1}\n');
  await fs.writeFile(join(legacy, 'runs', 'roster-7-coder.log'), 'coder log\n');
  await fs.writeFile(join(legacy, 'memory', 'coder.jsonl'), '{"note":"kept"}\n');
  return legacy;
}

function failingCopy(after) {
  let calls = 0;
  return {
    ...fs,
    copyFile: async (...args) => {
      calls += 1;
      if (calls > after) throw Object.assign(new Error('simulated disk failure'), { code: 'EIO' });
      return fs.copyFile(...args);
    },
  };
}

test('dry-run reports the plan and touches nothing', async (t) => {
  const root = await workspace(t);
  await legacyLayout(root);
  const before = await tree(root);

  const report = await migrate(root, { dryRun: true });

  assert.equal(report.dryRun, true);
  assert.equal(report.applied, false);
  assert.equal(report.sourceLayout, '.roster');
  assert.match(report.destinationLayout, /^\.roster-state\/worktrees\/wt-[0-9a-f]{16}$/);
  assert.equal(report.planned.files, 3);
  assert.ok(report.planned.bytes > 0);
  assert.equal(report.activeRun.active, false);
  assert.deepEqual(report.retained, ['config.yml', 'fleet.yml', 'memory']);
  assert.deepEqual(report.moves.map((move) => move.from).sort(), [
    '.roster/checkpoints/issue-7/state.json', '.roster/runs/roster-7-coder.log', '.roster/runs/runs.jsonl',
  ]);
  assert.deepEqual(await tree(root), before, 'dry-run writes, renames, deletes, and creates nothing');
  assert.equal(await present(join(root, '.roster-state')), false);
});

test('migration copies only state, keeps legacy files, and is idempotent', async (t) => {
  const root = await workspace(t);
  await legacyLayout(root);
  const legacyBefore = await tree(join(root, '.roster'));
  const layout = migrationLayout(root);

  const first = await migrate(root);
  assert.equal(first.applied, true);
  assert.deepEqual(await tree(join(root, '.roster')), legacyBefore, 'legacy state is never removed or rewritten');
  assert.equal(await fs.readFile(join(layout.destination, 'runs', 'runs.jsonl'), 'utf8'), '{"run":1}\n');
  assert.equal(await present(join(layout.destination, 'config.yml')), false, 'config never moves');
  assert.equal(await present(join(layout.destination, 'memory')), false, 'memory is not per-worktree state');
  const record = await readMigrationRecord(root);
  assert.equal(record.id, MIGRATION_ID);
  assert.equal(record.status, 'complete');
  assert.equal(record.files.length, 3);
  assert.ok(await present(layout.backup), 'the backup manifest is written under .roster-state');
  assert.equal(await present(join(root, 'migration.json')), false, 'nothing is written at the repo root');

  const afterFirst = await tree(root);
  const second = await migrate(root);
  assert.equal(second.planned.files, 0, 'second run plans zero changes');
  assert.equal(second.alreadyMigrated, 3);
  assert.deepEqual(await tree(root), afterFirst, 'second run leaves the tree and record identical');
});

test('legacy state that grew after a migration refreshes the copy it wrote', async (t) => {
  const root = await workspace(t);
  await legacyLayout(root);
  const layout = migrationLayout(root);
  await migrate(root);
  await fs.appendFile(join(root, '.roster', 'runs', 'runs.jsonl'), '{"run":2}\n');

  const report = await migrate(root);
  assert.equal(report.planned.files, 1);
  assert.equal(await fs.readFile(join(layout.destination, 'runs', 'runs.jsonl'), 'utf8'), '{"run":1}\n{"run":2}\n');
});

test('a destination changed by someone else is never overwritten', async (t) => {
  const root = await workspace(t);
  await legacyLayout(root);
  const layout = migrationLayout(root);
  await fs.mkdir(join(layout.destination, 'runs'), { recursive: true });
  await fs.writeFile(join(layout.destination, 'runs', 'runs.jsonl'), 'hand edited\n');
  const before = await tree(root);

  await assert.rejects(() => migrate(root), (error) => {
    assert.equal(error.code, MIGRATION_ERROR_CODES.E_CONFLICT);
    assert.match(error.message, /runs\.jsonl/);
    assert.match(error.message, /Recovery:/);
    return true;
  });
  assert.deepEqual(await tree(root), before);
});

test('mid-migration failure rolls back to the pre-migration tree byte-for-byte', async (t) => {
  const root = await workspace(t);
  await legacyLayout(root);
  const before = await tree(root);
  const layout = migrationLayout(root);

  await assert.rejects(() => migrate(root, { fileSystem: failingCopy(1) }), (error) => {
    assert.ok(error instanceof MigrationError);
    assert.equal(error.code, MIGRATION_ERROR_CODES.E_ROLLBACK);
    assert.ok(error.message.includes(root));
    assert.match(error.message, /simulated disk failure/);
    assert.match(error.message, /Recovery:/);
    return true;
  });
  const backup = JSON.parse(await fs.readFile(layout.backup, 'utf8'));
  assert.equal(backup.id, MIGRATION_ID, 'the backup was written before the first copy');
  assert.deepEqual(backup.prior, []);
  assert.deepEqual(await tree(root, { skipMigrationDir: true }), before);
  assert.equal(await readMigrationRecord(root), null, 'a rolled-back migration leaves no record');
});

test('a failed refresh rolls back to the earlier migrated bytes', async (t) => {
  const root = await workspace(t);
  await legacyLayout(root);
  await migrate(root);
  await fs.appendFile(join(root, '.roster', 'runs', 'runs.jsonl'), '{"run":2}\n');
  await fs.appendFile(join(root, '.roster', 'runs', 'roster-7-coder.log'), 'more\n');
  const before = await tree(root);

  await assert.rejects(() => migrate(root, { fileSystem: failingCopy(1) }),
    (error) => error.code === MIGRATION_ERROR_CODES.E_ROLLBACK);
  assert.deepEqual(await tree(root), before);
});

test('an interrupted migration is detected on the next run and completed', async (t) => {
  const root = await workspace(t);
  await legacyLayout(root);
  const layout = migrationLayout(root);
  // Crash state: in-progress record, one truncated copy, one stray temp file.
  const full = '{"run":1}\n';
  await fs.mkdir(join(layout.destination, 'runs'), { recursive: true });
  await fs.writeFile(join(layout.destination, 'runs', 'runs.jsonl'), full.slice(0, 4));
  await fs.writeFile(join(layout.destination, 'runs', 'roster-7-coder.log.migrate-tmp'), 'cod');
  await fs.mkdir(join(root, '.roster-state', 'migration'), { recursive: true });
  await fs.writeFile(layout.record, JSON.stringify({
    id: MIGRATION_ID, version: 1, status: 'in-progress', files: [
      { path: 'runs/runs.jsonl', sha256: createHash('sha256').update(full).digest('hex') },
    ],
  }));

  const dry = await migrate(root, { dryRun: true });
  assert.equal(dry.interrupted, true);
  assert.equal(dry.planned.repairs, 1, 'the partial copy is detected');
  assert.equal(dry.planned.temporaries, 1, 'the stray temp file is detected');

  await migrate(root);
  assert.equal(await fs.readFile(join(layout.destination, 'runs', 'runs.jsonl'), 'utf8'), full);
  assert.equal(await present(join(layout.destination, 'runs', 'roster-7-coder.log.migrate-tmp')), false);
  assert.equal((await readMigrationRecord(root)).status, 'complete');
  assert.equal((await migrate(root, { dryRun: true })).planned.files, 0);
});

test('a corrupt migration record fails closed with path and recovery steps', async (t) => {
  const root = await workspace(t);
  await legacyLayout(root);
  const layout = migrationLayout(root);
  await fs.mkdir(join(root, '.roster-state', 'migration'), { recursive: true });
  await fs.writeFile(layout.record, '{"id": "split-layout-v1", "files": [');
  const before = await tree(root);

  for (const dryRun of [true, false]) {
    await assert.rejects(() => migrate(root, { dryRun }), (error) => {
      assert.equal(error.code, MIGRATION_ERROR_CODES.E_CORRUPT_STATE);
      assert.ok(error.message.includes(layout.record));
      assert.match(error.message, /Recovery: .*backup\.json/);
      return true;
    });
  }
  assert.deepEqual(await tree(root), before);
});

test('active runs are detected from repo locks before any mutation', async (t) => {
  const root = await workspace(t);
  await legacyLayout(root);
  await fs.mkdir(join(root, '.roster', 'locks'), { recursive: true });
  const lock = join(root, '.roster', 'locks', 'issue-7.lock');
  await fs.writeFile(lock, JSON.stringify({ holder: 'roster-run-7', pid: 4242, acquiredAt: new Date().toISOString() }));
  const before = await tree(root);

  const alive = (pid) => pid === 4242;
  const status = await detectActiveRun(root, { isAlive: alive });
  assert.equal(status.active, true);
  assert.equal(status.locks[0].holder, 'roster-run-7');
  assert.equal((await migrate(root, { dryRun: true, isAlive: alive })).activeRun.active, true);
  await assert.rejects(() => migrate(root, { isAlive: alive }), (error) => {
    assert.equal(error.code, MIGRATION_ERROR_CODES.E_ACTIVE_RUN);
    assert.match(error.message, /roster-run-7/);
    assert.match(error.message, /Recovery:/);
    return true;
  });
  assert.deepEqual(await tree(root), before, 'the active run and every file are preserved');

  // Once the holder has exited, the lock is stale and the migration proceeds.
  const report = await migrate(root, { isAlive: () => false });
  assert.equal(report.applied, true);
});

test('secrets do not move: the sentinel stays in place and no new file holds it', async (t) => {
  const root = await workspace(t);
  await legacyLayout(root);
  const configPath = join(root, '.roster', 'config.yml');
  const configBefore = await fs.readFile(configPath, 'utf8');
  const before = await tree(root);

  await migrate(root);

  assert.equal(await fs.readFile(configPath, 'utf8'), configBefore, 'sentinel file remains in its original location');
  const after = await tree(root);
  const created = Object.keys(after).filter((path) => !(path in before));
  assert.ok(created.length >= 3, 'the migration created the split layout');
  for (const path of created) {
    assert.equal(after[path].includes(SECRET_SENTINEL), false, `${path} must not contain the sentinel`);
    for (const shape of CREDENTIAL_SHAPES) {
      assert.equal(shape.test(after[path]), false, `${path} must not contain a credential-shaped value`);
    }
  }
});
