import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { assertSeatCovers, parseRecipe, RecipeError, validateRecipe } from "../src/lib/recipe.mjs";

const example = `version: 1
ask: issue:42
seats:
  - id: planner
    principal: coder
    worker: copilot
  - id: coder
    principal: coder
    worker: hermes
`;

function temporaryDirectory(context) {
  const directory = mkdtempSync(join(tmpdir(), "roster-recipe-"));
  context.after(() => rmSync(directory, { recursive: true, force: true }));
  return directory;
}

test("parses the version 1 planner/coder recipe without granting capabilities", () => {
  const recipe = parseRecipe(example);
  assert.deepEqual(recipe, {
    version: 1,
    ask: "issue:42",
    seats: [
      { id: "planner", principal: "coder", worker: "copilot" },
      { id: "coder", principal: "coder", worker: "hermes" },
    ],
  });
  assert.equal(Object.isFrozen(recipe), true);
  assert.equal(Object.isFrozen(recipe.seats), true);
  assert.equal(Object.isFrozen(recipe.seats[0]), true);
  assert.throws(() => { recipe.seats[0].principal = "merger"; }, TypeError);
});

test("accepts builtin planner, coder, then reviewer while retaining legacy recipes", () => {
  const source = readFileSync(new URL("../templates/sdlc/RECIPE.yml", import.meta.url), "utf8")
    .replace("issue:N", "issue:42");
  assert.deepEqual(parseRecipe(source).seats, [
    { id: "planner", principal: "coder", worker: "builtin",
      sequence: ["read_ask", "plan", "write_task"] },
    { id: "coder", principal: "coder", worker: "builtin",
      sequence: ["load_context", "implement", "run_tests", "summarize"] },
    { id: "reviewer", principal: "reviewer", worker: "builtin",
      sequence: ["read_diff", "check_acceptance", "write_review"] },
  ]);
  assert.throws(() => parseRecipe(source.replace("read_ask, plan, write_task", "write_file")),
    RecipeError);
});

test("supports comments, CRLF, unordered keys, and a single coder seat", () => {
  const source = `# Human-written recipe
seats: # Ordered steps
  - worker: copilot
    id: planner
    principal: coder
  - principal: coder
    worker: hermes
    id: coder
ask: issue:42
version: 1
`;
  assert.deepEqual(parseRecipe(source.replace(/\n/g, "\r\n")), parseRecipe(example));
  assert.deepEqual(parseRecipe("version: 1\nask: issue:1\nseats:\n  - id: coder\n    principal: coder\n    worker: hermes\n").seats, [
    { id: "coder", principal: "coder", worker: "hermes" },
  ]);
});

test("rejects missing, duplicate, or extra root and seat keys", () => {
  for (const [name, source] of [
    ["empty", ""],
    ["missing version", example.replace("version: 1\n", "")],
    ["missing ask", example.replace("ask: issue:42\n", "")],
    ["missing seats", example.slice(0, example.indexOf("seats:"))],
    ["missing id", example.replace("  - id: planner\n", "  - worker: copilot\n").replace("    worker: copilot\n", "")],
    ["missing principal", example.replace("    principal: coder\n", "")],
    ["missing worker", example.replace("    worker: hermes\n", "")],
    ["duplicate version", `version: 1\n${example}`],
    ["duplicate ask", `${example}ask: issue:43\n`],
    ["duplicate seats", `${example}seats:\n  - id: coder\n    principal: coder\n    worker: hermes\n`],
    ["duplicate seat key", example.replace("    worker: hermes\n", "    worker: hermes\n    worker: copilot\n")],
    ["duplicate seat id", `${example}  - id: coder\n    principal: coder\n    worker: hermes\n`],
    ["unknown root key", `${example}default: allow\n`],
    ["unknown seat key", example.replace("    worker: hermes\n", "    worker: hermes\n    merge: true\n")],
    ["inline empty seats", example.replace("seats:", "seats: []")],
  ]) {
    assert.throws(() => parseRecipe(source), RecipeError, name);
  }
});

