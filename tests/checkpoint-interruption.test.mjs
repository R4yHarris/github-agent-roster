import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { promises as fs, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { captureCheckpoint, listCheckpoints, restoreVerifiedCheckpoint, rewindCheckpoint } from '../src/lib/checkpoints.mjs';
import { acquireRepoLock } from '../src/lib/repo-locks.mjs';
import { createDispatcher } from '../src/repl.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub, runtimeRecipe } from '../src/planner/stub.mjs';

function fixture(t) {
  const worktree = mkdtempSync(path.join(process.cwd(), '.checkpoint-interruption-'));
  t.after(() => rmSync(worktree, { recursive: true, force: true }));
  const git = (...args) => execFileSync('git', ['-c', 'user.name=fixture', '-c', 'user.email=fixture@example.invalid',
    '-c', 'commit.gpgsign=false', ...args], { cwd: worktree, encoding: 'utf8', stdio: 'pipe' });
  git('init', '-q', '-b', 'issue-108');
  mkdirSync(path.join(worktree, 'src'));
  writeFileSync(path.join(worktree,'src','a.mjs'), 'export const a = 1;\n');
  writeFileSync(path.join(worktree,'src','b.mjs'), 'export const b = 1;\n');
  const allowedFiles = ['src/**'];
  writeFileSync(path.join(worktree,'TASK.md'), planStub('Update src/a.mjs and src/b.mjs.').task.replace(/## Files allowed[\s\S]*?(?=## Ask)/, () => '## Files allowed\n- \x60src/**\x60\n\n'));
  writeFileSync(path.join(worktree,'RECIPE.yml'), runtimeRecipe('issue:108'));
  git('add','.'); git('commit','-qm','baseline fixture');
  return {worktree, task:'issue-108', allowedFiles, env:{}, hasPublishedPr:async () => false};
}
const bytes = (root) => ['a.mjs','b.mjs','c.mjs'].map((name) => {
  const file = path.join(root,'src',name); return existsSync(file) ? readFileSync(file) : null;
});

for (const kind of ['second-open-failure', 'partial-second-write-failure', 'partial-created-product-failure', 'cancel-first-write', 'cancel-final-write', 'cancel-final-delete']) {
  test('multi-file restoration is atomic and backup recoverable: ' + kind, async (t) => {
    const options = fixture(t);
    await captureCheckpoint({...options, verified:true, tests:{exit_code:0}});
    writeFileSync(path.join(options.worktree,'src','a.mjs'), Buffer.from([0,1,2,255,10]));
    writeFileSync(path.join(options.worktree,'src','b.mjs'), 'later b exact bytes\r\n');
    writeFileSync(path.join(options.worktree,'src','c.mjs'), 'later newly created product\n');
    if (kind === 'partial-created-product-failure') rmSync(path.join(options.worktree,'src','b.mjs'));
    if (kind === 'cancel-final-write') rmSync(path.join(options.worktree,'src','c.mjs'));
    const previous = bytes(options.worktree);
    const controller = new AbortController();
    const open = fs.open; const unlink = fs.unlink; let injected = false; let changedObserved = false;
    fs.open = async (...args) => {
      const target = path.resolve(String(args[0]));
      if (!injected && kind === 'second-open-failure' && target === path.join(options.worktree,'src','b.mjs')) {
        injected = true; changedObserved = !readFileSync(path.join(options.worktree,'src','a.mjs')).equals(previous[0]);
        throw Object.assign(new Error('controlled write interruption'), {code:'EIO'});
      }
      const handle = await open(...args);
      const write = handle.writeFile.bind(handle);
      handle.writeFile = async (content, ...rest) => {
        if (!injected && ['partial-second-write-failure','partial-created-product-failure'].includes(kind) && target === path.join(options.worktree,'src','b.mjs')) {
          injected = true; changedObserved = !readFileSync(path.join(options.worktree,'src','a.mjs')).equals(previous[0]);
          await write(Buffer.from('partial')); throw Object.assign(new Error('controlled partial write'), {code:'EIO'});
        }
        const result = await write(content,...rest);
        if (!injected && ((kind === 'cancel-first-write' && target === path.join(options.worktree,'src','a.mjs')) ||
            (kind === 'cancel-final-write' && target === path.join(options.worktree,'src','b.mjs')))) {
          injected = true; changedObserved = !readFileSync(target).equals(previous[0]); controller.abort(new Error('controlled cancellation'));
        }
        return result;
      };
      return handle;
    };
    fs.unlink = async (...args) => {
      const result = await unlink(...args);
      if (!injected && kind === 'cancel-final-delete' && path.resolve(String(args[0])) === path.join(options.worktree,'src','c.mjs')) {
        injected = true; changedObserved = !existsSync(args[0]); controller.abort(new Error('controlled final cancellation'));
      }
      return result;
    };
    try {
      await assert.rejects(restoreVerifiedCheckpoint({...options, signal:controller.signal}), /prior product state was restored|cancel/i);
    } finally { fs.open = open; fs.unlink = unlink; }
    assert.equal(injected,true);
    assert.equal(changedObserved,true,'injection must occur after actual product mutation');
    assert.deepEqual(bytes(options.worktree), previous,'all prior product bytes/new files must survive');
    assert.equal(existsSync(path.join(options.worktree,'RESULT.md')),false);
    assert.equal(existsSync(path.join(options.worktree,'REVIEW.md')),false);
    const lock = await acquireRepoLock(options.task,{lockRoot:path.join(options.worktree,'.git','roster','wave-runs')});
    await lock.release();
    const records = await listCheckpoints(options);
    assert.equal(records.length,2);
    assert.equal(records[1].status,'before-write');
    await restoreVerifiedCheckpoint(options);
    assert.equal(readFileSync(path.join(options.worktree,'src','a.mjs'),'utf8'),'export const a = 1;\n');
    assert.equal(existsSync(path.join(options.worktree,'src','c.mjs')),false);
    await rewindCheckpoint({...options, number:records[1].number});
    assert.deepEqual(bytes(options.worktree), previous,'saved later edits must remain recoverable');
  });
}

test('interrupted shell green resume launches no seat or publisher and preserves later bytes', async (t) => {
  const options = fixture(t);
  await captureCheckpoint({...options, verified:true, tests:{exit_code:0}});
  writeFileSync(path.join(options.worktree,'src','a.mjs'),'later a');
  writeFileSync(path.join(options.worktree,'src','b.mjs'),'later b');
  const before = bytes(options.worktree); let launched = 0; let injected = false;
  const config = parseConfig(readFileSync(new URL('../roster.config.example.yml',import.meta.url),'utf8'));
  const output = {text:'',write(value){this.text+=value;}};
  const shell = createDispatcher({cwd:options.worktree,config,env:{},output,errorOutput:output,services:{
    repositoryRoot:() => options.worktree,
    readLocalRun:async () => ({worktreePath:options.worktree, task:options.task, issue:{number:108}, planMode:false}),
    restoreVerifiedCheckpoint:(request) => restoreVerifiedCheckpoint({...request,hasPublishedPr:options.hasPublishedPr}),
    runBuiltinIssue:() => {launched++; assert.fail('no execution or publication after failed restore');},
  }});
  const open = fs.open;
  fs.open = async (...args) => {
    if (!injected && path.resolve(String(args[0])) === path.join(options.worktree,'src','b.mjs')) {
      injected=true; throw Object.assign(new Error('controlled shell restore failure'),{code:'EIO'});
    }
    return open(...args);
  };
  try { await assert.rejects(shell.dispatch('/resume 108 --green'),/prior product state was restored/); }
  finally { fs.open=open; }
  assert.equal(injected,true);
  assert.equal(launched,0);
  assert.equal(shell.state.pendingConfirm,null);
  assert.deepEqual(bytes(options.worktree),before);
  assert.doesNotMatch(output.text,/Restored verified checkpoint/);
  const lock=await acquireRepoLock(options.task,{lockRoot:path.join(options.worktree,'.git','roster','wave-runs')});
  await lock.release();
});

test('rollback I/O failure is reported honestly while backup and lock remain usable', async (t) => {
  const options=fixture(t);
  await captureCheckpoint({...options,verified:true,tests:{exit_code:0}});
  writeFileSync(path.join(options.worktree,'src','a.mjs'),'later a');
  writeFileSync(path.join(options.worktree,'src','b.mjs'),'later b');
  const before=bytes(options.worktree); const open=fs.open; let failRollback=false;
  fs.open=async (...args) => {
    const target=path.resolve(String(args[0]));
    if (!failRollback && target===path.join(options.worktree,'src','b.mjs')) {
      failRollback=true; throw Object.assign(new Error('controlled forward failure'),{code:'EIO'});
    }
    if (failRollback && target===path.join(options.worktree,'src','a.mjs')) {
      throw Object.assign(new Error('controlled rollback failure'),{code:'EIO'});
    }
    return open(...args);
  };
  try { await assert.rejects(restoreVerifiedCheckpoint(options),/rollback failed; inspect product files/); }
  finally {fs.open=open;}
  assert.equal(failRollback,true);
  const lock=await acquireRepoLock(options.task,{lockRoot:path.join(options.worktree,'.git','roster','wave-runs')});
  await lock.release();
  const records=await listCheckpoints(options);
  assert.equal(records.length,2);
  await rewindCheckpoint({...options,number:records[1].number});
  assert.deepEqual(bytes(options.worktree),before);
});
