export function githubRepository(origin) {
  let url;
  try {
    url = new URL(origin.startsWith('git@github.com:')
      ? `ssh://git@github.com/${origin.slice('git@github.com:'.length)}`
      : origin);
  } catch {
    throw new Error('origin must be a GitHub HTTPS or SSH repository URL');
  }
  const match = /^\/([A-Za-z0-9-]+)\/([A-Za-z0-9_.-]+?)(?:\.git)?$/.exec(url.pathname);
  if (url.hostname !== 'github.com' || !['https:', 'ssh:'].includes(url.protocol) ||
      url.port || url.search || url.hash || url.password ||
      (url.protocol === 'https:' ? url.username : url.username !== 'git') ||
      !match || ['.', '..'].includes(match[2])) {
    throw new Error('origin must be a GitHub HTTPS or SSH repository URL');
  }
  return `${match[1]}/${match[2]}`;
}