test("denies unsupported seat roles, worker names, and workflow shapes", () => {
  for (const [name, source] of [
    ["deploy seat", example.replace("id: coder", "id: deploy")],
    ["review seat", example.replace("id: coder", "id: reviewer")],
    ["reviewer before coder", example.replace("  - id: coder", "  - id: reviewer")],
    ["merger principal", example.replace("    principal: coder", "    principal: merger")],
    ["deploy principal", example.replace("    principal: coder", "    principal: deploy")],
    ["planner principal", example.replace("    principal: coder", "    principal: planner")],
    ["unknown worker", example.replace("worker: hermes", "worker: unknown")],
    ["merge allowance", example.replace("    worker: hermes\n", "    worker: hermes\n    allow: [merge]\n")],
    ["deploy permission", example.replace("    worker: hermes\n", "    worker: hermes\n    deploy: true\n")],
    ["planner only", example.slice(0, example.indexOf("  - id: coder"))],
    ["no seats", "version: 1\nask: issue:42\nseats:\n"],
    ["coder before planner", example.replace(
      /  - id: planner[\s\S]+/,
      "  - id: coder\n    principal: coder\n    worker: hermes\n  - id: planner\n    principal: coder\n    worker: copilot\n",
    )],
  ]) {
    assert.throws(() => parseRecipe(source), RecipeError, name);
  }
  const builtin = readFileSync(new URL("../templates/sdlc/RECIPE.yml", import.meta.url), "utf8");
  assert.throws(() => parseRecipe(builtin.replace('principal: reviewer', 'principal: coder')),
    RecipeError);
  assert.throws(() => parseRecipe(builtin.replace('read_diff, check_acceptance, write_review', 'write_file')),
    RecipeError);
  assert.throws(() => parseRecipe(builtin.replace('worker: builtin\n    sequence: [read_diff',
    'worker: hermes\n    sequence: [read_diff')), RecipeError);
});

test("rejects malformed issue references and unsupported YAML", () => {
  for (const [name, source] of [
    ...["issue:0", "issue:01", "issue:-1", "issue:1.0", "issue:9007199254740992", "https://github.com/x/y/issues/1"]
      .map((ask) => [ask, example.replace("issue:42", ask)]),
    ["wrong version", example.replace("version: 1", "version: 2")],
    ["quoted version", example.replace("version: 1", 'version: "1"')],
    ["quoted worker", example.replace("worker: hermes", 'worker: "hermes"')],
    ["anchor", example.replace("worker: hermes", "worker: &hermes hermes")],
    ["alias", example.replace("worker: hermes", "worker: *hermes")],
    ["tag", example.replace("worker: hermes", "worker: !worker hermes")],
    ["document marker", `---\n${example}`],
    ["tab indentation", example.replace("  - id: coder", "\t- id: coder")],
    ["wrong indentation", example.replace("    worker: hermes", "   worker: hermes")],
    ["lone carriage return", example.replace("version: 1\n", "version: 1\r")],
    ["nul comment", `# comment\0\n${example}`],
    ["inline mapping", example.replace("  - id: coder\n    principal: coder\n    worker: hermes", "  - {id: coder, principal: coder, worker: hermes}")],
  ]) {
    assert.throws(() => parseRecipe(source), RecipeError, name);
  }
  assert.throws(() => parseRecipe(" ".repeat(65_537)), RecipeError);
  assert.throws(() => parseRecipe(undefined), RecipeError);
});

test("validates files without falling back on missing, malformed, or oversized input", (context) => {
  const directory = temporaryDirectory(context);
  const path = join(directory, "recipe.yml");
  writeFileSync(path, example);
  assert.deepEqual(validateRecipe(path), parseRecipe(example));

  writeFileSync(path, "version: 1\nask: issue:42\nseats: []\n");
  assert.throws(() => validateRecipe(path), RecipeError);
  writeFileSync(path, "x".repeat(65_537));
  assert.throws(() => validateRecipe(path), /at most 64 KiB/);
  writeFileSync(path, Buffer.from([0xff]));
  assert.throws(() => validateRecipe(path), /must be UTF-8/);
  assert.throws(() => validateRecipe(directory), /regular file/);
  assert.throws(() => validateRecipe(join(directory, "missing.yml")), /Cannot read recipe file/);
  assert.throws(() => validateRecipe(""), /Recipe path/);
});

