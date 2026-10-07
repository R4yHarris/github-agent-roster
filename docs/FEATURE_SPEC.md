# Roster: the software-delivery orchestration standard

Feature document for an agent harness that beats today's coding agents and crew frameworks by running a real software lifecycle. Agents are treated as named workers with skills, capacity, and a performance history. Work is an issue, a branch, a pull request, and a review. The board is GitHub. The identity is a GitHub App. The score is the next assignment.

This is the product intent for `github-agent-roster`, with `github-agent-contracts` as the identity, policy, and metrics dependency.

## 1. What "beat them" means

A winning harness is not the one with the most tools. It is the one a team can leave running.

It wins if a human can state an outcome, walk away, and come back to a reviewed change whose author, model, hardware, cost, difficulty, and defects are recorded. The next similar task is smaller, assigned to a worker that has earned it, and less likely to repeat the last defect.

Today's products fail that test in one of three ways.

- Coding agents (Claude Code, Codex, Cursor, Copilot, Devin, Jules, OpenHands, SWE-agent) are strong at one seat. They do not run a team, and they publish as the human unless a separate identity is bolted on.
- Crew frameworks (CrewAI, LangGraph, AutoGen / Microsoft Agent Framework, MetaGPT) are strong at roles and graphs. They do not own the repository, the ruleset, or the delivery record.
- Hermes kanban is the closest board. It is also the failure this roster exists to replace: cards stall, workers crash, claims expire, and there is no SDLC gate or model-performance ledger.

## 2. What the field actually has

Sources for the 2026 landscape: LangGraph, CrewAI, and AutoGen comparisons (Enterprise DNA, Oct 2026; saaro, Sep 2026; Presenc AI); Microsoft Agent Framework GA April 2026 and AutoGen maintenance since October 2025; OpenHands v1.24 MIT versus SWE-agent; Cloudroom comparison of Claude Code, Codex, Cursor, Devin, Jules, and OpenHands (Sep 2026); Hermes kanban worker-lane docs and dispatcher crash reports.

### Coding seats

| System | Take the best of | Leave behind |
| --- | --- | --- |
| Claude Code | Repo-scale search, long context, permissioned shell, subagents, hooks, CLAUDE.md / AGENTS.md | Single-vendor model, session memory that dies with the chat, human credentials if you let it push |
| OpenAI Codex | Terminal strength, cloud sandbox, isolated container per task | OpenAI-only, no team roster, no cross-model ledger |
| Cursor / Windsurf | In-editor agent, repo index, cloud agents, rules files | IDE lock-in, subscription metering, weak audit of who merged |
| GitHub Copilot agent | Issue-to-PR inside GitHub, Actions, draft PRs | Still a Copilot identity, weak model metadata (`AI-Model: unknown` unless the harness sets it), no fleet routing |
| Devin | Whole-ticket handoff, VM with shell, browser, and IDE | Closed, quota-bound, not a team process |
| Jules | GitHub-native Gemini agent, cloud VM | One model family |
| OpenHands | MIT, any model, self-hosted, issue to PR, parallel tasks, ACP so Claude Code and Codex can be workers | Runtime, not a delivery system; no human-style estimation loop |
| SWE-agent | YAML agent-computer interface, benchmark discipline | Research agent, not a roster |
| Aider / Continue / Cline | Git-native edits, local models, small diff discipline | One coder, no review seat, no policy |

### Orchestration frameworks

| System | Take the best of | Leave behind |
| --- | --- | --- |
| LangGraph | Explicit state, checkpoints, resume, interrupts, lowest orchestration tax in published comparisons | You still build the SDLC, the repo rules, and the eval store |
| CrewAI | Role and task metaphor, fast crew prototype, MCP and A2A in 1.10 | Role play is not a protected branch; memory is not a commit trailer |
| AutoGen / AG2 | Debate and user-proxy | Maintenance mode; chat history is not an audit log |
| Microsoft Agent Framework | YAML agents, graph checkpoints, MCP, A2A, OpenTelemetry, approvals | Azure-shaped; not a GitHub delivery loop |
| MetaGPT | Software-company SOP: PM, architect, engineer, QA | Research workflow, last meaningful commit early 2026, not repository law |
| OpenAI Swarm / Agents SDK | Handoffs, small API | Experimental or single-vendor |
| Hermes kanban | Profile lanes, claim, block, heartbeat, orchestrator that must not code | Spawn bugs, stranded ready-column, no required check, no model ledger |

### The missing layer

Nobody in that table is the system of record for all four of these at once:

1. Who did the work (App bot, not the human).
2. Which model, effort, context, and hardware did it.
3. Whether the change passed the same gates a human team would require.
4. Whether that worker should get a harder task next time.

