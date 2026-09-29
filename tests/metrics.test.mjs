import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { formatMetrics, loadMetrics, summarizeMetrics } from "../src/lib/metrics.mjs";

const metricsJsonl = readFileSync(new URL("./fixtures/metrics.jsonl", import.meta.url), "utf8");
const evalsJsonl = readFileSync(new URL("./fixtures/evals.jsonl", import.meta.url), "utf8");
const evalsPath = fileURLToPath(new URL("./fixtures/evals.jsonl", import.meta.url));
const contractsPath = resolve("sibling-contracts");
const sampleLine = metricsJsonl.split("\n")[0];
const sample = JSON.parse(sampleLine);
const sampleEval = JSON.parse(evalsJsonl.split("\n")[0]);
const jsonl = (record) => `${JSON.stringify(record)}\n`;

test("exports local AI-Run records, joins local evals by full SHA, and prints model/effort counts", () => {
  const cwd = resolve("local-git-repository");
  const records = loadMetrics({
    contractsPath,
    cwd,
    ref: "HEAD~3..HEAD",
    evalsPath,
    run(command, args, options) {
      assert.equal(command, process.execPath);
      assert.deepEqual(args, [
        resolve(contractsPath, "scripts", "export-agent-metrics.mjs"),
        "--ref",
        "HEAD~3..HEAD",
      ]);
      assert.equal(options.cwd, cwd);
      assert.equal(options.encoding, "utf8");
      assert.deepEqual(options.stdio, ["ignore", "pipe", "pipe"]);
      assert.equal(options.maxBuffer, 16 * 1024 * 1024);
      return metricsJsonl;
    },
  });

  assert.equal(records.length, 4);
  assert.deepEqual(records.map((record) => record.evaluation?.verdict ?? null),
    [null, null, "accept", "revise"]);
  assert.deepEqual(records[3].evaluation, JSON.parse(evalsJsonl.split("\n")[1]));
  const groups = summarizeMetrics(records);
  const unknown = { task_class: null, n: 0, accepted: 0, acceptRate: null,
    medianMinutes: null, medianDifficulty: null, estimate_min: null };
  assert.deepEqual(groups, [
    { ...unknown, model: "model-a", effort: "l", runs: 2, evaluated: 2,
      n: 1, accepted: 1, acceptRate: 1, medianDifficulty: 3 },
    { ...unknown, model: "model-a", effort: "h", runs: 1, evaluated: 0 },
    { ...unknown, model: "model-b", effort: null, runs: 1, evaluated: 0 },
  ]);
  assert.deepEqual(formatMetrics(groups).trim().split('\n').map((line) => line.split(/\s+/)), [
    ["MODEL", "EFFORT", "RUNS", "EVALS", "TASK_CLASS", "N", "ACCEPT", "MEDIAN_MIN", "MEDIAN_DIFFICULTY"],
    ["model-a", "l", "2", "2", "-", "1", "100.0%", "-", "3"],
    ["model-a", "h", "1", "0", "-", "0", "-", "-", "-"],
    ["model-b", "-", "1", "0", "-", "0", "-", "-", "-"],
  ]);
});

test("does not read evals unless a path is provided", () => {
  const records = loadMetrics({
    contractsPath,
    run: () => metricsJsonl,
    readFile: () => { throw new Error("unexpected eval read"); },
  });
  assert.ok(records.every((record) => record.evaluation === null));
  assert.ok(summarizeMetrics(records).every((group) => group.evaluated === 0));
});

test("accepts empty export and SHA-256 commit IDs", () => {
  assert.deepEqual(loadMetrics({ contractsPath, run: () => "" }), []);
  assert.equal(formatMetrics(summarizeMetrics([])), "No AI-Run records found.\n");
  const sha = "f".repeat(64);
  assert.equal(loadMetrics({ contractsPath, run: () => jsonl({ ...sample, sha }) })[0].sha, sha);
});

