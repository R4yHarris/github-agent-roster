import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { reviewedChecks } from './helpers/review.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { parseDefects, runReviewer } from '../src/seats/reviewer.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:1234/v1')
  .replace('model: ""', 'model: review-model'));

const defect = { file: 'src/app.mjs', symbol: 'release', input: 'release() twice by one holder at capacity 2',
  outcome: 'a third holder is admitted beyond capacity' };

function fixture(context) {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-reviewer-defects-'));
  context.after(() => rmSync(worktree, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 }));
  mkdirSync(path.join(worktree, 'src'));
  writeFileSync(path.join(worktree, 'TASK.md'), '# Task: Update app\n\ndifficulty: 2\ntask_class: feat\n\n' +
    '## Acceptance checks\n- app exports ready\n\n## Files allowed\n- `src/app.mjs`\n\n## Ask\nUpdate app.\n');
  const source = path.join(worktree, 'src', 'app.mjs');
  writeFileSync(source, 'export const ready = false;\n');
  const git = (...args) => execFileSync('git', args, { cwd: worktree, encoding: 'utf8' });
  git('init', '-q', '-b', 'main');
  git('add', '--all');
  git('-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-q', '-m', 'Fixture');
  writeFileSync(source, 'export const ready = true;\n');
  const resultPath = path.join(worktree, 'RESULT.md');
  writeFileSync(resultPath, '# Result\n\n## Verification\n\nChecks: PASS\n- node --test exited 0\n');
  return { worktree, repoRoot: worktree, coderResult: { mode: 'llm', model: 'review-model', resultPath,
    excellence: { pass: true, files: ['src/app.mjs'] } } };
}

function review(options, extra) {
  const seen = [];
  return runReviewer({ ...options, config, env: {}, askKind: 'slice',
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      seen.push(body.messages[0].content);
      const content = JSON.stringify({ verdict: 'pass', reasons: [], security_notes: [],
        checks: reviewedChecks(body).map((id) => ({ id, met: true, evidence: `Check ${id} is in the diff.` })),
        ...extra });
      return { status: 200, json: async () => ({ choices: [{ finish_reason: 'stop',
        message: { role: 'assistant', content } }] }) };
    } }).then((result) => ({ result, seen }));
}

test('a concrete defect fails the review even when every check is met', async (context) => {
  const { result, seen } = await review(fixture(context), { defects: [defect] });
  assert.equal(result.verdict, 'fail');
  assert.ok(result.reasons.includes(
    'Defect in src/app.mjs release: release() twice by one holder at capacity 2 -> a third holder is admitted beyond capacity'));
  assert.match(readFileSync(result.reviewPath, 'utf8'), /Verdict: fail[\s\S]*Defect in src\/app\.mjs release/);
  assert.match(seen[0], /bypasses the repository redaction path/);
  assert.match(seen[0], /releasable by the wrong holder/);
});

test('a reply without defects keeps the check-based verdict', async (context) => {
  const { result } = await review(fixture(context), {});
  assert.equal(result.verdict, 'pass', result.content);
});

test('reviewer asks for original-Ask fidelity even when a planner check and green tests agree on wrong semantics', async (context) => {
  const options = fixture(context);
  writeFileSync(path.join(options.worktree, 'TASK.md'), '# Task: Integer parity\n\n' +
    'difficulty: 2\ntask_class: feat\n\n## Acceptance checks\n- isEven(0) returns false\n\n' +
    '## Files allowed\n- `src/app.mjs`\n\n## Ask\n' +
    'Create isEven(value) for safe integers, including positive, negative and zero values.\n');
  writeFileSync(path.join(options.worktree, 'src', 'app.mjs'),
    'export function isEven(value) { return value !== 0 && value % 2 === 0; }\n');
  const result = await runReviewer({ ...options, config, env: {}, askKind: 'slice',
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      assert.match(body.messages[0].content, /Original Ask is authoritative/);
      assert.match(body.messages[0].content, /incorrect observable result.*mathematical invariant/);
      assert.match(body.messages[1].content, /isEven\(0\) returns false/);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: JSON.stringify({ verdict: 'pass', reasons: [], security_notes: [],
          checks: [{ id: 1, met: true, evidence: 'src/app.mjs explicitly returns false for zero.' }],
          defects: [{ file: 'src/app.mjs', symbol: 'isEven', input: 'isEven(0)',
            outcome: 'returns false although zero is even; planner check contradicts the requested parity operation' }],
        }),
      } }] });
    },
  });
  assert.equal(result.verdict, 'fail');
  assert.match(result.content, /zero is even/);
});

