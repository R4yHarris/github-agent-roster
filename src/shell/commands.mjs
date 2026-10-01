export const commandGroups = Object.freeze(['Session', 'Ask', 'Model', 'Board', 'Human', 'Settings']);
const page = (name, group, usage, description, example, flags = [], aliases = []) =>
  Object.freeze({ name, group, usage, description, example, flags: Object.freeze(flags), aliases: Object.freeze(aliases) });

export const commands = Object.freeze([
  page('help', 'Session', '/help [GROUP|COMMAND]', 'Show command groups or a complete command page.', '/help run'),
  page('status', 'Session', '/status [N] [--offline]', 'Show cached session fields without a model or GitHub request.', '/status',
    ['--offline: use local cached evidence only']),
  page('history', 'Session', '/history', 'Show the last 20 stored safe commands.', '/history'),
  page('redraw', 'Session', '/redraw', 'Repaint the tray without clearing scrollback.', '/redraw'),
  page('clear', 'Session', '/clear', 'Clear the screen and repaint the tray.', '/clear'),
  page('quit', 'Session', '/quit', 'Exit with code 0; bare exit is also accepted.', '/q', [], ['q']),
  page('ask', 'Ask', '/ask TEXT', 'Run a local Ask through planner, coder and reviewer without creating a GitHub issue.',
    '/ask Add a Status section to README.md.'),
  page('run', 'Ask', '/run N [--confirm] [--auto-model]', 'Run an existing issue in builtin seats.', '/run 108 --confirm',
    ['--confirm: pause after the task summary; Enter continues and /stop cancels',
      '--auto-model: route from fleet priors and human evaluations']),
  page('retry', 'Ask', '/retry', 'Repeat the last Ask or issue run in the same worktree without worktree add.', '/retry'),
  page('stop', 'Ask', '/stop', 'Cancel the active seat or confirmed task, like one Ctrl+C.', '/stop'),
  page('publish', 'Ask', '/publish [SUBJECT] [--model MODEL] [--skip-review]',
    'Publish reviewed changes only through the GitHub App SDK.', '/publish feat: add status --model GPT-6.1-Sol',
    ['--model MODEL: actual publishing model; measured seat metadata wins',
      '--skip-review: if review failed, explicitly bypass that verdict only, not tests or policy']),
  page('model', 'Model', '/model [ID|clear] [--save]', 'Show or select the model; --save persists private configuration.',
    '/model deepseek-v4.1-flash --save', ['--save: write the private model setting']),
  page('effort', 'Model', '/effort [l|m|h|x|none]', 'Show or select explicit reasoning effort; docs slices cap at high.', '/effort l'),
  page('stats', 'Model', '/stats [REF]', 'Read model run metrics without selecting a model.', '/stats HEAD'),
  page('log', 'Board', '/log N|debug', 'Tail local seat logs or this process debug metadata.', '/log 108'),
  page('eval', 'Human', '/eval TARGET VERDICT 1-5 y|n [--minutes N] [--comment "TEXT"]',
    'Record a human evaluation; agent seats cannot invoke this writer.',
    '/eval roster-108-coder accept 2 n --minutes 15 --comment "Meets the checks."',
    ['--minutes N: actual human elapsed minutes', '--comment TEXT: quoted human feedback']),
  page('recommend', 'Human', '/recommend feat|fix|docs|test [--difficulty 1-5]',
    'Read a fleet recommendation without changing the default.', '/recommend fix --difficulty 2',
    ['--difficulty 1-5: required task capacity']),
  page('statusbar', 'Settings', '/statusbar on|off', 'Toggle both tray bars for this process.', '/statusbar off'),
  page('debug', 'Settings', '/debug on|off|status', 'Toggle or show metadata-only debug logging for this process.', '/debug status'),
  page('vault', 'Settings', '/vault [list]|get NAME|set NAME', 'Manage named secrets without showing values.',
    '/vault set ROSTER_API_KEY'),
]);

export function canonicalCommand(name) {
  return commands.find((command) => command.name === name || command.aliases.includes(name))?.name ?? name;
}

export function completeCommand(line) {
  if (!/^\/[a-z]*$/.test(line)) return [[], line];
  const matches = commands.flatMap(({ name, aliases }) => [name, ...aliases])
    .map((name) => `/${name}`).filter((name) => name.startsWith(line)).sort();
  return [matches, line];
}

export function formatHelp(target = '') {
  const selected = target.trim().replace(/^\//, '');
  const command = commands.find((entry) => entry.name === selected.toLowerCase() ||
    entry.aliases.includes(selected.toLowerCase()));
  const group = commandGroups.find((name) => name.toLowerCase() === selected.toLowerCase());
  if (command && selected !== group) {
    return `Usage: ${command.usage}\n${command.description}\n` +
      `Aliases: ${command.aliases.length ? command.aliases.map((name) => `/${name}`).join(', ') : '(none)'}` +
      (command.name === 'quit' ? '; bare exit' : '') + '\n' +
      `Flags:\n${command.flags.length ? command.flags.map((flag) => `  ${flag}`).join('\n') : '  (none)'}\n` +
      `Example: ${command.example}\n`;
  }
  if (selected && !group) return null;
  const groups = group ? [group] : commandGroups;
  return 'Commands:\n' + groups.map((name) => `${name}\n` +
    commands.filter((entry) => entry.group === name).map((entry) => `  ${entry.usage}`).join('\n')
  ).join('\n\n') + '\n';
}
