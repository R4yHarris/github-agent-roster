import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import {
  EFFORTS, failureDetail, isObject, joinLearning, loadLearning, parseJsonl, validateSha,
} from "./learn.mjs";
import { resolveContractsPath } from "./paths.mjs";

const EXPORT_SOURCE = "export-agent-metrics.mjs output";

function validateRun(record, source) {
  if (!isObject(record)) throw new Error(`${source}: expected a JSON object`);
  validateSha(record, source);
  if (record.schema !== 1) throw new Error(`${source}: expected AI-Run schema 1`);
  if (typeof record.model !== "string" || !record.model || /\s/.test(record.model)) {
    throw new Error(`${source}: model must be a nonempty name without whitespace`);
  }
  if (record.effort !== null && !EFFORTS.includes(record.effort)) {
    throw new Error(`${source}: effort must be l, m, h, x, or null`);
  }
}

function validateEvaluation(record, source) {
  if (!isObject(record)) throw new Error(`${source}: expected a JSON object`);
  validateSha(record, source);
  if (typeof record.verdict !== "string" || !record.verdict.trim() || record.verdict !== record.verdict.trim()) {
    throw new Error(`${source}: verdict must be a nonempty string without surrounding whitespace`);
  }
  if (!Number.isFinite(record.difficulty) || record.difficulty < 0) {
    throw new Error(`${source}: difficulty must be a finite, nonnegative number`);
  }
  if (typeof record.again !== "boolean") {
    throw new Error(`${source}: again must be a boolean`);
  }
}

export function loadMetrics({
  contractsPath = resolveContractsPath(),
  cwd = process.cwd(),
  ref,
  evalsPath,
  run = execFileSync,
  readFile = readFileSync,
} = {}) {
  if (typeof contractsPath !== "string" || !contractsPath.trim()) {
    throw new Error("Set GITHUB_AGENT_CONTRACTS to the sibling contracts clone or pass contractsPath.");
  }
  const revision = ref === undefined ? "HEAD" : ref;
  if (typeof revision !== "string" || !revision.trim() || revision.startsWith("-")) {
    throw new Error("ref must be a nonempty Git revision or range, not an option.");
  }

  const evaluations = [];
  if (evalsPath !== undefined && evalsPath !== null) {
    if (typeof evalsPath !== "string" || !evalsPath.trim()) {
      throw new Error("evalsPath must be a nonempty local file path.");
    }
    const file = resolve(cwd, evalsPath);
    let text;
    try {
      text = readFile(file, "utf8");
    } catch (error) {
      throw new Error(`Could not read AI-Eval file ${file}: ${failureDetail(error)}`, { cause: error });
    }
    evaluations.push(...parseJsonl(text, file, validateEvaluation, true));
  }

  const exporter = resolve(contractsPath, "scripts", "export-agent-metrics.mjs");
  let output;
  try {
    output = run(process.execPath, [exporter, "--ref", revision], {
      cwd,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      maxBuffer: 16 * 1024 * 1024,
    });
  } catch (error) {
    throw new Error(`Metrics exporter failed (${exporter}): ${failureDetail(error)}`, { cause: error });
  }
  const exported = parseJsonl(output, EXPORT_SOURCE, validateRun, true);
  const local = loadLearning({ cwd, readFile });
  return joinLearning(exported, local.runs, [...local.evaluations, ...evaluations], ref === undefined);
}

export function summarizeMetrics(records) {
  if (!Array.isArray(records)) throw new TypeError("Expected an array of joined metrics records.");
  const groups = new Map();
  for (const record of records) {
    const model = record.model ?? null;
    const effort = record.effort ?? null;
    const key = JSON.stringify([model, effort]);
    let group = groups.get(key);
    if (!group) {
      group = { model, effort, runs: 0, evaluated: 0 };
      groups.set(key, group);
    }
    group.runs += 1;
    if (record.evaluation != null) group.evaluated += 1;
  }
  return [...groups.values()].sort((left, right) =>
    (left.model ?? "").localeCompare(right.model ?? "") ||
    [ ...EFFORTS, null ].indexOf(left.effort) - [ ...EFFORTS, null ].indexOf(right.effort));
}

export function formatMetrics(groups) {
  if (!Array.isArray(groups)) throw new TypeError("Expected an array of metrics groups.");
  if (groups.length === 0) return "No AI-Run records found.\n";
  const rows = [
    ["MODEL", "EFFORT", "RUNS", "EVALS"],
    ...groups.map(({ model, effort, runs, evaluated }) =>
      [model ?? "-", effort ?? "-", String(runs), String(evaluated)]),
  ];
  const widths = rows[0].map((_, index) =>
    rows.reduce((width, row) => Math.max(width, row[index].length), 0));
  return `${rows.map((row) =>
    row.map((cell, index) => index === row.length - 1 ? cell : cell.padEnd(widths[index]))
      .join("  ")).join("\n")}\n`;
}
