# Trailer and test CI

The human-owned [trailer workflow](../.github/workflows/check-agent-trailers.yml)
runs on pull requests. Its `jobs.trailers.name` must remain
`check-agent-trailers`: that is the required check name. The independent
`tests` job is not yet required by the ruleset.

Both jobs check out with `actions/checkout@v4` using `fetch-depth: 0`,
`persist-credentials: false`, and `submodules: recursive`, then use
`actions/setup-node@v4` with `node-version: 20`. The trailer job invokes the
local [composite action](../.github/actions/check-agent-trailers/action.yml),
passing the pull request's full base and head SHAs. That action passes
`BASE_SHA` and `HEAD_SHA` to the complete pinned checker:

```sh
node vendor/github-agent-contracts/scripts/check-pr-agent-trailers.mjs
```

Do not detach to the base revision before invoking the composite action.
Doing so previously selected an older local checker without its
`parse-agent-run.mjs` dependency and failed before checking trailers.
Full history is required for the `base..head` commit range, and the
recursive checkout supplies the `v0.2.0` contracts submodule. The tests
job runs `node --test tests/*.test.mjs` on Node 20 independently of the
trailer check.

Workflow files are owned and published by humans, not the coder seat. If
workflow permissions are unavailable, provide the required YAML to a human
for review rather than committing a workflow from the App. Verify a change
with `node --test` locally and confirm both checks on its PR. See the
[contracts dependency guide](DEPENDENCY.md) for submodule initialization.
