import { test } from 'node:test';
import assert from 'node:assert/strict';
import { RETENTION_POLICIES, STATE_SCOPES, StateScopeError, normalizeScope } from '../src/lib/paths.mjs';

test('RETENTION_POLICIES has one entry per STATE_SCOPES scope', () => {
  assert.ok(RETENTION_POLICIES, 'RETENTION_POLICIES must be exported');
  const scopes = ['machine', 'repo', 'worktree', 'run'];
  for (const scope of scopes) {
    assert.ok(Object.hasOwn(RETENTION_POLICIES, scope), `RETENTION_POLICIES must include scope "${scope}"`);
    const entry = RETENTION_POLICIES[scope];
    assert.equal(entry.scope, scope);
    assert.equal(typeof entry.root, 'string');
    assert.ok(entry.root.length > 0, `root for ${scope} must be non-empty`);
    assert.ok(Number.isFinite(entry.defaultWindowMs) && entry.defaultWindowMs > 0,
      `defaultWindowMs for ${scope} must be a positive number`);
    assert.equal(typeof entry.optOutSupported, 'boolean');
  }
});

test('RETENTION_POLICIES uses distinct root values for each scope', () => {
  const roots = STATE_SCOPES.map((scope) => RETENTION_POLICIES[scope].root);
  const unique = new Set(roots);
  assert.equal(unique.size, roots.length, 'every scope must have a distinct root');
  // Machine history must be separated from repo state
  assert.notEqual(RETENTION_POLICIES.machine.root, RETENTION_POLICIES.repo.root);
  assert.notEqual(RETENTION_POLICIES.repo.root, RETENTION_POLICIES.worktree.root);
  assert.notEqual(RETENTION_POLICIES.worktree.root, RETENTION_POLICIES.run.root);
});

test('normalizeScope rejects unknown scope names with StateScopeError', () => {
  // Exercises the real library validation path: normalizeScope checks the
  // canonical STATE_SCOPES list and throws StateScopeError for anything else.
  // Since RETENTION_POLICIES is keyed by exactly STATE_SCOPES, any scope that
  // passes normalizeScope is guaranteed to have a RETENTION_POLICIES entry.
  for (const unknown of ['bogus', 'machine2', 'repo ', '', null, 42]) {
    assert.throws(() => normalizeScope(unknown), StateScopeError,
      `normalizeScope(${JSON.stringify(unknown)}) should throw StateScopeError`);
  }
});

test('normalizeScope accepts every STATE_SCOPES value and RETENTION_POLICIES covers them', () => {
  // For each valid scope, normalizeScope succeeds AND RETENTION_POLICIES has an entry.
  for (const scope of STATE_SCOPES) {
    const canonical = normalizeScope(scope);
    assert.equal(canonical, scope);
    assert.ok(Object.hasOwn(RETENTION_POLICIES, canonical),
      `RETENTION_POLICIES must have an entry for normalized scope "${canonical}"`);
  }
  // Legacy alias: 'repository' normalizes to 'repo' and has a RETENTION_POLICIES entry.
  const alias = normalizeScope('repository');
  assert.equal(alias, 'repo');
  assert.ok(Object.hasOwn(RETENTION_POLICIES, alias));
});
