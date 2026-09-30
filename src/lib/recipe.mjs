import { lstatSync, readFileSync } from "node:fs";
import { TextDecoder } from "node:util";

const MAX_RECIPE_BYTES = 65_536;
const SEAT_IDS = ["planner", "coder", "reviewer"];
const WORKERS = ["copilot", "hermes", "builtin"];
const BUILTIN_SEQUENCES = {
  planner: ["read_ask", "plan", "write_task"],
  coder: ["load_context", "implement", "run_tests", "summarize"],
  reviewer: ["read_diff", "check_acceptance", "write_review"],
};

export class RecipeError extends Error {}

function invalidRecipe() {
  throw new RecipeError("Invalid or unsupported recipe; expected the strict version 1 format in docs/SEATS.md.");
}

export function parseRecipe(source) {
  if (typeof source !== "string" || Buffer.byteLength(source, "utf8") > MAX_RECIPE_BYTES) invalidRecipe();

  const rootKeys = new Set();
  const seatIds = new Set();
  const seats = [];
  let ask;
  let inSeats = false;
  let fields;

  const finishSeat = () => {
    if (!fields) return;
    if (
      !Object.hasOwn(fields, "id") || !Object.hasOwn(fields, "principal") ||
      !Object.hasOwn(fields, "worker") || !SEAT_IDS.includes(fields.id) ||
      fields.principal !== (fields.id === "reviewer" ? "reviewer" : "coder") ||
      !WORKERS.includes(fields.worker) || (fields.id === "reviewer" && fields.worker !== "builtin") ||
      seatIds.has(fields.id) ||
      (fields.id === "planner" && seatIds.size > 0) ||
      (fields.id === "coder" && seatIds.has("reviewer")) ||
      (fields.id === "reviewer" && !seatIds.has("coder"))
    ) invalidRecipe();
    const builtin = fields.worker === "builtin";
    if (builtin !== Object.hasOwn(fields, "sequence") ||
        (builtin && fields.sequence !== `[${BUILTIN_SEQUENCES[fields.id].join(", ")}]`)) invalidRecipe();
    seats.push(Object.freeze({
      id: fields.id,
      principal: fields.principal,
      worker: fields.worker,
      ...(builtin ? { sequence: Object.freeze([...BUILTIN_SEQUENCES[fields.id]]) } : {}),
    }));
    seatIds.add(fields.id);
    fields = undefined;
  };

  const addField = (key, value) => {
    if (Object.hasOwn(fields, key) || !value) invalidRecipe();
    fields[key] = value;
  };

  for (const original of source.replace(/\r\n/g, "\n").split("\n")) {
    if (/[\x00-\x1f\x7f]/.test(original)) invalidRecipe();
    const line = original.replace(/(?:^|\s+)#.*$/, "").trimEnd();
    if (!line) continue;

    const root = /^(version|ask|seats):(?: (.*))?$/.exec(line);
    if (root) {
      finishSeat();
      if (rootKeys.has(root[1])) invalidRecipe();
      rootKeys.add(root[1]);
      inSeats = root[1] === "seats";
      if (inSeats) {
        if (root[2] !== undefined) invalidRecipe();
      } else if (root[1] === "version") {
        if (root[2] !== "1") invalidRecipe();
      } else {
        const issue = /^issue:([1-9][0-9]*)$/.exec(root[2] ?? "");
        const draft = /^local:[A-Za-z0-9_-]{1,64}$/.test(root[2] ?? "");
        if (!draft && (!issue || !Number.isSafeInteger(Number(issue[1])))) invalidRecipe();
        ask = root[2];
      }
      continue;
    }

    const item = /^  - (id|principal|worker|sequence): (.*)$/.exec(line);
    if (item) {
      if (!inSeats) invalidRecipe();
      finishSeat();
      fields = {};
      addField(item[1], item[2]);
      continue;
    }

    const field = /^    (id|principal|worker|sequence): (.*)$/.exec(line);
    if (!field || !inSeats || !fields) invalidRecipe();
    addField(field[1], field[2]);
  }

  finishSeat();
  if (rootKeys.size !== 3 || !seatIds.has("coder")) invalidRecipe();
  return Object.freeze({ version: 1, ask, seats: Object.freeze(seats) });
}

export function validateRecipe(path) {
  if (typeof path !== "string" || !path) throw new RecipeError("Recipe path must be a nonempty string.");

  let status;
  try {
    status = lstatSync(path);
  } catch (cause) {
    throw new RecipeError("Cannot read recipe file.", { cause });
  }
  if (!status.isFile() || status.isSymbolicLink() || status.size > MAX_RECIPE_BYTES) {
    throw new RecipeError("Recipe must be a regular file, not a symlink, and at most 64 KiB.");
  }

  let contents;
  try {
    contents = readFileSync(path);
  } catch (cause) {
    throw new RecipeError("Cannot read recipe file.", { cause });
  }
  let source;
  try {
    source = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(contents);
  } catch (cause) {
    throw new RecipeError("Recipe file must be UTF-8.", { cause });
  }
  return parseRecipe(source);
}
