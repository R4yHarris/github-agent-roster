import { lstatSync } from 'node:fs';
import { resolvePublishModel } from '../metrics/run.mjs';

const usage = 'Use roster doctor --copilot [--opt-in] [--runtime PATH] [--model ID].';
const authReasons = Object.freeze({
  unknown: 'Authentication is unverified; offline diagnostics never inspect credentials or contact the provider.',
  'signed-out': 'Declared signed out. After capability and account approval, the human must use official interactive CLI authentication.',
  expired: 'Declared expired authentication. The human must renew separately approved inference authentication; no automatic login or retry.',
  revoked: 'Declared revoked authentication. Ask the account administrator to review access before human reauthentication.',
  'policy-disabled': 'Declared Copilot policy disabled. Ask the organization administrator; Roster cannot change policy.',
  'permission-denied': 'Declared permission denied. Review the separately approved inference account and permissions; never reuse the publishing App.',
});
const sourceReasons = Object.freeze({
  unknown: 'Authentication source is unknown; no ambient credential or CLI login is accepted as inference authorization.',
  environment: 'Declared environment token source. Ambient tokens are not approved inference credentials and must not be forwarded.',
  'stored-oauth': 'Declared stored CLI OAuth source. Host credential stores must remain inaccessible to the future worker.',
  'gh-fallback': 'Declared gh fallback source. Human gh authentication must not be inherited by the future worker.',
  'separate-inference': 'Declared separate inference source; scope, expiry, revocation and isolation still require independent approval.',
});
const modelReasons = Object.freeze({
  unknown: 'Model availability and entitlement are unverified; an exact requested ID does not prove access.',
  unavailable: 'Declared requested model unavailable. Review the account/model policy; do not silently select another model.',
  'policy-disabled': 'Declared model policy disabled. Ask the organization administrator; Roster cannot enable the model.',
});

export function parseCopilotDoctorArgs(args) {
  if (!Array.isArray(args)) throw new TypeError(usage);
  const options = { optIn: false };
  const seen = new Set();
  for (let index = 0; index < args.length; index += 1) {
    const flag = args[index];
    if (!['--opt-in', '--runtime', '--model'].includes(flag) || seen.has(flag)) throw new TypeError(usage);
    seen.add(flag);
    if (flag === '--opt-in') options.optIn = true;
    else {
      const value = args[++index];
      if (typeof value !== 'string' || !value.trim() || value.startsWith('--')) throw new TypeError(usage);
      options[flag === '--runtime' ? 'runtimePath' : 'model'] = value;
    }
  }
  return options;
}

