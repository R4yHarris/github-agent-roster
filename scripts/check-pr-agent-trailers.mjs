#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { checkMessage } from "./check-agent-trailers.mjs";

function git(args, cwd) {
  return execFileSync("git", args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
}

export function checkPullRequest({ baseSha, headSha, requireAll = false, cwd = process.cwd() }) {
  for (const [name, value] of Object.entries({ baseSha, headSha })) {
    if (typeof value !== "string" || !/^[0-9a-f]{40}$/i.test(value)) {
      throw new Error(`${name} must be a full GitHub commit SHA`);
    }
  }
  if (typeof requireAll !== "boolean") {
    throw new Error("requireAll must be a boolean");
  }
  if (git(["rev-parse", "--is-shallow-repository"], cwd).trim() !== "false") {
    throw new Error("Full history is required; use actions/checkout with fetch-depth: 0");
  }
  const commits = git(["rev-list", "--reverse", `${baseSha}..${headSha}`, "--"], cwd).trim();
  if (!commits) {
    throw new Error("No commits found in the PR range");
  }
  return commits.split("\n").map((sha) => {
    const message = git(["show", "--no-patch", "--format=%B", sha, "--"], cwd);
    const authorEmail = git(["show", "--no-patch", "--format=%ae", sha, "--"], cwd).trim();
    return { sha, ...checkMessage(message, requireAll, authorEmail) };
  });
}

function main() {
  try {
    const requireAll = process.env.REQUIRE_ON_ALL_COMMITS ?? "false";
    if (requireAll !== "true" && requireAll !== "false") {
      throw new Error("require-on-all-commits must be true or false");
    }
    const results = checkPullRequest({
      baseSha: process.env.BASE_SHA,
      headSha: process.env.HEAD_SHA,
      requireAll: requireAll === "true",
    });
    for (const result of results) {
      const status = result.ok ? "OK" : `missing trailers: ${result.missing.join(", ")}`;
      process.stdout.write(`${result.sha}: ${status}\n`);
    }
    process.exitCode = results.every((result) => result.ok) ? 0 : 1;
  } catch (error) {
    process.stderr.write(`PR trailer check failed: ${error.message}\n`);
    process.exitCode = 2;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main();
}