for (const stage of ['ask-audit', 'checklist']) {
  test(`reviewer traces multiline diagnostics across requested behavior interactions in ${stage}`, async (context) => {
    const options = fixture(context);
    mkdirSync(path.join(options.worktree, 'medium-csv'));
    writeFileSync(path.join(options.worktree, 'medium-csv', 'parseCsv.mjs'),
      'export function rowError(recordIndex) { return new SyntaxError(`Row on line ${recordIndex + 1}`); }\n');
    options.coderResult.excellence.files = ['medium-csv/parseCsv.mjs'];
    writeFileSync(path.join(options.worktree, 'TASK.md'), '# Task: CSV diagnostics\n\n' +
      'difficulty: 2\ntask_class: feat\n\n## Acceptance checks\n' +
      '- Quoted newlines work and wrong field counts throw SyntaxError naming the line\n\n' +
      '## Files allowed\n- `medium-csv/parseCsv.mjs`\n\n## Ask\n' +
      'Parse CSV with newlines inside quoted fields and throw SyntaxError naming the line for a wrong field count. ' +
      'Output it in a new "medium-csv" directory.\n');
    let calls = 0;
    const result = await runReviewer({ ...options, config, env: {}, askKind: 'slice',
      fetchImpl: async (_url, request) => {
        const body = JSON.parse(request.body);
        calls += 1;
        assert.match(body.messages[0].content, /Trace relevant interactions between requested behaviors/);
        assert.match(body.messages[0].content, /green isolated tests do not prove combined correctness/);
        assert.match(body.messages[0].content, /physical source positions.*record indexes/);
        assert.match(body.messages[0].content, /Do not invent unrelated combinations or extra requirements/);
        const audit = calls === 1;
        const fail = stage === 'ask-audit' && audit;
        return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
          content: JSON.stringify({ verdict: fail ? 'fail' : 'pass',
            reasons: fail ? ['medium-csv/parseCsv.mjs reports record index rather than physical line after a quoted newline.'] : [],
            security_notes: [], checks: [{ id: 1, met: !fail,
              evidence: 'medium-csv/parseCsv.mjs computes line as recordIndex + 1.' }],
            ...(fail ? { grounds: [{
              obligation: 'throw SyntaxError naming the line for a wrong field count',
              file: 'medium-csv/parseCsv.mjs',
              excerpt: 'new SyntaxError(`Row on line ${recordIndex + 1}`)',
              reason: 'medium-csv/parseCsv.mjs reports record index rather than physical line after a quoted newline.',
            }] } : {}),
            ...(!audit && stage === 'checklist' ? { defects: [{
              file: 'medium-csv/parseCsv.mjs', symbol: 'rowError',
              input: 'a,b\\n"first\\nsecond",ok\\nx',
              outcome: 'reports line 3 instead of physical line 4 after a quoted multiline record',
            }] } : {}),
          }),
        } }] });
      },
    });
    assert.equal(calls, 2);
    assert.equal(result.verdict, 'fail');
    assert.equal(result.completed, true);
    assert.match(result.content, /physical line/);
  });
}

test('reviewer preserves required written setup instructions without demanding deployment execution', async (context) => {
  const options = fixture(context);
  writeFileSync(path.join(options.worktree, 'TASK.md'), '# Task: Marketing setup\n\n' +
    'difficulty: 1\ntask_class: docs\n\n## Acceptance checks\n- Guide explains localhost preview\n\n' +
    '## Files allowed\n- `src/app.mjs`\n\n## Ask\n' +
    'Write setup instructions for obtaining and serving a public web page to market a business. Do not deploy it.\n');
  const result = await runReviewer({ ...options, config, env: {}, askKind: 'slice',
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      assert.match(body.messages[0].content, /written setup.*not permission to execute/);
      assert.match(body.messages[0].content, /local-only preview.*public-serving instructions/);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: JSON.stringify({ verdict: 'fail', reasons: [
          'Original Ask requires public-serving instructions; localhost preview alone cannot market the business.',
        ], security_notes: [], checks: [{ id: 1, met: true, evidence: 'Preview steps exist.' }] }),
      } }] });
    },
  });
  assert.equal(result.verdict, 'fail');
  assert.match(result.content, /public-serving instructions/);
});