test("refuses a symlink even when its target is a valid recipe", (context) => {
  const directory = temporaryDirectory(context);
  const target = join(directory, "recipe.yml");
  const link = join(directory, "link.yml");
  writeFileSync(target, example);
  try {
    symlinkSync(target, link, "file");
  } catch (error) {
    if (["EPERM", "EACCES", "ENOTSUP"].includes(error.code)) {
      context.skip("Creating symlinks is unavailable on this system.");
      return;
    }
    throw error;
  }
  assert.throws(() => validateRecipe(link), /not a symlink/);
});


test("parses and freezes optional capabilities without changing omitted fields", () => {
  const source = example + "    skills: [implement-task, run-tests]\n    tools: [read_file, edit_file]\n    max_difficulty: 3\n";
  const seat = parseRecipe(source).seats[1];
  assert.deepEqual(seat.skills, ["implement-task", "run-tests"]);
  assert.deepEqual(seat.tools, ["read_file", "edit_file"]);
  assert.equal(seat.max_difficulty, 3);
  assert.ok(Object.isFrozen(seat.skills));
  assert.ok(Object.isFrozen(seat.tools));
  assert.equal(Object.hasOwn(parseRecipe(example).seats[1], "tools"), false);
  for (const max of [1, 5]) {
    assert.equal(parseRecipe(example + `    max_difficulty: ${max}\n`).seats[1].max_difficulty, max);
  }
  assert.deepEqual(parseRecipe(example + "    tools: []\n    skills: []\n").seats[1].tools, []);
  assert.deepEqual(parseRecipe(example.replace("  - id: coder", "  - tools: []\n    id: coder")).seats[1].tools, []);
});

test("rejects unknown, duplicate, malformed capabilities and out-of-range difficulty", () => {
  for (const field of [
    "skills: [unknown]", "skills: [implement-task, implement-task]", "skills: implement-task",
    'skills: ["docs"]', "skills: [docs,]", "tools: [merge]", "tools: [shell]",
    "tools: [read_file, read_file]", "tools: {}", "max_difficulty: 0", "max_difficulty: 6",
    "max_difficulty: 2.5", "max_difficulty: 03", 'max_difficulty: "3"', "max_difficulty: high",
    "tools: []\n    tools: [read_file]", "skills: []\n    skills: [docs]",
    "max_difficulty: 3\n    max_difficulty: 4",
  ]) assert.throws(() => parseRecipe(example + `    ${field}\n`), RecipeError, field);
});

test("coverage is pure, checks all skills and exact difficulty ceilings, and keeps legacy unbounded", () => {
  const seat = parseRecipe(example + "    skills: [implement-task, run-tests]\n    max_difficulty: 3\n").seats[1];
  const task = Object.freeze({ difficulty: 3, skills: Object.freeze(["implement-task", "run-tests"]) });
  assert.doesNotThrow(() => assertSeatCovers(seat, task));
  assert.throws(() => assertSeatCovers(seat, { ...task, difficulty: 4 }), /max_difficulty 3/);
  assert.throws(() => assertSeatCovers(seat, { ...task, skills: ["implement-task", "docs"] }), /docs/);
  assert.throws(() => assertSeatCovers(seat, { skills: [] }), /Task difficulty/);
  assert.throws(() => assertSeatCovers(seat, { difficulty: 2 }), /Task skills/);
  const empty = parseRecipe(example + "    skills: []\n").seats[1];
  assert.doesNotThrow(() => assertSeatCovers(empty, { skills: [] }));
  assert.throws(() => assertSeatCovers(empty, { skills: ["docs"] }), /docs/);
  assert.doesNotThrow(() => assertSeatCovers(parseRecipe(example).seats[1], {}));
});