// Declared states are offline fixture evidence, not a provider probe or a
// capability receipt. Nothing supplied here can qualify or enable a worker.
export function checkCopilotDoctor({
  optIn = false,
  runtimePath,
  model,
  isTTY = Boolean(process.stdin.isTTY && process.stdout.isTTY),
  inspect = lstatSync,
  authState = 'unknown',
  authSource = 'unknown',
  modelState = 'unknown',
} = {}) {
  if (typeof optIn !== 'boolean' || typeof isTTY !== 'boolean' ||
      typeof inspect !== 'function' ||
      !Object.hasOwn(authReasons, authState) || !Object.hasOwn(sourceReasons, authSource) ||
      !Object.hasOwn(modelReasons, modelState)) {
    throw new TypeError('Invalid offline Copilot diagnostic options; use documented states only.');
  }
  if (runtimePath !== undefined) {
    if (typeof runtimePath !== 'string' || !runtimePath.trim() || /[\x00-\x1f\x7f]/.test(runtimePath)) {
      throw new TypeError('Runtime path must be a nonempty local filesystem path without ASCII control characters.');
    }
    const uriLike = /^[A-Za-z][A-Za-z0-9+.-]*:/.test(runtimePath) && !/^[A-Za-z]:/.test(runtimePath);
    if (/^[\\/]{2}|^[\\/]\?\?[\\/]/.test(runtimePath) || uriLike) {
      throw new TypeError('Runtime path must be local; network, device namespace and URI paths are not allowed.');
    }
  }
  if (model !== undefined) {
    try {
      resolvePublishModel({ model, ghcp: true, env: {} });
    } catch (error) {
      if (!(error instanceof TypeError)) throw error;
      throw new TypeError('Requested model must be an exact model ID, not a placeholder or whitespace.');
    }
  }
  let runtimeReason = 'No runtime path supplied. Installation is unverified; supply a human-selected pinned executable with --runtime PATH for metadata inspection only.';
  let runtimePresent = false;
  if (!optIn) {
    runtimeReason = 'Runtime inspection skipped: explicitly opt in to offline diagnostics with --opt-in. This does not enable Copilot execution.';
  } else if (runtimePath !== undefined) {
    try {
      const status = inspect(runtimePath);
      runtimePresent = status.isFile() && !status.isSymbolicLink();
      runtimeReason = runtimePresent
        ? 'Regular file present only; executable identity, version, platform support and containment are unqualified. Mounted-drive locality is not proven; the operator must choose a local filesystem. No executable was launched.'
        : 'Runtime path is not a regular nonsymlink file. Choose a trusted pinned executable; Roster will not install or launch it.';
    } catch (error) {
      if (error?.code === 'ENOENT' || error?.code === 'ENOTDIR') {
        runtimeReason = 'Runtime file is missing. Consult official CLI installation guidance; Roster will not install it.';
      } else if (error?.code === 'EACCES' || error?.code === 'EPERM') {
        runtimeReason = 'Runtime metadata permission denied. Ask the human to review the path and access; Roster will not change permissions.';
      } else {
        throw new Error('Runtime metadata inspection failed; review the supplied path and filesystem access.');
      }
    }
  }
  const checks = [
    { name: 'offline diagnostic opt-in', ok: optIn,
      reason: optIn ? 'Explicit for this invocation only; no provider or saved default was changed.'
        : 'Copilot remains off. Use --opt-in only to inspect offline readiness; local/stub defaults are unchanged.' },
    { name: 'runtime file metadata', ok: runtimePresent, reason: runtimeReason },
    { name: 'runtime capability qualification', ok: false,
      reason: 'Unqualified: #571 production containment/capability and auth-mode gates remain. Copilot execution is unavailable.' },
    { name: 'interactive auth handoff', ok: isTTY,
      reason: isTTY ? 'A terminal is present, but diagnostics never start login; authentication remains human-controlled.'
        : 'No interactive terminal. Offline reporting is allowed; human authentication must happen separately in a terminal after approval.' },
    { name: 'inference authentication', ok: false, reason: authReasons[authState] },
    { name: 'inference auth source', ok: false, reason: sourceReasons[authSource] },
    { name: 'model availability', ok: false, reason: modelReasons[modelState] },
    { name: 'account/data/policy/budget approval', ok: false,
      reason: 'License, policy, repository data disclosure, retention and spend authorization are unknown. Cloud execution stays off.' },
  ];
  return { ok: false, enabled: false, qualified: false, evidence: 'offline-declared', checks };
}

export function formatCopilotDoctor(result) {
  if (result?.ok !== false || result.enabled !== false || result.qualified !== false ||
      result.evidence !== 'offline-declared' || !Array.isArray(result.checks) ||
      result.checks.length !== 8 ||
      result.checks.some((check) => typeof check?.name !== 'string' ||
        typeof check.ok !== 'boolean' || typeof check.reason !== 'string')) {
    throw new TypeError('Expected blocked offline Copilot diagnostic checks.');
  }
  return 'Copilot offline diagnostics: BLOCKED (not provider enablement or live qualification)\n' +
    result.checks.map((check) => `${check.ok ? 'OK' : 'FAIL'} ${check.name}: ${check.reason}`).join('\n') +
    '\nAuth guidance: supported environment tokens take precedence (COPILOT_GITHUB_TOKEN, GH_TOKEN, GITHUB_TOKEN), followed by stored CLI OAuth and possible gh fallback. Diagnostics read none of these sources and launch no child process. Never forward ambient tokens, GITHUB_APP_* publishing credentials, host HOME/keychain or gh login to inference.\n' +
    'Login/logout guidance: after separate capability/account approval, the human should follow official CLI authentication and logout instructions in their own terminal. Roster does not extract, display, copy or delete credentials. Logout alone does not remove an environment-token override; the human must review its source separately.\n' +
    'Official guidance: https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/authenticate-copilot-cli\n' +
    'Install guidance: https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/install-copilot-cli (npm CLI requires Node 22+; Roster remains Node 20 ESM with no runtime dependencies).\n';
}
