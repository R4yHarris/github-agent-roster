// Gate B only (spec 5.3–5.6/5.8): synthetic protocol checks, not a CLI adapter,
// authentication sandbox, process-exit oracle, billing bound, or publisher.
import { createHash } from 'node:crypto';
import { lstatSync, mkdirSync, readFileSync, unlinkSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { createTools, isForbiddenWrite } from '../../src/runtime/tools.mjs';

function freeze(value) {
  if (value && typeof value === 'object') {
    for (const child of Object.values(value)) freeze(child);
    Object.freeze(value);
  }
  return value;
}

function snapshot(value, state = { ancestors: new Set(), nodes: 0, bytes: 0 }, depth = 0) {
  if (++state.nodes > 4096 || depth > 16) throw new Error('Contract JSON exceeds structural bounds');
  state.bytes += typeof value === 'string' ? Buffer.byteLength(value, 'utf8') : 8;
  if (state.bytes > 2_000_000) throw new Error('Contract JSON exceeds byte bounds');
  if (value === null || typeof value === 'string' || typeof value === 'boolean') return value;
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  if (!value || typeof value !== 'object' ||
      ![Object.prototype, Array.prototype].includes(Object.getPrototypeOf(value))) {
    throw new Error('Expected plain JSON data');
  }
  const result = Array.isArray(value) ? [] : {};
  if (state.ancestors.has(value)) throw new Error('Cyclic contract JSON');
  state.ancestors.add(value);
  if (Array.isArray(value) &&
      (Object.keys(value).length !== value.length ||
       Array.from({ length: value.length }, (_, index) => index).some((index) => !Object.hasOwn(value, index)))) {
    throw new Error('Sparse or decorated array');
  }
  for (const key of Object.keys(value).sort()) {
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    if (!descriptor || !('value' in descriptor) || key === '__proto__') throw new Error('Invalid JSON property');
    state.bytes += Buffer.byteLength(key, 'utf8');
    result[key] = snapshot(descriptor.value, state, depth + 1);
  }
  state.ancestors.delete(value);
  return freeze(result);
}

export function offlineDigest(value) {
  return createHash('sha256').update(JSON.stringify(snapshot(value))).digest('hex');
}

function text(value) {
  if (typeof value !== 'string' || !value.trim()) throw new Error('Expected nonempty text');
  return value;
}

function shape(value, required, optional = []) {
  if (!value || Array.isArray(value) || typeof value !== 'object' ||
      required.some((key) => !Object.hasOwn(value, key)) ||
      Object.keys(value).some((key) => ![...required, ...optional].includes(key))) {
    throw new Error('Malformed contract shape');
  }
}

function relativeFile(value) {
  text(value);
  const parts = value.split('/');
  if (value.includes('\\') || /[:\0*?]/.test(value) || parts.some((part) =>
    !part || part === '.' || part === '..' || /[. ]$/.test(part) ||
    /^(con|prn|aux|nul|com[0-9]|lpt[0-9])(\.|$)/i.test(part)) ||
    parts.some((part) => /^(task\.md|recipe\.yml|estimate\.md|result\.md|review\.md|agents\.md)$/i.test(part)) ||
    isForbiddenWrite(value)) throw new Error('Protected or ambiguous path');
  return value;
}

function scopeList(value) {
  if (!Array.isArray(value) || !value.length || value.length > 16) throw new Error('Invalid exact scope');
  const files = value.map(relativeFile);
  if (new Set(files.map((file) => file.toLowerCase())).size !== files.length) throw new Error('Duplicate scope');
  return files;
}

// Synchronous preflight/import avoids yielding to fixture callbacks between
// validation and mutation. It does not defeat a concurrent OS filesystem attacker.
function checkedFile(root, relative) {
  let current = root;
  const rootEntry = lstatSync(root);
  if (!rootEntry.isDirectory() || rootEntry.isSymbolicLink()) throw new Error('Invalid fixture root');
  const parts = relativeFile(relative).split('/');
  for (let index = 0; index < parts.length; index++) {
    current = path.join(current, parts[index]);
    const entry = lstatSync(current, { throwIfNoEntry: false });
    if (!entry) continue;
    if (entry.isSymbolicLink() || (index < parts.length - 1 ? !entry.isDirectory() :
      !entry.isFile() || entry.nlink !== 1)) throw new Error('Special or linked file');
  }
  return current;
}

const receipts = new WeakSet();

// Independent host-review fixture, deliberately separate from worker claims.
export function offlineHostReview(subject, checks) {
  if (!Array.isArray(checks) || !checks.length ||
      checks.some((check) => !check || check.source !== 'synthetic-host-check' || check.pass !== true)) {
    throw new Error('Missing trusted fixture checks');
  }
  const receipt = snapshot({ source: 'synthetic-independent-host', subject: offlineDigest(subject),
    checks: offlineDigest(checks), verdict: 'pass' });
  receipts.add(receipt);
  return receipt;
}

// An injected logical clock tests deadline transitions without timers or claims
// about real wall-clock/process/remote execution enforcement.
export async function createOfflineAttempt({ root, contract, signal, now = () => 0, read }) {
  const input = snapshot(contract);
  shape(input, ['ask', 'task', 'base', 'run', 'attempt', 'scope', 'config', 'limits']);
  for (const key of ['ask', 'task', 'base', 'run', 'attempt']) text(input[key]);
  shape(input.scope, ['read', 'write', 'delete']);
  const readFiles = scopeList(input.scope.read);
  const writeFiles = scopeList(input.scope.write);
  if (!Array.isArray(input.scope.delete) ||
      input.scope.delete.some((file) => !writeFiles.includes(relativeFile(file))) ||
      new Set(input.scope.delete).size !== input.scope.delete.length) throw new Error('Invalid deletion scope');
  shape(input.config, ['requestedModel', 'executable', 'version']);
  Object.values(input.config).forEach(text);
  shape(input.limits, ['files', 'bytes', 'callbacks', 'events', 'duration']);
  const ceilings = { files: 16, bytes: 1_000_000, callbacks: 32, events: 128, duration: 60_000 };
  for (const [key, ceiling] of Object.entries(ceilings)) {
    if (!Number.isSafeInteger(input.limits[key]) || input.limits[key] < 1 || input.limits[key] > ceiling) {
      throw new Error('Invalid finite attempt limit');
    }
  }
  const anchors = snapshot({
    ask: offlineDigest(input.ask), task: offlineDigest(input.task), base: input.base, baseDigest: offlineDigest(input.base),
    scope: offlineDigest(input.scope), config: offlineDigest(input.config),
    run: input.run, attempt: input.attempt,
  });
  const capabilities = snapshot({
    evidence: 'synthetic-descriptor-only', environment: {}, callbacks: ['read_file'],
    network: false, launch: false, auth: false, shell: false, git: false,
    publisher: false, physicalContainment: false,
  });
  const started = now();
  if (!Number.isFinite(started)) throw new Error('Invalid fixture clock');
  let lastTime = started;
  let state = 'running';
  let reason = null;
  let active = 0;
  let callbackCount = 0;
  let eventCount = 0;
  let idle = false;
  let exit = false;
  let cleanup = false;
  let candidate = null;
  let imported = false;
  let disposed = false;
  const calls = [];
  const ids = new Set();
  const revoke = (why = 'revoked') => {
    state = 'blocked';
    reason ??= why;
    candidate = null;
    imported = false;
  };
  const abort = () => revoke('cancelled');
  signal?.addEventListener('abort', abort, { once: true });
  if (signal?.aborted) abort();
  function live() {
    const time = now();
    if (!Number.isFinite(time) || time < lastTime) revoke('invalid-clock');
    lastTime = time;
    if (time - started >= input.limits.duration) revoke('deadline');
    if (signal?.aborted) abort();
    if (disposed || state === 'blocked') throw new Error(reason ?? 'disposed');
  }
  function fail(error) {
    revoke(error.message);
    throw error;
  }
  let tools;
  try {
    live();
    tools = await createTools({
      worktree: root, allowedFiles: readFiles, env: {}, allowRunTest: false,
      allowInternet: false, sliceReadsOnly: true, scopeExpansion: 0,
    });
    live();
  } catch (error) {
    signal?.removeEventListener('abort', abort);
    throw error;
  }
  function subject() {
    return snapshot({ kind: 'external-full-agent', evidence: 'synthetic', anchors,
      candidate: candidate?.digest ?? null });
  }
  function bound(current) {
    if (offlineDigest(current) !== offlineDigest(anchors)) throw new Error('Task/base/scope/config drift');
  }
  function unchanged() {
    for (const file of candidate.files) {
      const target = checkedFile(root, file.path);
      if (file.delete ? lstatSync(target, { throwIfNoEntry: false }) !== undefined :
        !lstatSync(target, { throwIfNoEntry: false }) ||
        offlineDigest(readFileSync(target, 'utf8')) !== file.hash) throw new Error('Candidate byte drift');
    }
  }
  return Object.freeze({
    anchors, capabilities,
    async callback(name, args) {
      try {
        live();
        if (state !== 'running' || idle || ++callbackCount > input.limits.callbacks) throw new Error('Callback revoked or bounded');
        const copied = snapshot(args);
        shape(copied, ['path']);
        if (name !== 'read_file' || !readFiles.includes(relativeFile(copied.path))) throw new Error('Callback capability denied');
        checkedFile(root, copied.path);
        active++;
        try {
          const result = await (read ?? tools.read_file)(copied);
          live();
          if (state !== 'running') throw new Error('Late callback');
          return snapshot(result);
        } finally {
          active--;
        }
      } catch (error) { return fail(error); }
    },
    observe(raw) {
      try {
        live();
        if (state !== 'running' || ++eventCount > input.limits.events) throw new Error('Late or excess event');
        const event = snapshot(raw);
        if (event.type === 'inference') {
          shape(event, ['type', 'id'], ['servedModel', 'usage']);
          text(event.id);
          if (idle || ids.has(event.id)) throw new Error('Late or duplicate inference');
          ids.add(event.id);
          if (event.servedModel !== undefined) text(event.servedModel);
          if (event.usage !== undefined) {
            shape(event.usage, ['unit', 'value', 'complete']);
            text(event.usage.unit);
            if (!Number.isFinite(event.usage.value) || event.usage.value < 0 || event.usage.value > Number.MAX_SAFE_INTEGER ||
                typeof event.usage.complete !== 'boolean') throw new Error('Malformed usage');
          }
          calls.push(event);
        } else if (event.type === 'idle') {
          shape(event, ['type']);
          if (idle || active) throw new Error('Duplicate idle or active callback');
          idle = true;
        } else if (event.type === 'exit') {
          shape(event, ['type', 'code']);
          if (!idle || exit || event.code !== 0) throw new Error('Missing idle or failed exit');
          exit = true;
        } else if (event.type === 'cleanup') {
          shape(event, ['type', 'ok']);
          if (!exit || cleanup || event.ok !== true) throw new Error('Missing exit or cleanup failure');
          cleanup = true;
        } else throw new Error('Unknown event');
      } catch (error) { fail(error); }
    },
    finish(raw) {
      try {
        live();
        if (state !== 'running' || active || !idle || !exit || !cleanup || !calls.length) {
          throw new Error('Missing idle/exit/cleanup/inference evidence');
        }
        const envelope = snapshot(raw);
        shape(envelope, ['kind', 'evidence', 'anchors', 'files'], ['selfReview']);
        if (envelope.kind !== 'external-full-agent' || envelope.evidence !== 'synthetic') throw new Error('Invalid attempt kind');
        bound(envelope.anchors);
        if (!Array.isArray(envelope.files) || !envelope.files.length ||
            envelope.files.length > input.limits.files) throw new Error('Invalid file count');
        const seen = new Set();
        let bytes = 0;
        const files = envelope.files.map((file) => {
          shape(file, ['path'], ['content', 'delete']);
          const relative = relativeFile(file.path);
          if (!writeFiles.includes(relative) || seen.has(relative.toLowerCase())) throw new Error('Scope or duplicate artifact');
          seen.add(relative.toLowerCase());
          checkedFile(root, relative);
          if (file.delete === true && !Object.hasOwn(file, 'content')) {
            if (!input.scope.delete.includes(relative) ||
                !lstatSync(checkedFile(root, relative), { throwIfNoEntry: false })) throw new Error('Deletion denied');
            return { path: relative, delete: true };
          }
          if (Object.hasOwn(file, 'delete') || typeof file.content !== 'string') throw new Error('Malformed artifact bytes');
          if (Buffer.from(file.content, 'utf8').toString('utf8') !== file.content) throw new Error('Malformed UTF-8 bytes');
          bytes += Buffer.byteLength(file.content, 'utf8');
          if (bytes > input.limits.bytes) throw new Error('Artifact bytes exceeded');
          return { path: relative, content: file.content, hash: offlineDigest(file.content) };
        });
        candidate = snapshot({ files, digest: offlineDigest(files) });
        state = 'candidate';
        return candidate;
      } catch (error) { return fail(error); }
    },
    importCandidate(current) {
      try {
        live();
        if (state !== 'candidate' || active) throw new Error('Import denied');
        bound(current);
        for (const file of candidate.files) checkedFile(root, file.path);
        for (const file of candidate.files) {
          const target = checkedFile(root, file.path);
          if (file.delete) unlinkSync(target);
          else {
            mkdirSync(path.dirname(target), { recursive: true });
            writeFileSync(target, file.content, 'utf8');
          }
        }
        imported = true;
        return subject();
      } catch (error) { return fail(error); }
    },
    reviewEligibility(receipt, current) {
      try {
        live();
        if (state !== 'candidate' || !imported) throw new Error('Review denied');
        bound(current);
        unchanged();
        if (!receipts.has(receipt) || receipt.subject !== offlineDigest(subject())) throw new Error('Missing or stale independent host receipt');
        return snapshot({ hostReviewEligible: true, publishable: false, builtinSeat: false, aiRun: null });
      } catch (error) { return fail(error); }
    },
    provenance() {
      const models = [...new Set(calls.map((call) => call.servedModel).filter(Boolean))];
      const usages = calls.map((call) => call.usage);
      const units = [...new Set(usages.filter(Boolean).map((usage) => usage.unit))];
      const complete = calls.length > 0 && usages.every((usage) => usage?.complete);
      const sum = complete && units.length === 1 ? usages.reduce((total, usage) => total + usage.value, 0) : null;
      const safeTotal = sum !== null && Number.isFinite(sum) && sum <= Number.MAX_SAFE_INTEGER;
      const usageStatus = units.length > 1 ? 'mixed' : !usages.some(Boolean) ? 'unknown' :
        !complete || !safeTotal ? 'incomplete' : usages.every((usage) => usage.value === 0) ? 'zero' : 'complete';
      return snapshot({
        kind: 'external-full-agent', evidence: 'synthetic', builtinSeat: false, publishable: false, aiRun: null,
        requestedModel: input.config.requestedModel, servedModels: models,
        modelStatus: models.length > 1 ? 'mixed' : !calls.length || calls.some((call) => !call.servedModel) ? 'unknown' : 'known',
        mismatch: models.some((model) => model !== input.config.requestedModel),
        observedCalls: calls.length, calls, usageStatus, units,
        total: safeTotal ? sum : null,
      });
    },
    status() {
      return snapshot({ state, reason, active, callbackCount, eventCount, imported,
        syntheticCleanupComplete: idle && exit && cleanup && active === 0,
        writerDeathProven: false, publishable: false });
    },
    revoke,
    dispose() {
      disposed = true;
      revoke('disposed');
      signal?.removeEventListener('abort', abort);
    },
  });
}
