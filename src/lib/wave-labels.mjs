export function issueWave(issue) {
  if (issue.labels !== undefined && !Array.isArray(issue.labels)) throw new Error('Issue wave labels must be an array');
  const named = (issue.labels ?? []).map((label) => typeof label === 'string' ? label : label?.name)
    .filter((name) => /^wave:[1-8]$/.test(name ?? '')).map((name) => Number(name.slice(5)));
  const body = /<!-- Roster-Wave: ([1-8]) -->/.exec(issue.body ?? '')?.[1];
  if (body) named.push(Number(body));
  const values = [...new Set(named)];
  if (values.length > 1) throw new Error('Issue has conflicting wave metadata');
  return values[0] ?? null;
}
