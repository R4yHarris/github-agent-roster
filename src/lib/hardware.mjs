export const hardwareIndex = Object.freeze({
  'rtx-3090-24gb': { class: 'last-resort', memory_gb: 24, nodes: 1, notes: 'Local workstation GPU' },
  'rtx-6000-ada-48gb': { class: 'dedicated', memory_gb: 48, nodes: 1, notes: 'Dedicated data-center GPU' },
  'dgx-spark-gb10-128gb': { class: 'cluster', memory_gb: 128, nodes: 1, notes: 'One GB10 Spark, 128GB unified memory' },
  'dgx-spark-2n': { class: 'cluster', memory_gb: 256, nodes: 2, notes: 'Two GB10 Sparks' },
  'dgx-spark-4n': { class: 'cluster', memory_gb: 512, nodes: 4, notes: 'Four GB10 Sparks' },
});

export function describeHardware(id) {
  const record = hardwareIndex[id];
  if (!record) throw new TypeError(`Unknown hardware id: ${id}`);
  return Object.freeze({ id, ...record });
}

export function hardwareCost(profile, difficulty) {
  const record = hardwareIndex[profile.hardware];
  const kind = record?.class ?? (/3090/.test(profile.hardware) ? 'last-resort'
    : /6000|aperture/.test(profile.hardware) ? 'dedicated'
      : /spark|dgx/.test(profile.hardware) ? 'cluster' : 'unspecified');
  const context = profile.context_max > 0 ? profile.context_max / 1_000_000 : 2;
  if (kind === 'last-resort') return 100 + context;
  if (kind === 'cluster') return (difficulty >= 4 ? 8 : 30) + context + (record?.nodes ?? 1);
  if (kind === 'dedicated') return 10 + context;
  return 20 + context;
}