test('defects must name a changed file and carry exactly four one-line fields', () => {
  assert.deepEqual(parseDefects(undefined, ['src/app.mjs']), []);
  assert.deepEqual(parseDefects([defect], ['src\\app.mjs']), [defect]);
  assert.throws(() => parseDefects([{ ...defect, file: 'src/other.mjs' }], ['src/app.mjs']), /changed file/);
  const { outcome, ...missing } = defect;
  assert.throws(() => parseDefects([missing], ['src/app.mjs']), /changed file/);
  assert.throws(() => parseDefects([{ ...defect, extra: 'x' }], ['src/app.mjs']), /changed file/);
  assert.throws(() => parseDefects([{ ...defect, input: '' }], ['src/app.mjs']), /changed file/);
  assert.throws(() => parseDefects(Array(9).fill(defect), ['src/app.mjs']), /at most 8/);
  assert.throws(() => parseDefects({}, ['src/app.mjs']), /at most 8/);
});

const transferStep = 'In Netlify choose manual deploy and drop the folder containing index.html into the upload area.';

function directoryFixture(context) {
  const options = fixture(context);
  mkdirSync(path.join(options.worktree, 'new-design'));
  writeFileSync(path.join(options.worktree, 'new-design', 'guide.md'),
    `# Preview\nUse localhost for local preview.\n\n${transferStep}\n`);
  options.coderResult.excellence.files = ['new-design/guide.md'];
  writeFileSync(path.join(options.worktree, 'TASK.md'), '# Task: Marketing setup\n\n' +
    'difficulty: 1\ntask_class: docs\n\n## Acceptance checks\n- The guide explains local preview\n\n' +
    '## Files allowed\n- `new-design/guide.md`\n\n## Ask\n' +
    'Write setup instructions for serving a public web page to market a business. ' +
    'Output it in a new "new-design" directory\n');
  return options;
}

const cloneStep = 'git clone https://example.invalid/template.git site';

function acquisitionRun(context, { guide, ask = '', origin, controls = [], met = true, evidence, replies }) {
  const options = directoryFixture(context);
  const taskPath = path.join(options.worktree, 'TASK.md');
  writeFileSync(taskPath, readFileSync(taskPath, 'utf8').replace('serving a public web page',
    `obtaining and serving a public web page${ask}`));
  writeFileSync(path.join(options.worktree, 'new-design', 'guide.md'), guide);
  const seen = [];
  return runReviewer({ ...options, config, env: {}, askKind: 'slice',
    fetchImpl: async (_url, request) => {
      const system = JSON.parse(request.body).messages[0].content;
      const kind = system.includes('how the artifact to be served is obtained') ? 'acquisition' :
        system.includes('focused ONLY on the written transition') ? 'transition' : 'ask';
      seen.push(kind);
      const reply = replies?.[seen.length - 1];
      const fields = kind === 'acquisition' ? { acquisition: { origin, controls } } :
        kind === 'transition' ? { operation: { kind: 'ui', controls: ['upload area'] } } : {};
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: reply ?? JSON.stringify({ verdict: kind === 'acquisition' && !met ? 'fail' : 'pass',
          reasons: kind === 'acquisition' && !met ? ['The artifact is never obtained.'] : [], security_notes: [], ...fields,
          checks: [{ id: 1, met: kind === 'acquisition' ? met : true,
            evidence: kind === 'transition' ? transferStep : kind === 'acquisition' ? evidence : 'new-design/guide.md fulfills the Ask.' }] }),
      } }] });
    } }).then((result) => ({ result, seen }));
}

test('an obtain-and-serve Ask fails when the guide only transfers a Markdown file', async (context) => {
  const { result, seen } = await acquisitionRun(context, { guide: `# Upload\n${transferStep}\n`,
    origin: 'missing', met: false, evidence: 'No step obtains the page; only an upload is described.' });
  assert.deepEqual(seen, ['ask', 'transition', 'acquisition', 'ask']);
  assert.equal(result.verdict, 'fail');
  assert.match(result.content, /Acquisition: The artifact is never obtained/);
});

test('a transfer step cannot be reused as the acquisition evidence', async (context) => {
  const { result } = await acquisitionRun(context, { guide: `# Upload\n${transferStep}\n`,
    origin: 'guide_steps', controls: ['upload area'], evidence: transferStep });
  assert.equal(result.verdict, 'fail');
  assert.match(result.content, /No source-grounded acquisition command or named UI action/);
});

