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

function run(args, env = process.env, cliPath = cli, input, cwd = root) {
  return spawnSync(process.execPath, [cliPath, ...args], {
    cwd,
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
  assert.match(result.stdout, /^  roster\s+Open the interactive shell in a TTY$/m);
  assert.match(result.stdout, /roster\s+run\s+--issue/);
  assert.match(result.stdout, /roster\s+doctor/);
  assert.match(result.stdout, /roster\s+init/);
  assert.match(result.stdout, /roster\s+onboard/);
  assert.match(result.stdout, /roster\s+status\s+\[--issue N\]\s+\[--offline\]/);
  assert.match(result.stdout, /roster\s+recipe\s+validate/);
  assert.match(result.stdout, /roster\s+stats/);
  assert.match(result.stdout, /roster\s+vault\s+set\s+NAME/);
  assert.match(result.stdout, /roster\s+vault\s+list/);
  assert.match(result.stdout, /roster\s+vault\s+get\s+NAME/);
  assert.match(result.stdout, /roster\s+eval/);
  assert.match(result.stdout, /roster\s+recommend\s+--task-class/);
  assert.match(result.stdout, /roster\s+ask/);
  assert.match(result.stdout, /^  roster run --issue N \[--runtime builtin\] \[--seats planner,coder,reviewer\] \[--auto-model\] \[--publish\] \[--skip-review\]$/m);
  assert.match(result.stdout, /--auto-model/);
  assert.match(result.stdout, /^  roster prepare --issue N$/m);
});

test("the package exposes the roster bin", () => {
  const packageJson = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  assert.deepEqual(packageJson.bin, { roster: "src/cli.mjs" });
});

test("doctor checks prerequisites without network calls or leaking App env values", (t) => {
  const cwd = mkdtempSync(join(tmpdir(), 'roster-cli-doctor-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  mkdirSync(join(cwd, '.roster'));
  mkdirSync(join(cwd, '.github', 'workflows'), { recursive: true });
  writeFileSync(join(cwd, '.roster', 'config.yml'),
    readFileSync(join(root, 'roster.config.example.yml'), 'utf8').replace('model: ""', 'model: served-model'));
  writeFileSync(join(cwd, 'agent-policy.yml'), 'fixture policy marker\n');
  writeFileSync(join(cwd, '.github', 'workflows', 'check-agent-trailers.yml'), 'fixture workflow marker\n');
  const keyPath = join(cwd, 'test-only-path-marker.pem');
  writeFileSync(keyPath, 'fixture-only key marker\n');
  const ready = run(["doctor"], { ...process.env,
    GITHUB_APP_ID: 'test-only-app-marker', GITHUB_APP_PRIVATE_KEY_PATH: keyPath }, cli, undefined, cwd);
  assert.ifError(ready.error);
  assert.equal(ready.status, 0, ready.stderr);
  assert.equal((ready.stdout.match(/^OK /gm) ?? []).length, 6);
  assert.ok(!ready.stdout.includes('test-only-app-marker'));
  assert.ok(!ready.stdout.includes('test-only-path-marker'));
  const missing = run(["doctor"], { ...process.env,
    GITHUB_APP_ID: '', GITHUB_APP_PRIVATE_KEY_PATH: '' }, cli, undefined, cwd);
  assert.equal(missing.status, 1);
  assert.match(missing.stdout, /^FAIL GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH present$/m);
});
test("ask CLI creates an offline draft with a create command when gh is missing", () => {
  const directory = mkdtempSync(join(tmpdir(), "roster-ask-cli-"));
  try {
    const fixtureRoot = join(directory, "roster");
    cpSync(join(root, "src"), join(fixtureRoot, "src"), { recursive: true });
    cpSync(join(root, "templates"), join(fixtureRoot, "templates"), { recursive: true });
    cpSync(join(root, "roster.config.example.yml"), join(fixtureRoot, "roster.config.example.yml"));
    const offlineEnv = Object.fromEntries(Object.entries(process.env)
      .filter(([name]) => name.toLowerCase() !== "path"));
    offlineEnv.PATH = "";
    const result = run(["ask", "Add a Status section to README.md."], offlineEnv,
      join(fixtureRoot, "src", "cli.mjs"));
    assert.ifError(result.error);
    assert.equal(result.status, 0, result.stderr);
    const [ask, recipe, task, command] = result.stdout.trim().split(/\r?\n/);
    assert.match(ask, /^Ask: .*\.roster[\\/]asks[\\/].+\.md$/);
    assert.match(recipe, /^RECIPE: .*RECIPE\.yml$/);
    assert.match(task, /^TASK: .*TASK\.md$/);
    assert.match(readFileSync(recipe.slice("RECIPE: ".length), "utf8"), /worker: builtin/);
    assert.match(readFileSync(task.slice("TASK: ".length), "utf8"), /node --test exits 0/);
    assert.match(command, /^Next: gh issue create --title .+ --body-file .+$/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("builtin CLI defaults to paired seats, rejects unsupported selections, and refuses stub publication", () => {
  const invalid = run(["run", "--issue", "42", "--seat", "merger", "--runtime", "builtin"]);
  assert.notEqual(invalid.status, 0);
  assert.match(invalid.stderr, /--seats planner,coder,reviewer/);
  const invalidOrder = run(["run", "--issue", "42", "--runtime", "builtin",
    "--seats", "coder,planner"]);
  assert.notEqual(invalidOrder.status, 0);
  assert.match(invalidOrder.stderr, /--seats planner,coder,reviewer/);
  const noIssue = run(["run", "--issue", "n/a", "--runtime", "builtin",
    "--seats", "planner,coder"]);
  assert.notEqual(noIssue.status, 0);
  assert.match(noIssue.stderr, /Issue number must be a positive safe integer/);
  const explicitReviewer = run(["run", "--issue", "n/a", "--seats", "planner,coder,reviewer"]);
  assert.match(explicitReviewer.stderr, /Issue number must be a positive safe integer/);
  const skipReviewer = run(["run", "--issue", "n/a", "--skip-review"]);
  assert.match(skipReviewer.stderr, /Issue number must be a positive safe integer/);
  const invalidSkip = run(["run", "--seat", "coder", "--runtime", "builtin", "--skip-review"]);
  assert.match(invalidSkip.stderr, /--skip-review/);
  const autoIssue = run(["run", "--issue", "n/a", "--runtime", "builtin", "--auto-model"]);
  assert.match(autoIssue.stderr, /Issue number must be a positive safe integer/);
  const defaultSeats = run(["run", "--issue", "n/a", "--runtime", "builtin"]);
  assert.match(defaultSeats.stderr, /Issue number must be a positive safe integer/);
  const bareRun = run(["run", "--issue", "n/a"]);
  assert.match(bareRun.stderr, /Issue number must be a positive safe integer/);
  const legacySeat = run(["run", "--issue", "n/a", "--seat", "coder", "--runtime", "builtin"]);
  assert.match(legacySeat.stderr, /Issue number must be a positive safe integer/);
  const noPublish = run(["run", "--issue", "42", "--runtime", "builtin", "--publish"],
    { ...process.env, AI_MODEL: "", ROSTER_MODEL: "" });
  assert.notEqual(noPublish.status, 0);
  assert.match(noPublish.stderr, /set model/);
});

test("bare run uses the builtin planner and coder while prepare keeps manual handoff explicit", (t) => {
  const directory = mkdtempSync(join(tmpdir(), "roster-default-run-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const fixtureRoot = join(directory, "roster");
  cpSync(join(root, "src"), join(fixtureRoot, "src"), { recursive: true });
  const example = readFileSync(join(root, "roster.config.example.yml"), "utf8");
  writeFileSync(join(fixtureRoot, "roster.config.example.yml"),
    example.replace('profile: ""', "profile: vllm-local"));
  const fixtureCli = join(fixtureRoot, "src", "cli.mjs");
  const env = { ...process.env, AI_MODEL: "", ROSTER_MODEL: "" };
  const bareRun = run(["run", "--issue", "42"], env, fixtureCli, undefined, fixtureRoot);
  assert.ifError(bareRun.error);
  assert.equal(bareRun.status, 1);
  assert.match(bareRun.stderr, /set model/);
  assert.equal(bareRun.stdout, "");
  const explicitBuiltin = run(["run", "--issue", "42", "--runtime", "builtin"],
    env, fixtureCli, undefined, fixtureRoot);
  assert.match(explicitBuiltin.stderr, /set model/);
  const manual = run(["prepare", "--issue", "0"], env, fixtureCli, undefined, fixtureRoot);
  assert.match(manual.stderr, /Issue number must be a positive safe integer/);
  const invalid = run(["run", "--issue", "42", "--runtime", "prepare"],
    env, fixtureCli, undefined, fixtureRoot);
  assert.match(invalid.stderr, /Use roster run --issue N/);
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

test("status rejects invalid and repeated issue selectors without querying GitHub", () => {
  const invalid = run(["status", "--issue", "0", "--offline"]);
  assert.notEqual(invalid.status, 0);
  assert.match(invalid.stderr, /positive safe issue number/);
  const repeated = run(["status", "--issue", "42", "--issue", "43", "--offline"]);
  assert.notEqual(repeated.status, 0);
  assert.match(repeated.stderr, /Use roster status/);
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
  const fetched = run(["vault", "get", "ROSTER_TEST_TOKEN"], env);
  assert.ifError(fetched.error);
  assert.equal(fetched.status, 0, fetched.stderr);
  assert.equal(fetched.stdout, secret);
  assert.equal(fetched.stderr, "");
  const missing = run(["vault", "get", "MISSING"], env);
  assert.notEqual(missing.status, 0);
  assert.equal(missing.stdout, "");
  assert.match(missing.stderr, /No secret stored/);

  for (const [args, input] of [
    [["vault", "set", "TOKEN", secret], ""],
    [["vault", "set", "TOKEN"], "\n"],
    [["vault", "set", "../TOKEN"], secret],
    [["vault", "set", "GITHUB_APP_PRIVATE_KEY_PATH"], secret],
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
