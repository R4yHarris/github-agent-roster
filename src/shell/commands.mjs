export const commands = Object.freeze([
  'ask', 'run', 'status', 'statusbar', 'model', 'effort', 'log', 'debug', 'eval', 'publish',
  'stats', 'recommend', 'vault', 'help', 'redraw', 'clear', 'quit',
].map((name) => Object.freeze({ name, aliases: Object.freeze(name === 'quit' ? ['q'] : []) })));

export function canonicalCommand(name) {
  return commands.find((command) => command.name === name || command.aliases.includes(name))?.name ?? name;
}

export function completeCommand(line) {
  if (!/^\/[a-z]*$/.test(line)) return [[], line];
  const matches = commands.flatMap(({ name, aliases }) => [name, ...aliases])
    .map((name) => `/${name}`).filter((name) => name.startsWith(line)).sort();
  return [matches, line];
}
