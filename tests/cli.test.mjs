import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { createFileVault } from "../src/vault/file.mjs";

const cli = fileURLToPath(new URL("../src/cli.mjs", import.meta.url));
const root = fileURLToPath(new URL("..", import.meta.url));

function run(args, env = process.env, cliPath = cli, input) {
  return spawnSync(process.execPath, [cliPath, ...args], {
    cwd: root,
    encoding: "utf8",
    env,
    input,
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
  assert.match(result.stdout, /roster\s+vault\s+set\s+NAME/);
  assert.match(result.stdout, /roster\s+vault\s+list/);
  assert.match(result.stdout, /roster\s+eval/);
  assert.match(result.stdout, /roster\s+recommend\s+--task-class/);
  assert.match(result.stdout, /roster\s+ask/);
  assert.match(result.stdout, /--seat coder --runtime builtin/);
});

test("ask CLI creates an offline draft with recipe and task paths", () => {
  const directory = mkdtempSync(join(tmpdir(), "roster-ask-cli-"));
  try {
    const fixtureRoot = join(directory, "roster");
    cpSync(join(root, "src"), join(fixtureRoot, "src"), { recursive: true });
    cpSync(join(root, "templates"), join(fixtureRoot, "templates"), { recursive: true });
    cpSync(join(root, "roster.config.example.yml"), join(fixtureRoot, "roster.config.example.yml"));
    const result = run(["ask", "Add a Status section to README.md."], process.env,
      join(fixtureRoot, "src", "cli.mjs"));
    assert.ifError(result.error);
    assert.equal(result.status, 0, result.stderr);
    const [ask, recipe, task] = result.stdout.trim().split(/\r?\n/);
    assert.match(ask, /^Ask: .*\.roster[\\/]asks[\\/].+\.md$/);
    assert.match(recipe, /^RECIPE: .*RECIPE\.yml$/);
    assert.match(task, /^TASK: .*TASK\.md$/);
    assert.match(readFileSync(recipe.slice("RECIPE: ".length), "utf8"), /worker: builtin/);
    assert.match(readFileSync(task.slice("TASK: ".length), "utf8"), /node --test exits 0/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("builtin CLI rejects unsupported seats and refuses stub publication without GitHub access", () => {
  const invalid = run(["run", "--issue", "42", "--seat", "merger", "--runtime", "builtin"]);
  assert.notEqual(invalid.status, 0);
  assert.match(invalid.stderr, /--seat coder --runtime builtin/);
  const noIssue = run(["run", "--issue", "n/a", "--seat", "coder", "--runtime", "builtin"]);
  assert.notEqual(noIssue.status, 0);
  assert.match(noIssue.stderr, /Issue number must be a positive safe integer/);
  const noPublish = run(["run", "--issue", "42", "--seat", "coder", "--runtime", "builtin", "--publish"]);
  assert.notEqual(noPublish.status, 0);
  assert.match(noPublish.stderr, /--publish requires an LLM endpoint/);
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

test("vault CLI accepts only stdin secrets and lists names without values", async (t) => {
  const home = mkdtempSync(join(tmpdir(), "roster-vault-cli-"));
  t.after(() => rmSync(home, { recursive: true, force: true }));
  const env = { ...process.env, HOME: home, USERPROFILE: home };
  const secret = "test-only-cli-secret with trailing space ";
  const stored = run(["vault", "set", "ROSTER_TEST_TOKEN"], env, cli, `${secret}\r\n`);
  assert.ifError(stored.error);
  assert.equal(stored.status, 0, stored.stderr);
  assert.equal(stored.stdout, "Stored secret ROSTER_TEST_TOKEN.\n");
  assert.equal(stored.stderr, "");
  const vault = createFileVault({ directory: join(home, ".roster", "vault") });
  assert.equal(await vault.get("ROSTER_TEST_TOKEN"), secret);
  const listed = run(["vault", "list"], env);
  assert.ifError(listed.error);
  assert.equal(listed.status, 0, listed.stderr);
  assert.equal(listed.stdout, "ROSTER_TEST_TOKEN\n");
  assert.equal(listed.stderr, "");

  for (const [args, input] of [
    [["vault", "set", "TOKEN", secret], ""],
    [["vault", "set", "TOKEN"], "\n"],
    [["vault", "set", "../TOKEN"], secret],
    [["vault", "get", "ROSTER_TEST_TOKEN"], ""],
  ]) {
    const result = run(args, env, cli, input);
    assert.ifError(result.error);
    assert.notEqual(result.status, 0);
    assert.equal(result.stdout, "");
    assert.ok(!result.stderr.includes(secret));
  }
});

test("vault CLI refuses a home directory inside a Git worktree", (t) => {
  const home = mkdtempSync(join(tmpdir(), "roster-vault-cli-git-"));
  t.after(() => rmSync(home, { recursive: true, force: true }));
  mkdirSync(join(home, ".git"));
  const result = run(["vault", "set", "TOKEN"], { ...process.env, HOME: home, USERPROFILE: home }, cli, "test-secret");
  assert.ifError(result.error);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /outside a Git worktree/);
  assert.equal(result.stdout, "");
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
