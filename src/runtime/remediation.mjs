export function selectRemediationProfile(profiles, currentId) {
  const list = Array.isArray(profiles) ? profiles.filter((profile) => profile?.id && profile.base_url) : [];
  if (!list.length) return null;
  return [...list].sort((left, right) => (right.context_max ?? 0) - (left.context_max ?? 0) ||
    left.id.localeCompare(right.id))[0] ?? list.find((profile) => profile.id === currentId) ?? null;
}

export function remediationTask(files, workspace) {
  const tests = [...new Set((files ?? []).map((file) => String(file).replaceAll('\\', '/')))]
    .filter((file) => /^(?:tests\/)?[^/]+[._-]test\.[cm]?js$/.test(file) || file.startsWith('tests/'));
  if (!tests.length) throw new TypeError('Remediation requires at least one workspace test file');
  return {
    files: tests,
    ask: `Remediate the failing tests in ${workspace}. Edit only ${tests.join(', ')}. ` +
      'Do not change product code unless a test proves the current behavior is wrong. ' +
      'Run only those test files. Stop when they pass.',
  };
}
