import { createInterface } from 'node:readline/promises';
import { fileURLToPath } from 'node:url';
import { formatConfig, loadConfig, parseConfig } from './config.mjs';
import { formatFleet, getFleetProfile, normalizeFleetBaseUrl, parseFleet, validateFleet,
  validateFleetId, validateFleetProfile, withFleetProfile, writeFleet } from './fleet.mjs';
import { resolveProjectRoot } from './paths.mjs';
import { ensurePrivateFilesIgnored, readPrivateFile, writePrivateDocuments } from './private-files.mjs';
import { probeModelDetails } from '../onboard/wizard.mjs';
import { runFleetAssist } from '../onboard/fleet-assist.mjs';

const installation = fileURLToPath(new URL('../../', import.meta.url));
const usage = 'Use roster fleet list, add --id NAME --base-url URL [--model MODEL] [--context N] ' +
  '[--concurrency N] [--hardware TEXT] [--task-class feat,fix,docs,test], ' +
  'probe ID [--set-model [MODEL]], default ID, remove ID, or assist.';

function argumentsFor(args) {
  if (!Array.isArray(args) || !args.length || args.some((value) => typeof value !== 'string')) {
    throw new TypeError(usage);
  }
  const [command, ...rest] = args;
  if (command === 'list' && !rest.length) return { command };
  if (command === 'assist' && !rest.length) return { command };
  if (['default', 'remove'].includes(command) && rest.length === 1) {
    return { command, id: validateFleetId(rest[0]) };
  }
  if (command === 'probe' && rest.length >= 1 && rest.length <= 3 &&
      (rest.length === 1 || (rest[1] === '--set-model' && !rest[2]?.startsWith('--')))) {
    return { command, id: validateFleetId(rest[0]), setModel: rest.length > 1, model: rest[2] };
  }
  if (command !== 'add' || rest.length % 2) throw new TypeError(usage);
  const options = { command };
  const flags = { '--id': 'id', '--base-url': 'baseUrl', '--model': 'model', '--context': 'context',
    '--concurrency': 'concurrency', '--hardware': 'hardware', '--task-class': 'taskClass' };
  for (let index = 0; index < rest.length; index += 2) {
    const key = Object.hasOwn(flags, rest[index]) ? flags[rest[index]] : undefined;
    const value = rest[index + 1];
    if (!key || Object.hasOwn(options, key) || !value || value.startsWith('--')) throw new TypeError(usage);
    options[key] = value;
  }
  if (!options.id || !options.baseUrl) throw new TypeError(usage);
  validateFleetId(options.id);
  options.baseUrl = normalizeFleetBaseUrl(options.baseUrl);
  return options;
}

function positiveInteger(value, field) {
  if (typeof value !== 'string' || !/^[1-9]\d*$/.test(value) || !Number.isSafeInteger(Number(value))) {
    throw new TypeError(`Fleet ${field} must be a positive safe integer`);
  }
  return Number(value);
}

function matchesDefault(config, profile) {
  return Boolean(config.llm.base_url && config.llm.model === profile.model &&
    normalizeFleetBaseUrl(config.llm.base_url) === profile.base_url);
}

async function currentConfig(repoRoot, cwd, installationRoot) {
  const source = await readPrivateFile(repoRoot, 'config.yml');
  return { source, config: source === null ? loadConfig({ repoRoot: installationRoot, cwd }) : parseConfig(source) };
}

