# Interactive onboarding

Roster uses Node 20 ESM with no runtime npm dependencies. Install from a
persistent clone with the required contracts submodule, then run
`roster onboard` in a terminal from the project you want to configure.
Git and `gh` are needed for the GitHub issue loop and publishing, not for
the model discovery wizard itself.

## Install

### Windows (PowerShell)

Install Node 20, Git, and GitHub CLI, then:

```powershell
git clone --recurse-submodules https://github.com/R4yHarris/github-agent-roster.git
Set-Location github-agent-roster
npm install -g .
roster --help
```

Open your target project in PowerShell and run:

```powershell
roster onboard
```

### WSL

Install Node 20, Git, and GitHub CLI inside WSL; do not depend on a Windows
Node executable or share human GitHub credentials with a worker.

```sh
git clone --recurse-submodules https://github.com/R4yHarris/github-agent-roster.git
cd github-agent-roster
npm install -g .
roster onboard
```

Run onboarding from the intended target project. If vLLM runs on Windows,
paste a reachable Windows host URL such as
`http://<windows-host>:8000/v1`. Loopback reachability depends on WSL
networking mode; Roster does not guess a host address or change firewall
settings. Secure any server reachable beyond loopback.

### Linux and macOS

With Node 20, Git, and GitHub CLI installed in your user environment:

```sh
git clone --recurse-submodules https://github.com/R4yHarris/github-agent-roster.git
cd github-agent-roster
npm install -g .
roster onboard
```

Use a user-managed Node installation rather than running the wizard with
`sudo`. The same prompts are used on `win32`, `linux` (including WSL),
and `darwin`. For a previously cloned repository, initialize the dependency
with `git submodule update --init --recursive`.

`roster init` is still non-interactive. It copies only the config example
and policy note, keeps existing files, and never creates a private config,
policy, or workflow. Onboarding is a separate, explicitly interactive command.
Piped input or redirected output prints `roster onboard needs a terminal`
and exits 2 without probing or writing files.

## Wizard

1. Roster prints the detected OS.
2. Enter a vLLM API base URL, defaulting to `http://127.0.0.1:8000/v1`.
   URLs with credentials, query strings, fragments, or full `/models` or
   `/chat/completions` paths are refused.
3. Roster GETs `<base>/models` with a five-second total deadline, including
   JSON parsing. It lists actual `data[].id` entries and asks you to choose
   one. A failed or invalid probe is reported and asks for the actual served
   model ID instead. There is no fabricated model or `unknown` fallback.
   Discovery sends no credentials and follows no redirects.
4. Answer the local permission questions; blank means the bracketed default:

   | Question | Default | Effect |
   | --- | --- | --- |
   | Publish via GitHub App? | yes | Saves `publish.enabled`; false blocks Roster-managed publication, including `--publish` and `/publish`. This does not grant policy capability. |
   | Allow run_test? | yes | Saves `tools.run_test`; false removes the model tool and denies automatic tests. Tasks requiring tests stop before coding unless tests are explicitly waived in TASK.md. |
   | Reviewer required before publish? | yes | Saves `reviewer.required`; false bypasses only the review verdict, disclosed in issue PR bodies. The reviewer still runs; tests and excellence remain mandatory. |
   | Show advanced? | no | Only a yes displays the Advanced section. |

   The Advanced section asks whether to permit internet search/research
   beyond the worktree, default yes. It saves `tools.internet`. If Advanced
   is hidden, that default is stored without showing the extra question.
   **This flag is stored only. There is no live internet/search tool in
   this change.** Existing worktree research and model inference are not
   replaced by a new runtime.
5. Roster saves settings, prints the endpoint/model and permission summary,
   then prints the existing offline `roster doctor` checks. Missing App,
   policy, or workflow prerequisites are reported; the config remains
   saved but does not authorize publication.

The wizard does not ask for, print, or write tokens, API key values, PEM
contents, or private-key paths. It does not modify contracts sources,
`agent-policy.yml`, or workflows. A discovery probe checks only `/models`:
it does not test chat completions, coding tools, tests, internet search,
GitHub access, or whether the App can merge.

## Private configuration

The ignored file is `.roster/config.yml` in the current Git worktree root,
or the current directory when outside Git. Running from a nested project
directory selects that worktree root. The wizard adds private-config ignore
rules to the project's `.gitignore` if needed, refuses a tracked private
config, and asks before replacing existing onboarding settings. It keeps
unrelated settings, normalizes the YAML, and writes atomically.

The resulting model settings include:

```yaml
llm:
  profile: vllm-local
  base_url: "http://127.0.0.1:8000/v1"
  model: "your-actual-served-model"
  api_key_optional: true
  provider: vllm
```

Other required fields from the existing config or installed example are
retained. A custom `llm.base_url` overrides the vLLM profile's default,
so a WSL host selection is not silently replaced with loopback.

CLI commands and the interactive shell prefer the project's private config,
then the installation's private config, then its tracked example. `/model`
and `/effort` update the project config when one exists. Legacy configs
retain enabled explicit publishing/testing and a required review by default.
See [endpoint settings](ENDPOINTS.md) for manual configuration.

## When ready to PR

For this Copilot agent, set `AI_MODEL=GPT-6.1-Sol` and
`AI_PROVIDER=github-copilot`. With both App credentials provisioned outside
Git, run from the reviewed feature worktree root in PowerShell:

```powershell
node vendor\github-agent-contracts\scripts\agent-pr.mjs --message "..." --model GPT-6.1-Sol --merge-when-green
```

Replace the message placeholder with a conventional subject plus
`## Model`, `## Summary`, and how to test. Never publish a subject-only
body. POSIX terminals use the same flags with the POSIX path separator;
Roster-generated commands use the actual configured coder model, not a
hardcoded Copilot model.

Publishing still requires the configured App, external PEM, reviewed
human-owned policy, and required GitHub checks. The local permission answers
are not policy grants. No draft is left for a human, and no human-credential
fallback is used. See [the full GHCP handoff](GHCP.md).
