// #294: seats act on coding rules only. Publication, App credentials, and GHCP metadata are the
// harness's job, so including them invites a seat to commit, push, or open a PR on its own.
const omittedHeading = /publish|publication|metadata|credential|contracts dependency|ghcp/i;
const publicationRule = new RegExp([
  'agent-pr', 'GITHUB_APP', 'gh pr create', 'git push', 'git commit', '\\bAI_[A-Z_]+',
  'merge-when-green', 'draft PR', 'App credentials', 'private[- ]key', 'GHCP', 'publish',
].join('|'), 'i');

export const omittedRulesNote = '(Publication, credential, and session-metadata rules are omitted: ' +
  'the harness publishes after review; this seat never commits, pushes, or opens a PR.)';

// Splits a markdown section body into paragraphs and top-level list items, keeping nested lines with their item.
function blocks(body) {
  const result = [];
  let current = [];
  const flush = () => {
    if (current.length) result.push(current.join('\n'));
    current = [];
  };
  for (const line of body.split('\n')) {
    if (!line.trim()) {
      flush();
      continue;
    }
    if (/^(?:[-*]|\d+\.) /.test(line)) flush();
    current.push(line);
  }
  flush();
  return result;
}

export function seatRules(agents) {
  if (typeof agents !== 'string') throw new TypeError('AGENTS.md must be text');
  const text = agents.replace(/\r\n/g, '\n');
  const sections = text.split(/^(?=## )/m);
  const kept = [];
  let omitted = false;
  for (const section of sections) {
    const [first, ...rest] = section.split('\n');
    const heading = /^## /.test(first) ? first : null;
    if (heading && omittedHeading.test(heading)) {
      omitted = true;
      continue;
    }
    const body = heading ? rest.join('\n') : section;
    const parts = blocks(body).filter((block) => {
      if (!publicationRule.test(block)) return true;
      omitted = true;
      return false;
    });
    if (heading && !parts.length) continue;
    const isItem = (block) => /^(?:[-*]|\d+\.) /.test(block);
    const joined = parts.reduce((text, part, index) =>
      !index ? part : `${text}${isItem(part) && isItem(parts[index - 1]) ? '\n' : '\n\n'}${part}`, '');
    kept.push([heading, joined].filter(Boolean).join('\n\n'));
  }
  const rules = kept.join('\n\n').trim();
  return omitted ? `${rules}\n\n${omittedRulesNote}`.trim() : rules;
}