test('a guide-invented prepared page does not satisfy an obtain Ask', async (context) => {
  const guide = `# Setup\nHave your finished page ready.\n\n${transferStep}\n`;
  const missing = await acquisitionRun(context, { guide, origin: 'missing', met: false, evidence: 'The guide assumes a ready page.' });
  assert.equal(missing.result.verdict, 'fail');
  const claimed = await acquisitionRun(context, { guide, origin: 'ask_supplied', evidence: 'Have your finished page ready.' });
  assert.equal(claimed.seen.length, 4, 'one bounded correction, then explicit incomplete failure');
  assert.equal(claimed.result.verdict, 'fail');
  assert.equal(claimed.result.completed, false);
});

test('a human-supplied prepared artifact is accepted without guide acquisition steps', async (context) => {
  const { result } = await acquisitionRun(context, { guide: `# Upload\n${transferStep}\n`,
    ask: '. The page is already prepared', origin: 'ask_supplied', evidence: 'The page is already prepared' });
  assert.equal(result.verdict, 'pass', result.content);
  assert.match(result.content, /Verdict: pass/);
});

test('an ask_supplied claim must say the artifact exists', async (context) => {
  const { result } = await acquisitionRun(context, { guide: `# Upload\n${transferStep}\n`,
    origin: 'ask_supplied', evidence: 'serving a public web page' });
  assert.equal(result.verdict, 'fail');
  assert.match(result.content, /does not state that the artifact already exists/);
});

test('an ask_supplied claim cannot rest on a negated or nonexistent artifact', async (context) => {
  for (const [ask, evidence] of [['. The page is not already prepared', 'already prepared'],
    ['. I do not have a page ready', 'have a page ready'], ['. There is no finished page yet', 'finished page']]) {
    const { result } = await acquisitionRun(context, { guide: `# Upload\n${transferStep}\n`, ask, origin: 'ask_supplied', evidence });
    assert.equal(result.verdict, 'fail', ask);
    assert.match(result.content, /does not already exist/);
  }
});

test('a fragment of the transfer step cannot stand in as acquisition', async (context) => {
  const { result } = await acquisitionRun(context, { guide: `# Upload\n${transferStep}\n`,
    origin: 'guide_steps', controls: ['upload area'], evidence: 'drop the folder containing index.html into the upload area.' });
  assert.equal(result.verdict, 'fail');
  assert.match(result.content, /No source-grounded acquisition command or named UI action/);
});

test('a concrete acquisition command followed by the transfer path is accepted', async (context) => {
  const { result, seen } = await acquisitionRun(context, { guide: `# Setup\n\`\`\`sh\n${cloneStep}\n\`\`\`\n\n${transferStep}\n`,
    origin: 'guide_steps', controls: ['git clone'], evidence: cloneStep });
  assert.deepEqual(seen, ['ask', 'transition', 'acquisition', 'ask']);
  assert.equal(result.verdict, 'pass', result.content);
});

test('invented or control-less acquisition evidence is denied', async (context) => {
  const guide = `# Setup\n${cloneStep}\n\n${transferStep}\n`;
  const invented = await acquisitionRun(context, { guide, origin: 'guide_steps', controls: ['git clone'],
    evidence: 'git clone https://example.invalid/other.git' });
  assert.equal(invented.seen.length, 4);
  assert.equal(invented.result.verdict, 'fail');
  assert.equal(invented.result.completed, false);
  const wrongControl = await acquisitionRun(context, { guide, origin: 'guide_steps', controls: ['npm create'], evidence: cloneStep });
  assert.equal(wrongControl.result.verdict, 'fail');
  assert.match(wrongControl.result.content, /No source-grounded acquisition command/);
});

test('directory Ask audit is independent of planner checks and a failed audit overrides checklist pass', async (context) => {
  const options = directoryFixture(context);
  let requests = 0;
  const result = await runReviewer({ ...options, config, env: {}, askKind: 'slice',
    fetchImpl: async (_url, request) => {
      requests += 1;
      const body = JSON.parse(request.body);
      if (requests === 1) {
        assert.match(body.messages[0].content, /independent Original Ask audit/);
        assert.doesNotMatch(body.messages[1].content, /The guide explains local preview/);
        assert.match(body.messages[1].content, /serving a public web page/);
        return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
          content: JSON.stringify({ verdict: 'pass', reasons: [], security_notes: [],
            checks: [{ id: 1, met: true, evidence: 'The guide describes a design and hosting terminology.' }] }),
        } }] });
      }
      if (requests === 2) {
        assert.match(body.messages[0].content, /focused ONLY on the written transition/);
        assert.doesNotMatch(body.messages[1].content, /The guide explains local preview/);
        return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
          content: JSON.stringify({ verdict: 'pass', reasons: [], security_notes: [],
            operation: { kind: 'missing', controls: [] },
            checks: [{ id: 1, met: false, evidence: 'Public hosting steps required by the Ask are absent in new-design/guide.md.' }] }),
        } }] });
      }
      assert.match(body.messages[1].content, /The guide explains local preview/);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: JSON.stringify({ verdict: 'pass', reasons: [], security_notes: [],
          checks: [{ id: 1, met: true, evidence: 'Local preview is documented.' }] }),
      } }] });
    },
  });
  assert.equal(requests, 3);
  assert.equal(result.verdict, 'fail');
  assert.match(result.content, /Original Ask audit.*Public hosting/);
});

