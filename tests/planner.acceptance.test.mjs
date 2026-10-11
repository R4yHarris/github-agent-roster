import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { validatePlan } from '../src/planner/plan.mjs';
import { validatePlanningReceipt } from '../src/planner/receipt.mjs';
import { buildPlan, planAsk, planFromTask } from '../src/planner/stub.mjs';
import { parseTaskDocument } from '../src/planner/task.mjs';

const ask = 'Write a Markdown guide. Output it in a new "guide" directory';
const check = (length) => `The guide contains ${'x'.repeat(length - 23)} end`;
const options = { reference: 'local:acceptance', filesAllowed: ['guide/README.md'] };
const outline = (entry) => ({
  outcomes: ['Document safe setup'],
  issues: [1, 2].map((wave) => ({ title: `Guide ${wave}`, outcome: 'Explain setup', wave,
    files_allowed: ['guide/README.md'], acceptance_checks: [entry] })),
});

test('all planner ingress paths reject oversized checks and preserve the exact 240-character boundary', () => {
  const full = check(240);
  assert.equal(full.length, 240);
  const task = buildPlan(ask, { ...options, acceptanceChecks: [full] }).task;
  assert.equal(parseTaskDocument(task, { expectedAsk: ask }).acceptance_checks[0], full);
  assert.equal(parseTaskDocument(buildPlan(ask, { ...options,
    acceptanceChecks: [full.replace(' contains ', '  contains \n')] }).task).acceptance_checks[0], full);
  assert.equal(validatePlan(outline(full), { kind: 'feature', filesAllowed: options.filesAllowed })
    .issues[0].acceptance_checks[0], full);
  const receipt = (entry) => `title: ${JSON.stringify(parseTaskDocument(task).title)}\n` +
    `acceptance_checks:\n  - ${JSON.stringify(entry)}\nfiles_allowed:\n  - guide/README.md\n`;
  validatePlanningReceipt(receipt(full), parseTaskDocument(task));
  const long = check(241);
  for (const operation of [
    () => buildPlan(ask, { ...options, acceptanceChecks: [long] }),
    () => planFromTask(task.replace(full, long), ask),
    () => validatePlan(outline(long), { kind: 'feature', filesAllowed: options.filesAllowed }),
    () => validatePlanningReceipt(receipt(long), parseTaskDocument(task)),
  ]) assert.throws(operation, /Acceptance check.*240.*split/i);
  assert.equal(parseTaskDocument(task.replace(full, long), { expectedAsk: ask }).acceptance_checks[0], long);
});

test('every planner ingress identifies the normalized oversized check without echoing its contents', () => {
  const safe = 'The guide contains setup instructions';
  const secret = 'test-only-private-api-key';
  const long = `${check(241 - secret.length - 1)} ${secret}`;
  assert.equal(long.length, 241);
  const raw = long.replace(' contains ', '  contains \n   ');
  const task = buildPlan(ask, { ...options, acceptanceChecks: [safe, check(240)] }).task;
  const receipt = `title: ${JSON.stringify(parseTaskDocument(task).title)}\n` +
    `acceptance_checks:\n  - ${JSON.stringify(safe)}\n  - ${JSON.stringify(raw)}\nfiles_allowed:\n  - guide/README.md\n`;
  const plan = outline(check(240));
  plan.issues[0].acceptance_checks = [safe, raw];
  for (const operation of [
    () => buildPlan(ask, { ...options, acceptanceChecks: [safe, raw] }),
    () => planFromTask(task.replace(check(240), raw), ask),
    () => validatePlan(plan, { kind: 'feature', filesAllowed: options.filesAllowed }),
    () => validatePlanningReceipt(receipt, parseTaskDocument(task)),
  ]) {
    assert.throws(operation, (error) => {
      assert.match(error.message, /^Acceptance check must be at most 240 characters/);
      assert.match(error.message, /check 2, got 241, exceeds limit by 1/);
      assert.match(error.message, /split.*without dropping requirements/);
      assert.ok(!error.message.includes(secret));
      return true;
    });
  }
});

