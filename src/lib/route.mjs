import { loadCapabilities, validateCapabilities } from './capabilities.mjs';
import { getFleetProfile, loadFleet, validateFleet } from './fleet.mjs';
import {
  deriveDifficultyCeilings, EFFORTS, formatRecommendation, IDENTIFIER, inferTaskClass, learningSeat, summarizeLearning, TASK_CLASSES,
} from './learn.mjs';
import { hardwareCost } from './hardware.mjs';
import { resolveProjectRoot } from './paths.mjs';
import { depth as admissionDepth } from '../runtime/admission.mjs';

function preference(profile, difficulty) {
  return -hardwareCost(profile, difficulty);
}

function contextFit(profile) {
  return profile.context_max > 0 ? profile.context_max : Number.MAX_SAFE_INTEGER;
}

export function explainRouteEvidence(records, { model, taskClass, seat = 'coder', difficulty = 2,
  effort, now = Date.now() }) {
  if (!Number.isFinite(now)) throw new TypeError('Routing evidence clock must be finite milliseconds');
  if (!Array.isArray(records) || records.some((record) => !record || typeof record !== 'object' ||
      Array.isArray(record))) throw new TypeError('Routing evidence requires joined records');
  if (!TASK_CLASSES.includes(taskClass) || typeof model !== 'string' || !model ||
      typeof seat !== 'string' || !IDENTIFIER.test(seat) ||
      !Number.isInteger(difficulty) || difficulty < 1 || difficulty > 5 ||
      effort !== undefined && effort !== null && !EFFORTS.includes(effort)) {
    throw new TypeError('Invalid routing evidence scope');
  }
  const human = records.filter((record) => record.model === model &&
    record.evaluation != null && learningSeat(record) === seat);
  const groups = summarizeLearning(human).filter((group) => group.task_class === taskClass &&
    (effort === undefined || group.effort === effort));
  if (!groups.length) groups.push({ effort: effort ?? null, n: 0, accepted: 0,
    acceptRate: null, medianDifficulty: null });
  return groups.map((group) => {
    const times = human.filter((record) => (record.task_class ?? inferTaskClass(record.task)) === taskClass &&
      (record.effort ?? null) === group.effort &&
      ['accept', 'reject'].includes(record.evaluation.verdict))
      .map((record) => Date.parse(record.evaluation.at)).filter((time) => Number.isFinite(time) && time <= now);
    const latest = times.reduce((value, time) => value === null ? time : Math.max(value, time), null);
    return {
      model, seat, task_class: taskClass, effort: group.effort,
      samples: group.n, accepted: group.accepted, rejected: group.n - group.accepted,
      acceptRate: group.acceptRate, medianDifficulty: group.medianDifficulty,
      latestEvaluation: latest === null ? null : new Date(latest).toISOString(),
      ageDays: latest === null ? null : Math.floor((now - latest) / 86400000),
      sufficient: group.n >= 3 && group.medianDifficulty !== null && group.medianDifficulty >= difficulty,
    };
  });
}

export function chooseRoute({
  fleet, capabilities, records = [], taskClass, difficulty = 2, contextRequired = 0, profileId, seat = 'coder',
  excludedProfileIds = [], queueDepth = admissionDepth, now = Date.now(),
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
  if (!Array.isArray(excludedProfileIds) ||
      excludedProfileIds.some((id) => typeof id !== 'string' || !IDENTIFIER.test(id)) ||
      new Set(excludedProfileIds).size !== excludedProfileIds.length) {
    throw new TypeError('Excluded fleet profiles must be distinct opaque identifiers');
  }
  const excluded = new Set(excludedProfileIds);
  const ceilings = new Map(deriveDifficultyCeilings(records).filter((entry) => entry.seat === seat)
    .map((entry) => [entry.model, entry.ceiling]));
  const catalog = validateFleet(fleet);
  const priors = validateCapabilities(capabilities).capabilities;
  const profiles = (profileId === undefined ? catalog.profiles : [getFleetProfile(catalog, profileId)])
    .filter((profile) => !excluded.has(profile.id))
    .filter((profile) => !ceilings.has(profile.model) || ceilings.get(profile.model) >= difficulty);
  // Spec §5 fleet: queue depth only breaks ties left by eval history and priors; a busy profile is not a failed one.
  const depths = new Map(profiles.map((profile) => [profile.id, queueDepth(profile.id)]));
  const byDepth = (left, right) => depths.get(left.profile.id) - depths.get(right.profile.id);
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
    right.recommendation.n - left.recommendation.n || byDepth(left, right) || left.profile.id.localeCompare(right.profile.id) ||
    [...EFFORTS, null].indexOf(left.recommendation.effort) - [...EFFORTS, null].indexOf(right.recommendation.effort));
  const explain = (choice) => ({ ...choice, evidence: explainRouteEvidence(records, {
    model: choice.profile.model, seat, taskClass, difficulty,
    effort: choice.recommendation?.effort, now,
  }) });
  if (evaluated.length) return explain(evaluated[0]);

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
    contextFit(left.profile) - contextFit(right.profile) || byDepth(left, right) ||
    right.profile.concurrency - left.profile.concurrency || left.profile.id.localeCompare(right.profile.id));
  if (!candidates.length) return null;
  const { hinted, ...choice } = candidates[0];
  return explain(choice);
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
    ` concurrency=${profile.concurrency} reason=${reason}\n` +
    (choice.evidence ?? []).map((entry) =>
      `evidence model=${entry.model} seat=${entry.seat} task-class=${entry.task_class} effort=${entry.effort ?? '-'} ` +
      `origin=${entry.samples ? 'local-human-evaluations' : 'none'} n=${entry.samples} ` +
      `accepted=${entry.accepted} rejected=${entry.rejected} ` +
      `accept-rate=${entry.acceptRate === null ? 'unknown' : `${(entry.acceptRate * 100).toFixed(1)}%`} ` +
      `latest=${entry.latestEvaluation ?? 'unknown'} age-days=${entry.ageDays ?? 'unknown'} ` +
      `warning=${entry.sufficient ? 'none' : entry.samples ? 'insufficient-qualifying-evidence' : 'no-human-evidence'}\n`).join('');
}
