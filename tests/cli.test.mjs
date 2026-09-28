import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const cli = fileURLToPath(new URL("../src/cli.mjs", import.meta.url));
const root = fileURLToPath(new URL("..", import.meta.url));

function run(args, env = process.env, cliPath = cli) {
  return spawnSync(process.execPath, [cliPath, ...args], {
    cwd: root,
    encoding: "utf8",
    env,
    timeout: 10_000,
  });
}

test("help lists every prompt's command", () => {
  const result = run(["--help"]);
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /roster\s+run\s+--issue/);
  assert.match(result.stdout, /roster\s+recipe\s+validate/);
  assert.match(result.stdout, /roster\s+stats/);
});

test("recipe validation accepts the documented shape and rejects unknown keys", () => {
  const directory = mkdtempSync(join(tmpdir(), "roster-recipe-"));
  try {
    const path = join(directory, "recipe.yml");
    const recipe = `version: 1
ask: issue:42
seats:
  - id: planner
    principal: coder
    worker: copilot
  - id: coder
    principal: coder
    worker: hermes
`;
    writeFileSync(path, recipe);
    const valid = run(["recipe", "validate", path]);
    assert.ifError(valid.error);
    assert.equal(valid.status, 0, valid.stderr);

    writeFileSync(path, `${recipe}extra: ignored\n`);
    const invalid = run(["recipe", "validate", path]);
    assert.ifError(invalid.error);
    assert.notEqual(invalid.status, 0);
    assert.match(invalid.stderr, /invalid|unknown|unsupported|extra/i);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("run without a numeric issue fails before GitHub access", () => {
  const result = run(["run", "--issue", "not-an-issue"]);
  assert.ifError(result.error);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /issue/i);
});

test("stats prints grouped model and effort counts with optional local evals", () => {
  const directory = mkdtempSync(join(tmpdir(), "roster-contracts-"));
  try {
    const fixtureSource = join(directory, "roster", "src");
    cpSync(join(root, "src"), fixtureSource, { recursive: true });
    const scripts = join(directory, "scripts");
    mkdirSync(scripts);
    writeFileSync(join(scripts, "agent-pr.mjs"), "export {};\n");
    const output = readFileSync(new URL("./fixtures/metrics.jsonl", import.meta.url), "utf8");
    writeFileSync(join(scripts, "export-agent-metrics.mjs"),
      `process.stdout.write(${JSON.stringify(output)});\n`);
    const evals = fileURLToPath(new URL("./fixtures/evals.jsonl", import.meta.url));
    const result = run(["stats", "--evals", evals], {
      ...process.env,
      GITHUB_AGENT_CONTRACTS: directory,
    }, join(fixtureSource, "cli.mjs"));
    assert.ifError(result.error);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /MODEL\s+EFFORT\s+RUNS\s+EVALS/);
    assert.match(result.stdout, /model-a\s+l\s+2\s+2/);
    assert.match(result.stdout, /model-a\s+h\s+1\s+0/);
    assert.match(result.stdout, /model-b\s+-\s+1\s+0/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