export async function runFleet(args, {
  cwd = process.cwd(), installationRoot = installation, env = process.env,
  input = process.stdin, output = process.stdout, errorOutput = process.stderr,
  fetchImpl = globalThis.fetch, vault, question,
} = {}) {
  const options = argumentsFor(args);
  if (options.command === 'assist') {
    return runFleetAssist({ cwd, installationRoot, env, input, output, errorOutput, fetchImpl, vault, question });
  }
  const tty = Boolean(input.isTTY && output.isTTY);
  if (options.command === 'add' && !tty && !options.model) {
    throw new TypeError('Non-TTY fleet add requires --id, --base-url, and --model');
  }
  if (options.command === 'probe' && options.setModel && !options.model && !tty) {
    throw new TypeError('Non-TTY fleet probe --set-model requires a model ID');
  }
  const repoRoot = resolveProjectRoot(cwd);
  const previousFleet = await readPrivateFile(repoRoot, 'fleet.yml');
  const fleet = previousFleet === null ? validateFleet({ profiles: [] }) : parseFleet(previousFleet);
  let terminal;
  const ask = async (prompt) => {
    if (!tty) throw new TypeError('Fleet questions need a terminal or explicit flags');
    terminal ??= question ? null : createInterface({ input, output, terminal: true });
    const answer = await (question ? question(prompt) : terminal.question(prompt));
    if (typeof answer !== 'string') throw new Error('Fleet setup ended before an answer was supplied');
    return answer.trim();
  };
  const selectModel = async (ids, supplied) => {
    output.write('Available models:\n' + ids.map((id, index) => `  ${index + 1}. ${id}\n`).join(''));
    if (supplied !== undefined) {
      if (!ids.includes(supplied)) throw new Error('Requested model is not in the endpoint models list');
      return supplied;
    }
    for (;;) {
      const choice = await ask('Select a model [1]: ') || '1';
      if (/^[1-9]\d*$/.test(choice) && Number.isSafeInteger(Number(choice)) && Number(choice) <= ids.length) {
        return ids[Number(choice) - 1];
      }
      output.write('Enter a listed model number.\n');
    }
  };
  try {
    if (options.command === 'list') {
      if (!fleet.profiles.length) output.write('No fleet profiles configured. Run roster onboard or roster fleet add.\n');
      else {
        output.write('ID\tMODEL\tBASE_URL\tCONTEXT_MAX\tCONCURRENCY\tHARDWARE\tTASK_CLASS\n');
        for (const profile of fleet.profiles) {
          output.write([profile.id, profile.model, profile.base_url, profile.context_max || 'unknown',
            profile.concurrency, profile.hardware, profile.task_class?.join(',') ?? '-'].join('\t') + '\n');
        }
      }
      return { command: options.command, fleet };
    }
    if (options.command === 'add') {
      if (fleet.profiles.some(({ id }) => id === options.id)) throw new Error('Fleet profile ID already exists');
      const { config } = await currentConfig(repoRoot, cwd, installationRoot);
      const models = await probeModelDetails(options.baseUrl, { fetchImpl, env, apiKeyEnv: config.llm.api_key_env });
      const ids = models.map(({ id }) => id);
      const model = await selectModel(ids, options.model);
      const reportedContext = models.find(({ id }) => id === model)?.context_max;
      let context = options.context ?? (reportedContext === undefined ? undefined : String(reportedContext));
      if (!context && !tty) {
        throw new TypeError('Non-TTY fleet add requires --context when /v1/models does not report a context limit');
      }
      while (!context) {
        context = await ask('Model context limit in tokens [required]: ');
        if (!context) output.write('Supply a positive context limit; capacity is not guessed.\n');
      }
      if (options.context === undefined && reportedContext !== undefined) {
        output.write(`Using context_max=${reportedContext} reported by /v1/models for ${model}.\n`);
      }
      const profile = validateFleetProfile({
        id: options.id, base_url: options.baseUrl, model, provider: 'vllm',
        context_max: positiveInteger(context, 'context'), concurrency: options.concurrency === undefined
          ? 1 : positiveInteger(options.concurrency, 'concurrency'),
        hardware: options.hardware ?? 'unspecified',
        ...(options.taskClass ? { task_class: options.taskClass.split(',') } : {}), notes: '',
      });
      const updated = await writeFleet({ profiles: [...fleet.profiles, profile] }, { repoRoot, expectedSource: previousFleet });
      output.write(`Added fleet profile: ${profile.id}. Default config unchanged.\n`);
      return { command: options.command, fleet: updated, profile };
    }
    const profile = getFleetProfile(fleet, options.id);
    if (options.command === 'probe') {
      const { config, source: previousConfig } = await currentConfig(repoRoot, cwd, installationRoot);
      const models = await probeModelDetails(profile.base_url, { fetchImpl, env, apiKeyEnv: config.llm.api_key_env });
      const ids = models.map(({ id }) => id);
      if (!options.setModel) {
        output.write('Available models:\n' + models.map(({ id, context_max: contextMax }) =>
          `  ${id}${contextMax === undefined ? '' : ` (context_max=${contextMax})`}\n`).join(''));
        output.write(`Saved model unchanged: ${profile.model}\n`);
        const contextMax = models.find(({ id }) => id === profile.model)?.context_max;
        return { command: options.command, models: ids, modelDetails: models, profile,
          ...(contextMax === undefined ? {} : { context_max: contextMax }) };
      }
      const model = await selectModel(ids, options.model);
      const contextMax = models.find(({ id }) => id === model)?.context_max;
      const selected = { ...profile, model, ...(contextMax === undefined ? {} : { context_max: contextMax }) };
      const updated = validateFleet({ profiles: fleet.profiles.map((item) =>
        item.id === profile.id ? selected : item) });
      const documents = [{ name: 'fleet.yml', source: formatFleet(updated), expectedSource: previousFleet }];
      const names = ['fleet.yml'];
      if (matchesDefault(config, profile)) {
        documents.push({ name: 'config.yml', source: formatConfig(withFleetProfile(config, selected)),
          expectedSource: previousConfig });
        names.push('config.yml');
      }
      await ensurePrivateFilesIgnored(repoRoot, names);
      await writePrivateDocuments(documents, { repoRoot });
      output.write(`Updated saved model for ${profile.id}. ` + (contextMax === undefined
        ? 'Recheck its declared context limit for the new model.\n'
        : `Using context_max=${contextMax} reported by /v1/models.\n`));
      return { command: options.command, fleet: updated, profile: getFleetProfile(updated, options.id) };
    }
    const { config, source: previousConfig } = await currentConfig(repoRoot, cwd, installationRoot);
    if (options.command === 'default') {
      await ensurePrivateFilesIgnored(repoRoot, ['config.yml']);
      await writePrivateDocuments([{ name: 'config.yml', source: formatConfig(withFleetProfile(config, profile)),
        expectedSource: previousConfig }], { repoRoot });
      output.write(`Default endpoint/model selected: ${profile.id}\n`);
      return { command: options.command, profile };
    }
    if (fleet.profiles.length === 1) throw new Error('Cannot remove the last fleet profile');
    if (matchesDefault(config, profile)) {
      throw new Error('Select another fleet default before removing the currently configured endpoint');
    }
    const updated = await writeFleet({ profiles: fleet.profiles.filter(({ id }) => id !== profile.id) }, {
      repoRoot, expectedSource: previousFleet,
    });
    output.write(`Removed fleet profile: ${profile.id}\n`);
    return { command: options.command, fleet: updated };
  } finally {
    terminal?.close();
  }
}