for (const scenario of ['pass', 'formatted-transfer', 'corrected', 'missing-procedure-check', 'invented-transfer',
  'invalid-transfer', 'invalid', 'unknown-usage']) {
  test(`directory Ask audit handles ${scenario} with bounded calls and measured usage`, async (context) => {
    const options = directoryFixture(context);
    if (scenario === 'formatted-transfer') {
      const guide = path.join(options.worktree, 'new-design', 'guide.md');
      writeFileSync(guide, readFileSync(guide, 'utf8').replace(transferStep,
        transferStep.replace('Netlify', '**Netlify**').replace('index.html', '`index.html`').replace('and drop', 'and\ndrop')));
    }
    let requests = 0;
    const result = await runReviewer({ ...options, config, env: {}, askKind: 'slice',
      fetchImpl: async (_url, request) => {
        requests += 1;
        const body = JSON.parse(request.body);
        assert.equal(body.tools, undefined);
        const isAudit = body.messages[0].content.includes('independent Original Ask audit');
        const isTransition = body.messages[0].content.includes('focused ONLY on the written transition');
        if (isAudit && !isTransition) {
          assert.match(body.messages[0].content, /Markdown design and instructions are the authorized/);
          assert.match(body.messages[0].content, /not merely ask the reader/);
          assert.match(body.messages[0].content, /trace.*initial artifact.*requested end state/);
          assert.match(body.messages[0].content, /provider names.*not.*procedure/);
          assert.match(body.messages[0].content, /prepared page may be an explicit prerequisite/);
        }

        const invalid = scenario === 'invalid' || scenario === 'corrected' && requests === 1;
        const content = invalid ? '{}' : JSON.stringify({ verdict: 'pass', reasons: [], security_notes: [],
          ...(isTransition ? { operation: { kind: 'ui', controls: ['upload area'] } } : {}),
          checks: (isTransition && scenario === 'missing-procedure-check' && requests === 2 ? [] : [1]).map((id) =>
            ({ id, met: true, evidence: isTransition ? (scenario === 'invalid-transfer' ||
              scenario === 'invented-transfer' && requests === 2 ? 'Run git push to publish the site.' : transferStep) :
              'new-design/guide.md fulfills the requested written procedure.' })) });
        if (scenario === 'corrected' && requests === 2 ||
            ['missing-procedure-check', 'invented-transfer', 'invalid-transfer'].includes(scenario) && requests === 3) {
          assert.match(body.messages.at(-1).content, /Invalid audit JSON/);
          assert.match(body.messages.at(-1).content, /exactly one \{id:1/);
        }
        return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content } }],
          model: 'actual-review-model', ...(scenario === 'unknown-usage' && requests === 1 ? {} :
            { usage: { prompt_tokens: 7, completion_tokens: 2 } }) });
      },
    });
    const incomplete = ['invalid', 'invalid-transfer'].includes(scenario);
    assert.equal(requests, scenario === 'invalid' ? 2 :
      ['corrected', 'missing-procedure-check', 'invented-transfer'].includes(scenario) ? 4 : 3);
    assert.equal(result.verdict, incomplete ? 'fail' : 'pass', result.content);
    assert.equal(result.completed, !incomplete);
    if (!incomplete) assert.match(result.content, /## Original Ask audit\n\nVerdict: pass/);
    if (incomplete) assert.match(result.content, /Reviewer could not complete/);
    else assert.deepEqual(result.usage, scenario === 'unknown-usage' ? {} :
      { prompt_tokens: requests * 7, completion_tokens: requests * 2 });
  });
}

