import { loadCapabilities, validateCapabilities } from './capabilities.mjs';
import { getFleetProfile, loadFleet, validateFleet } from './fleet.mjs';
import {
  deriveDifficultyCeilings, EFFORTS, formatRecommendation, IDENTIFIER, learningSeat, summarizeLearning, TASK_CLASSES,
} from './learn.mjs';
import { hardwareCost } from './hardware.mjs';
import { resolveProjectRoot } from './paths.mjs';

function preference(profile, difficulty) {
  return -hardwareCost(profile, difficulty);
}

function contextFit(profile) {
  return profile.context_max > 0 ? profile.context_max : Number.MAX_SAFE_INTEGER;
}

export function chooseRoute({
  fleet, capabilities, records = [], taskClass, difficulty = 2, contextRequired = 0, profileId, seat = 'coder',
}) {
  if (!TASK_CLASSES.includes(taskClass)) throw new TypeError('Routing task class must be feat, fix, docs, or test');
  if (!Number.isInteger(difficulty) || difficulty < 1 || difficulty > 5) {
    throw new TypeError('Routing difficulty must be an integer from 1 to 5');
  }
  if (!Number.isSafeInteger(contextRequired) || contextRequired < 0) {
    throw new TypeError('Routing context requirement must be a nonnegative safe integer');
  }
  if (!Array.isArray(records) || records.some((record) =>
    !record || typeof record !== 'object' || Array.isArray(record) ||
    (record.evaluation != null && (typeof record.evaluation !== 'object' || Array.isArray(record.evaluation))))) {
    throw new TypeError('Routing requires joined run/evaluation records');
  }
  if (typeof seat !== 'string' || !IDENTIFIER.test(seat)) {
    throw new TypeError('Routing seat must be an opaque 1-64 character identifier');
  }
  const ceilings = new Map(deriveDifficultyCeilings(records).filter((entry) => entry.seat === seat)
    .map((entry) => [entry.model, entry.ceiling]));
  const catalog = validateFleet(fleet);
  const priors = validateCapabilities(capabilities).capabilities;
  const profiles = (profileId === undefined ? catalog.profiles : [getFleetProfile(catalog, profileId)])
    .filter((profile) => !ceilings.has(profile.model) || ceilings.get(profile.model) >= difficulty);
  const human = records.filter((record) => record.evaluation != null &&
    learningSeat(record) === seat);
  const groups = summarizeLearning(human).filter((group) =>
    group.task_class === taskClass && group.n >= 3 &&
    group.medianDifficulty !== null && group.medianDifficulty >= difficulty);
  const evaluated = profiles.filter((profile) =>
    profile.context_max > 0 && profile.context_max >= contextRequired).flatMap((profile) =>
    groups.filter((group) => group.model === profile.model).map((group) => ({
      profile, source: 'evals', reason: `${group.n} distinct human evaluations; declared context is sufficient`,
      recommendation: Object.fromEntries(['model', 'effort', 'n', 'accepted', 'acceptRate',
        'medianMinutes', 'medianDifficulty', 'estimate_min'].map((name) => [name, group[name]])),
    })));
  evaluated.sort((left, right) => right.recommendation.acceptRate - left.recommendation.acceptRate ||
    right.recommendation.n - left.recommendation.n || left.profile.id.localeCompare(right.profile.id) ||
    [...EFFORTS, null].indexOf(left.recommendation.effort) - [...EFFORTS, null].indexOf(right.recommendation.effort));
  if (evaluated.length) return evaluated[0];

  const candidates = profiles.flatMap((profile) => {
    if (contextRequired > 0 && profile.context_max < contextRequired) return [];
    const prior = priors.find((record) => record.profile_id === profile.id && record.task_class === taskClass) ??
      priors.find((record) => record.model_id === profile.model && record.task_class === taskClass);
    const hinted = profile.task_class?.includes(taskClass) ?? false;
    if (prior ? prior.suggested_difficulty < difficulty : !hinted) return [];
    return [{
      profile, source: 'prior', recommendation: null, hinted,
      reason: `${prior ? 'capability prior' : 'fleet task-class hint'}; smallest sufficient hardware; starting guess, not a benchmark` +
        (profile.context_max === 0 ? '; context capacity remains unknown' : ''),
    }];
  });
  candidates.sort((left, right) => Number(right.hinted) - Number(left.hinted) ||
    Number(preference(right.profile, difficulty)) - Number(preference(left.profile, difficulty)) ||
    contextFit(left.profile) - contextFit(right.profile) ||
    right.profile.concurrency - left.profile.concurrency || left.profile.id.localeCompare(right.profile.id));
  if (!candidates.length) return null;
  const { hinted, ...choice } = candidates[0];
  return choice;
}

export async function routeTask({
  cwd = process.cwd(), repoRoot = resolveProjectRoot(cwd), installationRoot,
  fleet, capabilities, ...options
}) {
  const [catalog, priors] = await Promise.all([
    fleet ?? loadFleet({ cwd, repoRoot }),
    capabilities ?? loadCapabilities({ cwd, repoRoot, installationRoot }),
  ]);
  return chooseRoute({ ...options, fleet: catalog, capabilities: priors });
}

export function formatRoute(choice, taskClass, config, env = process.env) {
  if (!choice) return formatRecommendation(null, taskClass, config, env);
  const { profile, source, reason, recommendation } = choice;
  const prefix = recommendation ? formatRecommendation(recommendation, taskClass).trimEnd()
    : `${taskClass}: ${profile.model}`;
  return `${prefix} profile=${profile.id} source=${source} context_max=${profile.context_max || 'unknown'}` +
    ` concurrency=${profile.concurrency} reason=${reason}\n`;
}
