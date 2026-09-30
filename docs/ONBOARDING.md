# Onboarding wizard

Roster is a Node 20+ ESM executable with no runtime npm dependencies.
Install from a persistent clone with its contracts submodule, then run
the wizard in the project you want to configure. Git and `gh` are needed
for the issue and PR workflow; the wizard never installs App credentials,
policy, or workflows.

## Windows PowerShell

Install Node 20+, Git, and GitHub CLI for Windows, then in PowerShell:

```powershell
git clone --recurse-submodules https://github.com/R4yHarris/github-agent-roster.git
cd github-agent-roster
npm install -g .
```

For a first run in this clone:

```powershell
roster --help
roster onboard
roster doctor
roster
```

To configure another repository, change to that worktree before
`roster onboard`. Keep Node and npm's global bin directory on PATH;
restart PowerShell after changing User environment variables.

## WSL

Install Node 20+, Git, and GitHub CLI **inside WSL**. Windows and WSL have
separate Node installations, global bins, filesystem paths, and shell
environment. Do not reuse a Windows Node executable as the WSL runtime or
forward a human's GitHub credentials to an agent worker.

```sh
git clone --recurse-submodules https://github.com/R4yHarris/github-agent-roster.git
cd github-agent-roster
npm install -g .
roster --help
roster onboard
roster doctor
roster
```

vLLM may run inside WSL, on Windows, or on a DGX host. Its default API port
is **8000**, with base URL `http://127.0.0.1:8000/v1` when loopback reaches
the server. Depending on WSL networking mode, `localhost` in WSL may not
reach a Windows-hosted server. Paste the reachable Windows host IP
(`http://<windows-host>:8000/v1`) or DGX address into the endpoint prompt.
Roster does not guess that address, modify the network, or open firewall
ports. Use a secured endpoint or tunnel for access beyond loopback.

## Linux

Use a user-managed Node 20+ installation, Git, and GitHub CLI:

```sh
git clone --recurse-submodules https://github.com/R4yHarris/github-agent-roster.git
cd github-agent-roster
npm install -g .
roster --help
roster onboard
roster doctor
roster
```

Run onboarding from the target project if it differs from this clone.
A local or DGX vLLM endpoint uses the same prompts and `/v1/models`
discovery; configure a reachable secured host if it is not on loopback.

## macOS

Use a user-managed Node 20+ installation, Git, and GitHub CLI:

```sh
git clone --recurse-submodules https://github.com/R4yHarris/github-agent-roster.git
cd github-agent-roster
npm install -g .
roster --help
roster onboard
roster doctor
roster
```

The same numbered prompts work in the macOS terminal. The server can be
on another secured host; use its actual reachable vLLM URL and model.
Do not run onboarding with `sudo` to work around global-bin PATH setup.

## Wizard steps

1. Run `roster onboard` in a terminal. The wizard displays the platform.
   Non-TTY input or output prints `roster onboard needs a terminal` and
   exits 2 without writing files.
2. Choose the LLM endpoint and an actual model. Prompts show defaults in
   brackets; blank accepts a default. A required model has no invented
   default and must be supplied when discovery is unavailable.
3. Answer local permission questions. Advanced settings are hidden unless
   requested. These preferences are not GitHub policy grants.
4. Review the endpoint, model, permissions, and destination. Answer
   `Confirm write .roster/config.yml? [yes]` before any config or ignore
   rule is written. Existing config also requires explicit overwrite
   consent; declining either confirmation preserves it.
5. After a confirmed atomic write, read the doctor result. Ctrl+C before
   write confirmation exits 0 without a half-finished config.

For installation commands, see [INSTALL.md](INSTALL.md). The implementation
is [src/onboard/wizard.mjs](../src/onboard/wizard.mjs); neither the wizard nor
its local preferences changes App policy, vendor sources, or workflows.
`roster init` remains non-interactive: it copies examples without
overwriting existing files. It is not a substitute for confirming the
private config in the wizard.

The saved `.roster/config.yml` is at the current Git worktree root
(even when launched from a nested directory), or the current directory
outside Git. Roster ensures private config is ignored and refuses a
tracked config. Commands prefer the project's saved settings, then
installation settings. Nothing changes the installed package's config
when onboarding a different project.

## Probing /v1/models

The endpoint step defaults to `http://127.0.0.1:8000/v1`. WSL talking to a
Windows-hosted server may need the Windows host IP instead of localhost.
Enter the API base URL, not the full models or chat-completions endpoint.

The wizard GETs `<base_url>/models` with a five-second total deadline,
including JSON parsing. It reads the OpenAI list shape `data[].id`,
prints the actual IDs, and defaults to the first one. A failed probe prints
only a safe class (`timeout`, `refused`, `HTTP <status>`, `network`, or
`invalid response`) before asking for the actual served model ID.
It never displays an upstream body or error details, invents an ID, or
uses `unknown` as a fallback.

Confirmed ignored configuration records `llm.profile: vllm-local`,
the chosen `llm.base_url` and `llm.model`, `llm.provider: vllm`, and
`llm.api_key_optional: true`. A custom vLLM host is not replaced with
loopback when the config is read. Discovery is not a chat or internet-tool
probe; no network tool is added to the coder.