test('a directory design Ask without setup keeps one audit check and does not demand an artifact transition', async (context) => {
  const options = directoryFixture(context);
  const taskPath = path.join(options.worktree, 'TASK.md');
  writeFileSync(taskPath, readFileSync(taskPath, 'utf8').replace(
    'Write setup instructions for serving a public web page to market a business.',
    'Write a Markdown layout and sample copy for a marketing page; do not implement it.'));
  let requests = 0;
  const result = await runReviewer({ ...options, config, env: {}, askKind: 'slice',
    fetchImpl: async (_url, request) => {
      requests += 1;
      const body = JSON.parse(request.body);
      if (requests === 1) assert.doesNotMatch(body.messages[1].content, /2\. The written setup procedure/);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: JSON.stringify({ verdict: 'pass', reasons: [], security_notes: [],
          checks: [{ id: 1, met: true, evidence: 'new-design/guide.md supplies the requested written design.' }] }),
      } }] });
    },
  });
  assert.equal(requests, 2);
  assert.equal(result.verdict, 'pass', result.content);
});

for (const scenario of ['command', 'prose-command', 'ungrounded-command', 'missing-control', 'missing-operation']) {
  test(`transition extraction grounds ${scenario} before accepting a claimed pass`, async (context) => {
    const options = directoryFixture(context);
    const command = 'scp index.html user@example.invalid:/var/www/html/';
    const step = scenario === 'command' || scenario === 'prose-command' ? command : transferStep;
    if (scenario === 'command' || scenario === 'prose-command') {
      writeFileSync(path.join(options.worktree, 'new-design', 'guide.md'),
        scenario === 'command' ? `# Upload\n\`\`\`sh\n${command}\n\`\`\`\n` : `# Upload\n${command}\n`);
    }
    let requests = 0;
    const result = await runReviewer({ ...options, config, env: {}, askKind: 'slice',
      fetchImpl: async (_url, request) => {
        requests += 1;
        const body = JSON.parse(request.body);
        const transition = body.messages[0].content.includes('focused ONLY on the written transition');
        return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
          content: JSON.stringify({ verdict: 'pass', reasons: [], security_notes: [],
            ...(transition ? { operation: { kind: scenario === 'missing-operation' ? 'missing' :
              scenario === 'missing-control' ? 'ui' : 'command',
            controls: scenario === 'missing-control' ? ['Undocumented deploy button'] :
              scenario === 'missing-operation' ? [] : scenario === 'ungrounded-command' ? ['git push'] : ['scp'] } } : {}),
            checks: [{ id: 1, met: true, evidence: transition ? step : 'new-design/guide.md fulfills the Ask.' }] }),
        } }] });
      },
    });
    assert.equal(requests, 3);
    assert.equal(result.completed, true);
    const passes = ['command', 'prose-command'].includes(scenario);
    assert.equal(result.verdict, passes ? 'pass' : 'fail', result.content);
    if (!passes) assert.match(result.content, /No source-grounded operational command or named UI control/);
  });
}

test('directory audit sees unchanged allowed deliverables instead of treating a small diff as the whole output', async (context) => {
  const options = directoryFixture(context);
  execFileSync('git', ['add', 'new-design/guide.md'], { cwd: options.worktree });
  execFileSync('git', ['-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid',
    'commit', '-q', '-m', 'Existing guide'], { cwd: options.worktree });
  writeFileSync(path.join(options.worktree, 'new-design', 'note.md'), '# Additional note\n');
  const taskPath = path.join(options.worktree, 'TASK.md');
  writeFileSync(taskPath, readFileSync(taskPath, 'utf8').replace('- `new-design/guide.md`',
    '- `new-design/guide.md`\n- `new-design/note.md`'));
  options.coderResult.excellence.files = ['new-design/note.md'];
  let requests = 0;
  const result = await runReviewer({ ...options, config, env: {}, askKind: 'slice',
    fetchImpl: async (_url, request) => {
      requests += 1;
      const body = JSON.parse(request.body);
      if (requests === 1) {
        assert.match(body.messages[1].content, /### new-design\/guide.md\n\n# Preview\nUse localhost/);
      }
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: JSON.stringify({ verdict: 'pass', reasons: [], security_notes: [],
          ...(requests === 2 ? { operation: { kind: 'ui', controls: ['upload area'] } } : {}),
          checks: [{ id: 1, met: true, evidence: requests === 2 ? transferStep : 'The unchanged guide is available for judgment.' }] }),
      } }] });
    },
  });
  assert.equal(requests, 3);
  assert.equal(result.verdict, 'pass', result.content);
});