test('measured feedback lets the planner split an oversized check without losing the original requirements', async () => {
  const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
    .replace('base_url: ""', 'base_url: "http://localhost:1234/v1"').replace('model: ""', 'model: test-model'));
  const safe = 'The guide contains setup instructions';
  const pieces = [
    'The guide contains setup instructions covering prerequisites, local preview commands and verification steps that the reader can follow.',
    'The guide contains a safety warning explaining that commands are illustrative examples and that readers must supply their own placeholder values.',
  ];
  const long = pieces.join('; ');
  assert.ok(long.length > 240);
  assert.ok(pieces.every((piece) => piece.length <= 240));
  let calls = 0;
  const result = await planAsk(ask, { config, env: {},
    tools: { write_file: async () => ({ ok: true }) },
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      calls += 1;
      if (calls === 1) {
        const valid = buildPlan(ask, { ...options, acceptanceChecks: [safe, check(240)] }).task;
        return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [{ id: 'long-check', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content: valid.replace(check(240), long) }),
          } }],
        } }] });
      }
      assert.ok(body.messages.at(-1).content.includes(
        `check 2, got ${long.length}, exceeds limit by ${long.length - 240}`));
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: JSON.stringify({
          title: 'Guide', acceptance_checks: [safe, ...pieces], files_allowed: options.filesAllowed,
        }),
      } }] });
    } });
  assert.equal(calls, 2);
  const parsed = parseTaskDocument(result.task, { expectedAsk: ask });
  assert.deepEqual(parsed.acceptance_checks, [safe, ...pieces]);
  assert.deepEqual(parsed.files_allowed, options.filesAllowed);
  assert.equal(parsed.ask, ask);
  assert.equal(result.error, undefined);
});

test('an oversized written TASK stays invalid until a complete concise JSON correction', async () => {
  const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
    .replace('base_url: ""', 'base_url: "http://localhost:1234/v1"').replace('model: ""', 'model: test-model'));
  const concise = 'The guide contains setup steps and the required safety warning';
  const valid = buildPlan(ask, { ...options, acceptanceChecks: [concise] }).task;
  let calls = 0;
  const result = await planAsk(ask, { config, env: {},
    tools: { write_file: async () => ({ ok: true }) },
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      calls += 1;
      assert.equal(body.messages[1].content, ask);
      if (calls === 1) return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'oversized', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content: valid.replace(concise, check(241)) }),
        } }],
      } }] });
      assert.match(JSON.stringify(body.messages), /240.*split/i);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: JSON.stringify({ title: 'Guide', acceptance_checks: [concise], files_allowed: options.filesAllowed }),
      } }] });
    } });
  assert.equal(calls, 2);
  assert.deepEqual(parseTaskDocument(result.task, { expectedAsk: ask }).acceptance_checks, [concise]);
});

test('a repeatedly invalid written TASK stops after one correction even with a large general turn budget', async () => {
  const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
    .replace('base_url: ""', 'base_url: "http://localhost:1234/v1"').replace('model: ""', 'model: test-model'));
  const valid = buildPlan(ask, { ...options, acceptanceChecks: ['The guide contains setup instructions'] }).task;
  const invalid = valid.replace('The guide contains setup instructions', check(241));
  let calls = 0;
  const result = await planAsk(ask, { config: { ...config, planner: { ...config.planner, turn_budget: 20 } }, env: {},
    tools: { write_file: async () => ({ ok: true }) },
    fetchImpl: async () => {
      calls += 1;
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: `invalid-${calls}`, type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content: invalid }),
        } }],
      } }] });
    } });
  assert.equal(calls, 2);
  assert.equal(result.mode, 'stub');
  assert.match(result.error, /240.*split/);
  assert.equal(parseTaskDocument(result.task, { expectedAsk: ask }).ask, ask);
});

test('slice JSON correction retains the original Ask and does not accept a truncated requirement', async () => {
  const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
    .replace('base_url: ""', 'base_url: "http://localhost:1234/v1"').replace('model: ""', 'model: test-model'));
  let calls = 0;
  const concise = 'The guide contains setup steps and ends with the required safety warning';
  const result = await planAsk(ask, { config, env: {},
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.equal(body.messages[1].content, ask);
      if (calls === 2) assert.match(body.messages.at(-1).content, /240.*split/i);
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant',
        content: JSON.stringify({ title: 'Guide', acceptance_checks: [calls === 1 ? check(241) : concise],
          files_allowed: options.filesAllowed }),
      } }] });
    } });
  assert.equal(calls, 2);
  const parsed = parseTaskDocument(result.task, { expectedAsk: ask });
  assert.equal(parsed.ask, ask);
  assert.deepEqual(parsed.acceptance_checks, [concise]);
});

test('critic JSON revision stops after one invalid correction even with a large planner turn budget', async () => {
  const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
    .replace('base_url: ""', 'base_url: "http://localhost:1234/v1"').replace('model: ""', 'model: test-model'));
  const task = buildPlan(ask, { ...options, acceptanceChecks: ['The guide includes the requested setup steps'] }).task;
  let calls = 0;
  await assert.rejects(planAsk(ask, {
    config: { ...config, seat: { ...config.seat, max_turns: 60 } },
    env: {}, criticFeedback: { task, defects: [{ problem: 'Missing requirement check' }] },
    fetchImpl: async () => {
      calls += 1;
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: JSON.stringify({
          title: 'Guide', acceptance_checks: [check(241)], files_allowed: options.filesAllowed,
        }),
      } }] });
    },
  }), /Plan critic revision failed after one correction:.*240.*split/);
  assert.equal(calls, 2);
  assert.equal(parseTaskDocument(task, { expectedAsk: ask }).ask, ask);
});
