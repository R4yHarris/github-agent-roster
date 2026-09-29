# 30-second offline demo

From the repository root with Node 20+, run the bundled Ask template. No GitHub
issue, App credentials, or LLM endpoint are needed:

```sh
node src/cli.mjs run --ask-file templates/sdlc/ASK.md --runtime builtin
```

If the `roster` bin is installed, the equivalent command is
`roster run --ask-file templates/sdlc/ASK.md --runtime builtin`. The template
uses the sample request in [fixtures/demo-task/ASK.md](../fixtures/demo-task/ASK.md):
add a Status section to a demo README.

The CLI prints `Worktree`, `RECIPE`, `TASK`, `RESULT`, and `Mode: stub` paths.
Open the printed `RECIPE.yml` to see the planner and coder seats, `TASK.md` to
see the requested change, and `RESULT.md` to see the stub's outcome. This is
a temporary demo directory, not a Git worktree; it remains after the command
so you can inspect it. The copied `README.md` is deliberately unchanged.
The same directory also contains the bounded `CONTEXT.md`, read-only
`RESEARCH.md`, and planner `ESTIMATE.md`. RESULT.md labels the stub's
unexecuted checks as unverified rather than claiming a completed implementation.

The deterministic stub proves that the planner passes a task to the coder and
the coder writes a result in the same run. It **does not** implement the
requested Status section, run `node --test`, contact a model, create a GitHub
issue or PR, or publish code. Planner and coder session records are appended
to ignored `.roster/memory/` files in the repository; leave those journals
intact.

When finished, confirm the printed `Worktree` path is an OS temporary directory
whose final component begins `roster-demo-`. Delete **only that exact
directory**, not the repository or the entire OS temp directory. For example,
after substituting the path printed by your run:

```powershell
Get-Content -LiteralPath '<printed RESULT path>'
Remove-Item -LiteralPath '<printed Worktree path>' -Recurse
```

See the [interactive shell](REPL.md) for the human interface and
[same-session seats](MULTIAGENT.md) for the issue-based planner/coder run.