test("reports exporter failures with their stderr and original cause", () => {
  const cause = Object.assign(new Error("exit code 1"), {
    stderr: "Invalid AI-Run in commit abc; no metrics were exported.\n",
  });
  assert.throws(() => loadMetrics({ contractsPath, run: () => { throw cause; } }), (error) => {
    assert.match(error.message, /Metrics exporter failed .*Invalid AI-Run in commit abc/);
    assert.equal(error.cause, cause);
    return true;
  });
});

test("reports missing eval files and invalid options instead of ignoring them", () => {
  const missing = fileURLToPath(new URL("./fixtures/missing-evals.jsonl", import.meta.url));
  assert.throws(() => loadMetrics({ contractsPath, evalsPath: missing, run: () => metricsJsonl }),
    /Could not read AI-Eval file .*missing-evals\.jsonl.*ENOENT/);
  assert.throws(() => loadMetrics({ contractsPath: "" }), /GITHUB_AGENT_CONTRACTS/);
  assert.throws(() => loadMetrics({ contractsPath, ref: "--help" }), /ref must be/);
  assert.throws(() => loadMetrics({ contractsPath, evalsPath: "" }), /evalsPath must be/);
});

const malformedRuns = [
  ["invalid JSON", `${sampleLine}\nnot-json\n`, /output:2: invalid JSON/],
  ["blank lines", `${sampleLine}\n\n`, /output:2: blank JSONL line/],
  ["non-object records", "[]\n", /output:1: expected a JSON object/],
  ["short SHAs", jsonl({ ...sample, sha: "abc" }), /output:1: sha must be a full/],
  ["unsupported schemas", jsonl({ ...sample, schema: 2 }), /output:1: expected AI-Run schema 1/],
  ["missing models", jsonl({ ...sample, model: null }), /output:1: model must be/],
  ["invalid efforts", jsonl({ ...sample, effort: "high" }), /output:1: effort must be/],
  ["duplicate SHAs", `${sampleLine}\n${jsonl({ ...sample, sha: sample.sha.toUpperCase() })}`,
    /output:2: duplicate sha/],
];

for (const [name, output, expected] of malformedRuns) {
  test(`rejects exported ${name}`, () => {
    assert.throws(() => loadMetrics({ contractsPath, run: () => output }), expected);
  });
}

const malformedEvals = [
  ["invalid JSON", `${jsonl(sampleEval)}not-json\n`, /evals\.jsonl:2: invalid JSON/],
  ["blank lines", `${jsonl(sampleEval)}\n`, /evals\.jsonl:2: blank JSONL line/],
  ["non-object records", "[]\n", /evals\.jsonl:1: expected a JSON object/],
  ["short SHAs", jsonl({ ...sampleEval, sha: "abc" }), /evals\.jsonl:1: sha must be a full/],
  ["blank verdicts", jsonl({ ...sampleEval, verdict: " " }), /evals\.jsonl:1: verdict must be/],
  ["non-numeric difficulty", jsonl({ ...sampleEval, difficulty: "3" }),
    /evals\.jsonl:1: difficulty must be/],
  ["negative difficulty", jsonl({ ...sampleEval, difficulty: -1 }),
    /evals\.jsonl:1: difficulty must be/],
  ["missing again", jsonl({ sha: sampleEval.sha, verdict: "accept", difficulty: 3 }),
    /evals\.jsonl:1: again must be/],
  ["duplicate SHAs", `${jsonl(sampleEval)}${jsonl({ ...sampleEval, sha: sampleEval.sha.toUpperCase() })}`,
    /evals\.jsonl:2: duplicate sha/],
];

for (const [name, text, expected] of malformedEvals) {
  test(`rejects eval ${name}`, () => {
    assert.throws(() => loadMetrics({
      contractsPath,
      evalsPath,
      readFile: () => text,
      run: () => metricsJsonl,
    }), expected);
  });
}
