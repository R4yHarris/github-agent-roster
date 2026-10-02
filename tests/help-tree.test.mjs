import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { createDispatcher } from '../src/repl.mjs';
import { commands, commandGroups, formatHelp } from '../src/shell/commands.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));

test('every registry command has usage, aliases, flags and exactly one example', () => {
  for (const command of commands) {
    assert.ok(commandGroups.includes(command.group));
    const help = formatHelp(command.name);
    assert.match(help, /^Usage: \//);
    assert.match(help, /Aliases:/);
    assert.match(help, /Flags:/);
    assert.equal(help.match(/^Example:/gm)?.length, 1);
    for (const alias of command.aliases) assert.equal(formatHelp(alias), help);
  }
  assert.match(formatHelp('run'), /--confirm/);
  assert.match(formatHelp('model'), /--save/);
  assert.match(formatHelp('publish'), /if review failed/);
});

test('group help lists only its group and root help contains all six ordered groups', () => {
  const root = formatHelp();
  let previous = -1;
  for (const group of commandGroups) {
    const index = root.indexOf(`${group}\n`);
    assert.ok(index > previous);
    previous = index;
    const help = formatHelp(group);
    for (const command of commands) {
      assert.equal(help.includes(`  ${command.usage}`), command.group === group);
    }
  }
  assert.equal(formatHelp('session'), formatHelp('Session'));
  assert.equal(formatHelp('unknown'), null);
});

test('slash alone is root help and unknown commands never enter a model or seat', async () => {
  let output = '';
  let errors = '';
  const shell = createDispatcher({ config, env: {}, services: {
    repositoryBranch: () => 'main',
    runBuiltinIssue: () => assert.fail('Help must not run issue seats'),
    runBuiltinAsk: () => assert.fail('Unknown slash commands must not run local seats'),
    submitAsk: () => assert.fail('Help must not submit asks'),
  }, output: { write(value) { output += value; } }, errorOutput: { write(value) { errors += value; } } });
  await shell.dispatch('/');
  const root = output;
  output = '';
  await shell.dispatch('/help');
  assert.equal(output, root);
  await shell.dispatch('/help run');
  assert.match(output, /--confirm/);
  await shell.dispatch('/help model');
  assert.match(output, /--save/);
  for (const command of ['/unknown', '/7', '/help unknown']) await shell.dispatch(command);
  assert.equal(errors, 'Unknown command. /help lists commands.\n'.repeat(3));
});
