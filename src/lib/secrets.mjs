import { createFileVault, validateSecretName } from '../vault/file.mjs';

export async function resolveSecret(name, { env = process.env, vault } = {}) {
  validateSecretName(name);
  const value = Object.hasOwn(env, name) ? env[name] : undefined;
  if (value !== undefined && typeof value !== 'string') {
    throw new TypeError('A secret environment value must be a string.');
  }
  if (value) return value;

  let stored;
  try {
    stored = await (vault ?? createFileVault()).get(name);
  } catch {
    throw new Error('Unable to resolve the secret from the file vault.');
  }
  if (stored !== undefined && (typeof stored !== 'string' || stored.length === 0)) {
    throw new TypeError('The file vault returned an invalid secret.');
  }
  return stored;
}