Roster is that layer. Seats can be Copilot, Claude Code, Codex, a local vLLM coder, or a built-in seat. The orchestrator does not care which CLI typed the diff. It cares about the contract.

## 3. Principles

These are product law, not prompts.

- GitHub is the board. An issue is a task. A branch is a claim. A pull request is the handoff. A ruleset is the gate. A comment is the status.
- Default deny. An agent publishes only through an App token. Human credentials are not on the orchestration machine.
- Agents are workers, not magic. They have a seat, a skill list, a capacity, a defect history, and a manager.
- One writer per worktree. Parallel seats get isolated trees. Merge is a separate role.
- The transcript is not the record. The record is the trailer, the check, the review, and the eval row.
- Stop early. A plan that validates is enough. A named-file edit ends the draft. Docs do not run the world.
- Humans approve policy, workflows, and production. Everything else is delegable and revocable.
- Local hardware first. Cloud models are a routed, budgeted choice, not the default.

## 4. Lifecycle the harness must run

Map every ask onto a delivery path a good engineering manager already knows.
[SWE_LIFECYCLE.md](SWE_LIFECYCLE.md) breaks these steps into seat turns, artifacts and gates.

1. Intake. Classify the ask: question, slice, story, epic, incident. Refuse unbounded work.
2. Estimate. Difficulty, expected minutes, files, risk, test plan. Record the estimate before coding.
3. Split. Epics become stories. Stories become slices with one acceptance check and a file allow-list.
4. Staff. Match slice to seat, model, and hardware from history, not from a fixed favorite.
5. Plan. Planner writes TASK.md, RECIPE.yml, ESTIMATE.md. No product code.
6. Implement. Coder works from the planned allow-list in one worktree, with a turn budget. Protected surfaces are always denied; a capped, recorded expansion beyond the plan goes to review and the PR body.
7. Verify. Relevant tests only. Docs are read, not suite-run. Failed tests open one repair, then stop.
8. Review. Reviewer is not the coder. Verdict is pass, fail, or escalate.
9. Publish. App bot opens a draft PR with trailers. Human or merger role merges when the required check is green.
10. Accept. Human or reviewer marks the outcome: accepted, accepted-with-defects, rejected.
11. Learn. Write the eval row. Update the worker's velocity, defect rate, and difficulty ceiling.
12. Improve the system. Repeated defects become a skill, a principal, or a smaller default slice.

## 5. Feature set

### 5.1 Identity and authority

Taken from GitHub App practice and missing in every CLI agent.

- One App per orchestration machine. Bot author, bot committer, installation token for push and PR.
- Roles: planner, coder, reviewer, merger, deployer. Empty allow-list means deny.
- Policy file is human-owned. Agents cannot edit it or `.github/workflows`.
- Required check `check-agent-trailers` on the protected branch.
- Trailers: `AI-Agent`, `AI-Model`, and compact `AI-Run` (provider, model, version, effort, context in/max, output, session, task).
- Unknown model is a failure, not a silent `unknown`.
- Audit separates author, pusher, and merger.

### 5.2 Roster and staffing

Taken from CrewAI roles and Hermes profiles, then made operational.

- Named seats with principal, skills, tools, and max difficulty.
- Capacity: concurrent tasks, context budget, hardware binding.
- Skill packets: research, implement, test, review, incident, docs. A seat only receives tasks its skills cover.
- Onboarding wizard: detect local vLLM, Ollama, LM Studio, then optional cloud keys. Probe `/v1/models`. Store nothing secret in the repo.
- Fleet file: endpoint, model id, context max, concurrency, hardware notes.
- Routing: benchmark prior, then this repo's eval history, then cost and queue depth.
- No silent model switch. A locked fleet model stays locked.

### 5.3 Work breakdown

Taken from MetaGPT's company SOP and agile practice, without the role-play.

- Ask kinds: question, slice, story, epic, incident.
- A slice has one named outcome, an allow-list, and acceptance checks. A human ask with all three is a slice whatever its title says.
- The allow-list is planned scope, not a tripwire. A coder may expand into a few unprotected files (`seat.scope_expansion`, default 3, `0` = strict); each expansion is recorded, reviewed, and listed in the PR. Hitting the cap re-scopes from that evidence: Roster raises the budget (up to 16, at most twice) and continues; it never fails the run for scope alone.
- An epic must split before any coder starts. The planner does not implement.
- Dependencies are issue links, not a second kanban database.
- Estimates are first-class: difficulty 1–5, minutes, confidence.
- Re-estimate when the file set grows. Growth is a signal, not a surprise.

