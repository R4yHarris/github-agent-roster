const controlNames = Object.freeze([
  'immutableTask', 'exactScope', 'boundedCallbacks', 'candidateHashes',
  'cancellation', 'deadline', 'exitAndCleanup', 'separateProvenance', 'independentReview',
]);
const authorityNames = ['launch', 'network', 'auth', 'shell', 'git', 'publisher'];
const limitNames = ['files', 'bytes', 'callbacks', 'events', 'duration'];
const liveBlockers = Object.freeze([
  'production-capability-bridge-unqualified',
  'physical-containment-unqualified',
  'inference-auth-unapproved',
  'account-data-model-spend-unapproved',
  'remote-cancellation-unqualified',
  'authoring-provenance-unverified',
  'dependency-gate-unresolved',
]);

function record(value, fields, label, { required = true } = {}) {
  if (!value || typeof value !== 'object' || Object.getPrototypeOf(value) !== Object.prototype) {
    throw new TypeError(`${label} must be a plain record`);
  }
  const keys = Reflect.ownKeys(value);
  if (keys.some((key) => !fields.includes(key)) ||
      (required && fields.some((key) => !Object.hasOwn(value, key)))) {
    throw new TypeError(`${label} must use only the documented fields`);
  }
  const result = {};
  for (const key of keys) {
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    if (!descriptor.enumerable || !Object.hasOwn(descriptor, 'value')) {
      throw new TypeError(`${label} must contain enumerable data properties`);
    }
    result[key] = descriptor.value;
  }
  return result;
}

/**
 * Assess a synthetic capability declaration, not execution readiness or a grant.
 * This module deliberately has no transport, environment, filesystem or publisher access.
 */
export function assessCopilotWorkerCapabilities(report) {
  const input = record(report,
    ['schema', 'kind', 'evidence', 'concurrency', 'tools', 'authority', 'limits', 'controls'],
    'Offline worker capability report');
  if (input.schema !== 1 || input.kind !== 'external-full-agent' ||
      input.evidence !== 'synthetic' || input.concurrency !== 1) {
    throw new TypeError('Offline report requires schema 1, external-full-agent, synthetic evidence and concurrency 1');
  }
  // Only the existing Gate B fixture's read callback is recognized.
  const tools = input.tools;
  if (!Array.isArray(tools) || Object.getPrototypeOf(tools) !== Array.prototype ||
      tools.length !== 1 || Reflect.ownKeys(tools).length !== 2) {
    throw new TypeError('Offline tools must contain only read_file');
  }
  const tool = Object.getOwnPropertyDescriptor(tools, '0');
  if (!tool?.enumerable || !Object.hasOwn(tool, 'value') || tool.value !== 'read_file') {
    throw new TypeError('Offline tools must contain only read_file');
  }
  const authority = record(input.authority, authorityNames, 'Offline authority');
  if (authorityNames.some((key) => authority[key] !== false)) {
    throw new TypeError('Offline launch, network, auth, shell, git and publisher authority must be false');
  }
  const limits = record(input.limits, limitNames, 'Offline limits');
  if (limitNames.some((key) => !Number.isSafeInteger(limits[key]) || limits[key] < 1)) {
    throw new TypeError('Offline limits must all be finite positive safe integers');
  }
  const controls = record(input.controls, controlNames, 'Offline controls', { required: false });
  if (Object.values(controls).some((value) => value !== null && typeof value !== 'boolean')) {
    throw new TypeError('Offline controls must be true, false or null (unknown)');
  }
  const unknownControls = Object.freeze(controlNames.filter((key) =>
    !Object.hasOwn(controls, key) || controls[key] === null));
  const unsupportedControls = Object.freeze(controlNames.filter((key) => controls[key] === false));
  const declaredControls = Object.freeze(Object.fromEntries(controlNames.map((key) =>
    [key, Object.hasOwn(controls, key) ? controls[key] : null])));
  return Object.freeze({
    schema: 1,
    kind: 'external-full-agent',
    evidence: 'synthetic',
    status: unknownControls.length || unsupportedControls.length ? 'blocked' : 'offline-contract-compatible',
    offlineContractCompatible: unknownControls.length === 0 && unsupportedControls.length === 0,
    unknownControls,
    unsupportedControls,
    capabilities: Object.freeze({
      concurrency: 1,
      tools: Object.freeze(['read_file']),
      authority: Object.freeze(authority),
      limits: Object.freeze(limits),
      controls: declaredControls,
    }),
    liveDispatchEligible: false,
    publicationEligible: false,
    liveBlockers,
  });
}
