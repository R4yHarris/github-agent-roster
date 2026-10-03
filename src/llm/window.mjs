export const contextFields = Object.freeze(['max_model_len', 'max_context_length', 'max_context_len',
  'context_length', 'context_window', 'context_max']);

export function reportedContextMax(entry) {
  const value = contextFields.map((field) => entry?.[field])
    .find((candidate) => Number.isSafeInteger(candidate) && candidate > 0);
  return value === undefined ? undefined : value;
}

export function contextMaxForModel(models, model, fallback) {
  if (!Array.isArray(models)) return fallback;
  const entry = models.find((candidate) => candidate?.id === model);
  const reported = entry?.context_max;
  return Number.isSafeInteger(reported) && reported > 0 ? reported : fallback;
}
