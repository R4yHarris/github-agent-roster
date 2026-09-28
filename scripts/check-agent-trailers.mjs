#!/usr/bin/env node
/**
 * Validate agent provenance trailers on a git commit message.
 * Zero dependencies. Node 20+.
 */

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { parseAgentRun, parseTrailers } from "./parse-agent-run.mjs";

export { parseTrailers } from "./parse-agent-run.mjs";

const REQUIRED = ["AI-Agent", "AI-Model"];
const BOT_EMAIL = /^[^@\s]+\[bot\]@(?:users\.)?noreply\.github\.com$/i;

function usage(stream = process.stdout) {
  stream.write(`Usage:
  check-agent-trailers.mjs [--require] [--require-run] [--json] [--author-email EMAIL]
    (--message TEXT | --message-file PATH)
  check-agent-trailers.mjs --help

  --message TEXT       Read a commit message from an argument
  --message-file PATH  Read a UTF-8 commit message from a file
  --author-email EMAIL Also classify GitHub noreply bot authors as agents
  --require            Require trailers on every message
  --require-run        Require a valid AI-Run on agent-authored messages
  --json               Print the result as JSON
  --help, -h           Show this help

  Exit 0  OK
  Exit 1  missing required trailers on an agent-authored message
  Exit 2  usage error
`);
}

function parseArgs(argv) {
  const out = { require: false, requireRun: false, json: false, message: null, authorEmail: "" };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--help" || argument === "-h") {
      out.help = true;
    } else if (argument === "--require") {
      out.require = true;
    } else if (argument === "--require-run") {
      out.requireRun = true;
    } else if (argument === "--json") {
      out.json = true;
    } else if (["--message", "--message-file", "--author-email"].includes(argument)) {
      const value = argv[++index];
      if (value === undefined || value.startsWith("--") || value === "-h") {
        throw new Error(`${argument} needs a value`);
      }
      if (argument === "--author-email") {
        out.authorEmail = value;
      } else {
        if (out.message !== null) {
          throw new Error("choose exactly one of --message or --message-file");
        }
        out.message = argument === "--message-file" ? readFileSync(value, "utf8") : value;
      }
    } else {
      throw new Error(`unknown argument: ${argument}`);
    }
  }
  return out;
}

export function isAgentAuthored(message, requireAll = false, authorEmail = "") {
  return requireAll || String(message).includes("AI-Agent:") || BOT_EMAIL.test(authorEmail);
}

export function checkMessage(message, requireAll = false, authorEmail = "", requireRun = false) {
  const trailers = parseTrailers(message);
  const agent = isAgentAuthored(message, requireAll, authorEmail);
  const missing = [];
  if (agent) {
    for (const key of REQUIRED) {
      if (!trailers[key] || String(trailers[key]).trim() === "") missing.push(key);
    }
    if (requireRun) {
      try {
        if (!parseAgentRun(trailers["AI-Run"], trailers["AI-Model"])) missing.push("AI-Run");
      } catch {
        missing.push("AI-Run");
      }
    }
  }
  return {
    agentAuthored: agent,
    trailers,
    missing,
    ok: missing.length === 0,
  };
}

function main(argv) {
  let args;
  try {
    args = parseArgs(argv);
  } catch (err) {
    process.stderr.write(`${err.message}\n`);
    usage(process.stderr);
    process.exit(2);
  }
  if (args.help) {
    usage();
    process.exit(0);
  }
  if (args.message == null) {
    usage(process.stderr);
    process.exit(2);
  }
  const result = checkMessage(args.message, args.require, args.authorEmail, args.requireRun);
  if (args.json) {
    process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
  } else if (!result.ok) {
    const reason = result.missing.includes("AI-Run") ? "missing or invalid trailers" : "missing trailers";
    process.stderr.write(`${reason}: ${result.missing.join(", ")}\n`);
  }
  process.exit(result.ok ? 0 : 1);
}

const isMain =
  process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href;

if (isMain) {
  main(process.argv.slice(2));
}