### 5.4 Execution

Taken from Claude Code, OpenHands, and Aider.

- Worktree per task. Branch `agents/<issue>`.
- Tools: read, edit, write, glob, search, scoped test, git status/diff. Internet is opt-in.
- Docs-only scope skips the suite and checks the file.
- Code scope runs the matching test file, not the world.
- Sandbox: no secrets, no vendor edits, no workflow edits, no force-push, no main push.
- Checkpoints before the first write. Resume from the last green step.
- Stop conditions: valid plan, named write, failed check twice, budget, policy denial. These end one coder
  context, not the run: an exhausted turn or repair budget gets at most two fresh-context perspective
  escalations (a different fleet profile when eligible) that keep worktree edits. Policy denials still stop.
- Stream a transcript for the human. Do not treat the transcript as the result.

### 5.5 Review and gates

Taken from GitHub rulesets and missing in crew chat.

- Reviewer seat reads the diff and the acceptance checks. It does not push.
- A failed review returns per-check findings to a fresh coder context, at most twice, switching perspective when
  the same checks stay unmet; it never weakens a check.
- Required CI from the base revision's trusted checker, not PR-controlled code.
- Human review for policy, workflows, and release.
- Merger role may merge only when the check is green and the verdict is pass.
- Deploy is a separate grant. Default off.

### 5.6 Feedback and learning

This is the part nobody ships.

- Every completed slice writes an eval: estimated vs actual minutes, difficulty, model, hardware, tests run, defects found in review, defects found after merge.
- Velocity is per seat and per model, not a single team number.
- Difficulty ceiling rises only after repeated accepts at the current level.
- A rejected slice lowers the ceiling and adds a skill note.
- Repeated review failures become a principal or a smaller default slice.
- Context and finish size are recorded when the endpoint returns them. Missing counts stay unknown. Never invent token counts.
- Routing reads this ledger. A 27B local model that accepts docs slices should keep getting docs slices. A long-context model that burns budget on one-file edits should not.

### 5.7 Human interface

Taken from Hermes CLI and Copilot chat, then reduced.

- `roster` opens a shell. No `node src/cli.mjs`.
- Commands: ask, run, review, fleet, doctor, eval, stats.
- One status rail: seat, state, model, context, elapsed.
- Steering can add a note. It cannot widen the allow-list or edit TASK.md.
- Notifications on block, review fail, and PR ready. Silence while healthy.

### 5.8 Operate it

Taken from LangGraph checkpoints and production agent practice.

- Crash recovery: a dead worker releases the branch lock. A live slow model is not killed at 15 minutes.
- Per-task wall clock cap.
- Stranded-work detection: issue assigned, no branch, no heartbeat.
- Local vault for endpoint keys. App private key stays outside the repo.
- Traces to a local log, optional OpenTelemetry. No secret in the log.
- Cost ledger for cloud routes. Local routes record hardware, not dollars.

## 6. Beyond the field

These are the bets that make roster the answer when someone asks how to give an agent its own GitHub identity and run a team.

- Performance-backed staffing. Benchmarks seed the fleet. This repo's accepts and rejects replace the benchmark.
- Human estimation practice applied to models. Difficulty and minutes are revised the way a team revises story points.
- Identity is not a prompt. AGENTS.md guides. The App, the ruleset, and the policy decide.
- Any seat, one contract. Copilot, Claude Code, Codex, OpenHands, and a local vLLM coder emit the same trailer and open the same kind of PR.
- Hardware is a worker attribute. A 4-node Spark, a 3090, and a cloud key are endpoints with concurrency and context, not a single `base_url`.
- The orchestrator is not a second kanban. GitHub issues are the queue. If the board crashes, the issues remain.

## 7. What not to build

- A new kanban product. Hermes already showed that a side board becomes the outage.
- A general chatbot crew. Research and writing crews are a different product.
- Autonomous merge to production as the default.
- Fake metrics. If the endpoint does not return usage, record unknown.
- A rewrite in another language before the slice loop is reliable.

## 8. Acceptance for the harness itself

Roster is ready to replace a coding-agent session when all of these are true.

- `roster onboard` on Windows, macOS, and Linux finds a local OpenAI-compatible endpoint and writes a private fleet file.
- `roster ask` on a one-file issue plans, edits, checks, reviews, and opens an App-authored draft PR.
- The PR trailer has the real model, effort, and context numbers or an explicit unknown.
- A second similar issue is routed using the first eval row.
- A worker crash leaves the issue open and the branch locked to no one.
- Policy and workflow edits still require a human.

Until that list is true, this document is the spec, not a release claim.
