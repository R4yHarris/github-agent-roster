export const commandGroups = Object.freeze(['Session', 'Ask', 'Model', 'Board', 'Human', 'Settings']);
const page = (name, group, usage, description, example, flags = [], aliases = []) =>
  Object.freeze({ name, group, usage, description, example, flags: Object.freeze(flags), aliases: Object.freeze(aliases) });

export const commands = Object.freeze([
  page('help', 'Session', '/help [GROUP|COMMAND]', 'Show command groups or a complete command page.', '/help run'),
  page('learn', 'Human', '/learn --recurring', 'Draft improvement proposals from three recurring slice failures; never installs skills, principals or routing changes.', '/learn --recurring'),
  page('status', 'Session', '/status [N] [--offline]', 'Show cached session fields without a model or GitHub request.', '/status',
    ['--offline: use local cached evidence only']),
  page('history', 'Session', '/history', 'Show the last 20 stored safe commands.', '/history'),
  page('resume', 'Session', '/resume [N]', 'List local issue runs with no argument; resume an isolated worktree and reuse a valid TASK/recipe.', '/resume 108'),
  page('worktrees', 'Session', '/worktrees', 'List registered issue worktree path, isolated branch, last known seat and dirty/clean Git state.', '/worktrees'),
  page('batch', 'Session', '/batch', 'Refused: One seat at a time. Worktrees are isolated. No parallel execution is started.', '/batch'),
  page('recap', 'Session', '/recap', 'Print one human metadata line: outcome, files, last test, review and finish reason. No completion body.', '/recap'),
  page('btw', 'Session', '/btw QUESTION', 'Ask one read-only model question about current task metadata; no tools, tests, writes, memory or publication changes.',
    '/btw Which acceptance check covers this edge case?'),
  page('context', 'Session', '/context', 'Show last measured seat provider, model, effort, input, output, context max, finish reason, character pack budget and prior-feedback inclusion. Missing counts are -.',
    '/context'),
  page('usage', 'Session', '/usage', 'Show one panel for the current session: model, endpoint, prompt and completion tokens, context max, effort, finish reason, tool calls, elapsed, thinking and the outbound completion cap. Missing values are -.',
    '/usage'),
  page('map', 'Session', '/map', 'Write a filename-only repo map: top two directory levels plus TASK paths, capped at 80 lines. Coder reads require non-docs difficulty4+.', '/map'),
  page('checkpoints', 'Session', '/checkpoints', 'List current-task checkpoint number, seat, short status and time.', '/checkpoints'),
  page('rewind', 'Session', '/rewind N', 'Restore checkpoint-covered product files; keep TASK/PLAN/logs. Refuse published PRs. /undo means latest only.',
    '/rewind 1', [], ['undo']),
  page('redraw', 'Session', '/redraw', 'Repaint the tray without clearing scrollback.', '/redraw'),
  page('clear', 'Session', '/clear', 'Clear the screen and repaint the tray.', '/clear'),
  page('quit', 'Session', '/quit', 'Exit with code 0; bare exit is also accepted.', '/q', [], ['q']),
  page('ask', 'Ask', '/ask TEXT', 'Run a local Ask through planner, coder and reviewer without creating a GitHub issue.',
    '/ask Add a Status section to README.md.'),
  page('plan', 'Ask', '/plan TEXT', 'Explore read-only and write PLAN.md only; Enter accepts a slice and /stop keeps it without coding.',
    '/plan Add a Status section to README.md.', ['--plan: use /run N --plan for the same gate on an issue']),
  page('run', 'Ask', '/run N [--parallel K] [--attempts K] [--confirm|--plan] [--auto-model]', 'Run an existing issue in builtin seats.', '/run 108 --confirm',
    ['--confirm: pause after the task summary; Enter continues and /stop cancels',
      '--plan: explore and write PLAN.md only before human acceptance',
      '--auto-model: route from fleet priors and human evaluations',
      '--parallel K: isolated ready child waves, bounded by declared capacity',
      '--attempts K: isolated slice candidates on distinct fleet profiles; publish only the best passing attempt']),
  page('retry', 'Ask', '/retry', 'Repeat the last Ask or issue run in the same worktree without worktree add.', '/retry'),
  page('stop', 'Ask', '/stop', 'Cancel the active seat or confirmed task, like one Ctrl+C.', '/stop'),
  page('steer', 'Ask', '/steer TEXT', 'Interrupt the drafting coder model call and send queued text plus this instruction next. It cannot widen Allowed Files or edit TASK.md; Ctrl+C cancels without steering. A bare line during a drafting coder seat is the same action.',
    '/steer Keep the public API unchanged.'),
  page('queue', 'Ask', '/queue [TEXT|list|drop N|clear]', 'Queue an Ask while a seat is running. list shows the queue, drop N removes one, clear removes all. A bare line while a seat is running queues an Ask unless the coder is waiting for steering. /q remains quit.',
    '/queue Add a Status section to README.md.', [], ['qs']),
  page('remediate', 'Ask', '/remediate [TEST ...]', 'Start a separate remediation agent for failing tests in this workspace. It edits only those tests and uses the largest-context fleet profile.',
    '/remediate tests/repl.test.mjs'),
  page('publish', 'Ask', '/publish [SUBJECT] [--model MODEL] [--skip-review]',
    'Publish reviewed changes only through the GitHub App SDK.', '/publish feat: add status --model GPT-6.1-Sol',
    ['--model MODEL: actual publishing model; measured seat metadata wins',
      '--skip-review: if review failed, explicitly bypass that verdict only, not tests or policy']),
  page('review', 'Ask', '/review [--again]', 'Run only the read-only reviewer on current verified task/diff evidence. It writes REVIEW.md, never product code. Failed review blocks publication.',
    '/review --again', ['--again: archive an existing managed report and rerun the current diff']),
  page('model', 'Model', '/model [ID|clear] [--save]', 'Show model and host; select for this session unless --save is explicit.',
    '/model deepseek-v4.1-flash --save', ['--save: write the private model setting']),
  page('effort', 'Model', '/effort [l|m|h|x|none|status]', 'Select session effort without writing config; docs cap at high and length retries drop reasoning.', '/effort l'),
  page('provider', 'Model', '/provider', 'Show session profile name and host, never a key.', '/provider'),
  page('fleet', 'Model', '/fleet [list|use ID|probe [ID] [--set-model [MODEL]]|add FLAGS]',
    'List or use session profiles, read-only probe models, or add with the existing fleet flags.',
    '/fleet use spark-4', ['--set-model [MODEL]: explicitly save a listed model to private config',
      '--id, --base-url, --model, --context, --concurrency, --hardware, --task-class: existing fleet add flags']),
  page('stats', 'Model', '/stats [REF]', 'Read model run metrics without selecting a model.', '/stats HEAD'),
  page('issues', 'Board', '/issues', 'List up to 100 open issue numbers and titles only; never bodies.', '/issues'),
  page('waves', 'Board', '/waves [open]', 'Read PLAN.md child drafts and GitHub states. Only explicit open creates those drafts; earlier open waves block later starts. No second board is stored.',
    '/waves', ['open: explicitly create only missing drafts already validated in PLAN.md']),
  page('issue', 'Board', '/issue N', 'Show cached title/state/branch/PR metadata; query GitHub only on a cache miss.', '/issue 108'),
  page('diff', 'Board', '/diff', 'Print tracked git diff filenames in the current issue worktree, never file bodies.', '/diff'),
  page('log', 'Board', '/log N|debug', 'Tail local seat logs; debug tail is refused while debug is off.', '/log 108'),
  page('eval', 'Human', '/eval TARGET accept|reject|rework --minutes N --difficulty 1-5 "TEXT"',
    'Record a human evaluation; agents cannot call it. Legacy positional difficulty and again remain accepted.',
    '/eval roster-108-coder accept --minutes 15 --difficulty 2 "Meets the checks."',
    ['--minutes N: actual human elapsed minutes', '--difficulty 1-5: human task difficulty']),
  page('recommend', 'Human', '/recommend [feat|fix|docs|test] [--difficulty 1-5]',
    'Read a route for the last task or default feat/difficulty2 without changing the model.', '/recommend',
    ['--difficulty 1-5: required task capacity']),
  page('statusbar', 'Settings', '/statusbar on|off', 'Toggle both tray bars for this process.', '/statusbar off'),
  page('debug', 'Settings', '/debug on|off|status', 'Toggle or show metadata-only debug logging for this process.', '/debug status'),
  page('doctor', 'Settings', '/doctor [warm]', 'Run existing offline prerequisite checks or a read-only host/status warm probe.', '/doctor warm'),
  page('config', 'Settings', '/config [path|set KEY VALUE]',
    'Show redacted private config or its path. Set only effort/context budget; statusbar/debug are process-only. Endpoints, PEM paths and policy are refused.',
    '/config set context.budget 12000'),
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