## Permissions and Advanced

After the model step, the wizard asks in order:

- `Allow publish through GitHub App? [yes]`
- `Require reviewer before publish? [yes]`
- `Allow run_test? [yes]`
- `Show advanced settings? [no]`

These flags do not grant contracts policy. Publishing still needs the App
and reviewed human-owned policy. The saved keys are `publish.enabled`,
`review.required`, and `tools.run_test`. Disabling tests does not silently
waive task acceptance checks, and making review optional does not remove
the reviewer or permit merging by a model.

Only Advanced displays internet/search preference, `Max tool turns [12]`,
and `Context char budget [8000]`. When entered, internet defaults yes but
only stores a flag; no network tool is implemented. When skipped,
`tools.internet` is **omitted** from the written YAML. The parser's existing
fallback remains false; there is no implicit permission to access the web.

`loop.turns` controls the coder turn limit (1-64), and `context.budget`
controls the positive context-character budget. New onboarding starts with
12 turns and 8000 characters; skipping Advanced preserves an existing
saved loop/context budget. Legacy `reviewer.required`, `seat.turn_budget`,
and `seat.context_chars` configs remain supported, while the wizard writes
the canonical keys. Conflicting review aliases are errors, not silent defaults.

## App environment

When App publication is enabled, the wizard checks `GITHUB_APP_ID` and
`GITHUB_APP_PRIVATE_KEY_PATH` without printing their values. If both are
present it prints `App env present`; a missing, non-file, or inaccessible
PEM path produces a warning and setup continues. The key is inspected as
file metadata only: no PEM is created, read into a prompt, or copied into
`.roster`.

Use one GitHub App for all worktrees. On Windows, set **User** environment
variables using placeholders replaced locally:

```powershell
setx GITHUB_APP_ID "<app-id>"
setx GITHUB_APP_PRIVATE_KEY_PATH "<absolute-path-to-existing-pem>"
```

Restart the terminal after `setx`; an existing shell does not inherit the
new User environment. In Unix shells:

```sh
export GITHUB_APP_ID="<app-id>"
export GITHUB_APP_PRIVATE_KEY_PATH="<absolute-path-to-existing-pem>"
```

Never paste PEM contents or tokens into a prompt, config, or Git. Presence
checks are not App authentication, an installation-token probe, or policy
grants. Humans still own policy, workflows, and the external private key.

## Doctor after confirmation

After a successful confirmed write, onboarding calls doctor in the **same
Node process**. It makes no network requests and prints no secret values.
It checks Node major >=20, the vendor publisher, opted-in App environment
and key-file presence, the human-owned policy and trailer workflow when
publishing is enabled, and a real model in the project's `.roster/config.yml`
when run or publish is expected. `AI_MODEL` cannot substitute for the saved
vLLM model.

Publishing disabled means App/policy/workflow checks are shown as `SKIP`,
not failures; a configured run-only endpoint still requires its saved model.
An offline nonpublishing stub does not require a model. Invalid private
config is an explicit failure, never a successful default.

Onboarding exits 0 only when the required checks pass. A failed doctor
exits 1 **after retaining the confirmed config**; fix the listed blocker:

- Use Node 20+ on PATH for a runtime failure.
- Initialize the contracts submodule for a missing vendor publisher.
- Set the App environment and an existing external PEM for App failures.
- Ask a human to provision policy or workflows; the wizard never writes them.
- Rerun onboarding or fix the saved private model/config for a model failure.

Then run `roster doctor` again from the project. These are local preflight
checks, not GitHub policy grants, authentication, chat, or internet-tool probes.

## The vLLM model is not the GHCP publication model

The model selected in onboarding is saved as `llm.model`. It is the real
model served by the chosen vLLM endpoint and used for `roster run` by the
planner, coder, and reviewer. Keep that served ID: do not replace it with
a Copilot model name merely to publish a PR.

User environment `AI_MODEL` describes the model used for a GHCP agent's
publication. For this Copilot session, set the publisher's current-session
attribution in PowerShell when the reviewed feature worktree is ready:

```powershell
$env:AI_MODEL = "GPT-6.1-Sol"
$env:AI_PROVIDER = "github-copilot"
node vendor\github-agent-contracts\scripts\agent-pr.mjs --message "<conventional subject plus Model, Summary, and test instructions>" --model GPT-6.1-Sol --merge-when-green
```

If using Windows User environment via `setx`, restart the shell before
publication. This environment metadata does not grant App or policy
capabilities. Roster-managed publication uses the completed coder's
actual model, provider, and reported usage instead; an onboarding config
with `llm.model` keeps that model ahead of inherited `AI_MODEL`.

No wizard step probes chat completions, App authentication, GitHub merge
permissions, or a live internet/search tool. The models request is the
only endpoint probe; doctor is offline. Internet true stores a future
preference only. Tests and review still occur during the configured task
run, and the human records AI-Eval and closes the issue afterwards.
