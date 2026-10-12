# Eve first-wave integration evidence

This report records the bounded test slice in [#586](https://github.com/R4yHarris/github-agent-roster/issues/586), a child of [#476](https://github.com/R4yHarris/github-agent-roster/issues/476). It is implementation and automated-verification evidence only, not acceptance of the integrated pilot.

This work traces to [FEATURE_SPEC](FEATURE_SPEC.md) sections 5.4 (execution), 5.5 (review and gates), 5.6 (honest evaluation), and 5.8 (operation), honoring sections 3 and 7. It adds a regression test and report only: no runtime behavior or dependencies changed.

## Deterministic lifecycle comparison

`tests/eve-pilot.integration.test.mjs` invokes the public `runBuiltinIssue` path twice against the same temporary repository, task ask, source revision, and available tools. Separate issue worktrees isolate the baseline and opted-in runs. The baseline omits `seat.evidence_workspace`; the second run sets it to `true`.

The transport is a deterministic in-process test double for planner, coder, and reviewer requests. It makes no live inference or network request and returns no endpoint usage. Repeated injected test failures force the existing fresh-coder handoff; after recovery, the fixture's actual `node --test` command runs and passes. The opted-in context is asserted to carry the original checks with check 1 `pending` and check 2 `in_progress`; the baseline context has no acceptance continuation. The completed runs then pass their actual returned automated-test and reviewer outcomes to the paired evaluator. The reviewer is the separate builtin seat, but its response is scripted, not a human review.

| Observation | Baseline (option omitted) | Evidence workspace (opt-in) |
| --- | --- | --- |
| Fixture task | Synthetic documentation task | Same task and base revision |
| Final fixture `node --test` | pass | pass |
| Scripted builtin reviewer verdict | pass | pass |
| Acceptance continuation | absent | carried through the forced handoff; verified on completion |
| Human acceptance | unknown | unknown |
| Endpoint usage and resource measurements | unknown | unknown |

The evaluator receives one paired task and one trial per condition. Both records are marked `synthetic: true`; `held_out` is `false` because this harness case was not independently selected as a research-held-out task. Its comparison summary is deterministic and reports `pilot-incomplete`, zero measured held-out tasks, no comparable human-acceptance pair, and an `unknown` latency threshold. Prompt/completion tokens, latency, energy, cost, and hardware remain unknown. These observations do not demonstrate model inference, a human acceptance, or superiority.

Scope remains bounded to the issue worktree: `README.md` is the only changed application file, and the source repository `HEAD` is unchanged. The test also requires the reviewer request to have no coder tools. Existing lifecycle/continuation tests exercise cancellation and no-action-after-cancellation; those tests passed in the verification set below.

## Verification record

- Runtime: Node `v20.19.5`, ESM; Windows `win32` x64, OS release `10.0.26200`.
- The Node 20 executable was installed side-by-side with the existing Node 24 runtime; the global Node selection was not changed.
- Required contracts submodule resolved through `resolveContractsPath` to `vendor/github-agent-contracts`, commit `61b97b3e93e670a675c0301cce5412c73404d179`.
- The fetched implementation base was `fe9bd1188739d406e74eec22d11693df60d81d71`; #474 and #475 were closed before this slice began.
- No runtime dependencies, billed inference, hardware run, or 40-task plus 20-fault benchmark.

The following PowerShell commands ran with `$node20` resolving to the side-by-side Node 20.19.5 executable:

```powershell
& $node20 --version
# v20.19.5; exit 0

& $node20 --test --test-reporter=dot `
  tests\eve-pilot.integration.test.mjs `
  tests\eve-pilot.test.mjs `
  tests\builtin.lifecycle.test.mjs `
  tests\builtin.recovery.test.mjs `
  tests\builtin.replay.test.mjs `
  tests\loop.acceptance.test.mjs `
  tests\config.test.mjs `
  tests\context.test.mjs `
  tests\checklist.test.mjs `
  tests\runtime.test.mjs `
  tests\builtin.issue-run.test.mjs
# exit 0; all selected test cases passed
```

Resolved harness-only failed attempts:

- The first focused run exited 1 because the injected test runner's third argument is an options object, not a working-directory string. The test now uses its explicit `cwd`.
- A subsequent run exited 1 because the shared research-test adapter intercepted reviewer calls and supplied canned usage. This integration test now calls the public builtin entry point directly and scripts every seat response itself, leaving usage absent.
- An initial assertion expected the first handoff check to be `blocked`; the observed capsule kept it `pending` while check 2 was `in_progress`. The final assertion records those observed states.

The baseline and opt-in runs use ephemeral repositories created by the test;
their generated fixture commit IDs are not publication SHAs. At the child
handoff no implementation commit or PR had been created. Review, publication
and pilot acceptance remain separate; this report does not claim #476 acceptance.

## Parent review and fresh-main replay

The parent independently reviewed the two-file scope after exclusive child
release, then fast-forwarded it to reviewed Roster main
`c7186eb9d4a1ac3cd073d698a7242cf84353ff20` and initialized the reviewed contracts
pin `fa48c244bc9b8ead5df927ec705ef41a39c56f68`. No vendor sources were rewritten.

On Windows Node `v20.20.2`, the following exact replay passed **20/20** tests,
zero failures or skips, in **27729.7464 ms**:

```powershell
& $node20 --test tests\eve-pilot.integration.test.mjs tests\eve-pilot.test.mjs
```

The inherited dot-reporter run above did not record a numeric total; do not
derive one from this different selector. Earlier child Auto-selected model
identity was unverifiable; parent review/publication metadata must not be
presented as measured child model/usage. This fresh replay confirms public
lifecycle-to-evaluator plumbing, default-off behavior and honest unknowns, not
a live model comparison or measured held-out pilot. Parent #476 remains open
for its separate acceptance and comparative-evidence obligations.
