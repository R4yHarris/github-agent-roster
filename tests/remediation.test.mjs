import assert from 'node:assert/strict';
import test from 'node:test';
import { remediationTask, selectRemediationProfile } from '../src/runtime/remediation.mjs';

test('remediation stays on the failing workspace tests and picks the largest context', () => {
  const task = remediationTask(['tests/repl.test.mjs', 'tests/human-settings-shell.test.mjs'], 'D:/oss/github-agent-roster');
  assert.match(task.ask, /tests\/repl.test.mjs/);
  assert.equal(task.files.length, 2);
  const profile = selectRemediationProfile([
    { id: 'local', base_url: 'http://127.0.0.1:8000/v1', context_max: 32768 },
    { id: 'aperture-qwen', base_url: 'https://example.test/v1', context_max: 262144 },
  ]);
  assert.equal(profile.id, 'aperture-qwen');
});
