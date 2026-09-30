# Human evaluation is the retrospective

After reviewing `RESULT.md` and the delivered changes, the **human** records
the outcome. A model, coder seat, passing test, or publisher cannot accept
its own work.

```sh
roster eval roster-42-coder accept 3 n --minutes 18 --comment "Ship quality; keep the edge-case tests."
roster eval COMMIT_SHA rework 4 y --minutes 35 --comment "Retry the same task with the missing cases."
```

The shell supports the same flags with `/eval`; quote comments containing
spaces. Full or unambiguous abbreviated SHAs and opaque session IDs remain
supported. The original four-argument form remains valid: absent minutes are
`null`, never fabricated zero actuals, and the comment defaults to empty.
Minutes are nonnegative integers; comments are limited to 4 KiB.

| Verdict | Meaning |
| --- | --- |
| `accept` | Ship-quality delivery |
| `rework` | Repeat the same task with the feedback |
| `reject` | The breakdown was wrong; reconsider the task |

In agile terms, **difficulty** (1-5) is the refined point estimate and
**minutes** are actuals. `again` records whether the human would reuse that
configuration, not an automated quality score.

The command appends one JSON object per line to the repository-root
`.roster/evals.jsonl`, even from a nested directory:

```json
{"sha":null,"session":"roster-42-coder","model":"served-model","task_class":"fix","verdict":"accept","difficulty":3,"again":false,"minutes":18,"comment":"Ship quality.","at":"2026-09-29T12:00:00.000Z"}
```

SHA, session, model, and task class are enriched from matching local runs and
contracts-exported history. Unreported fields stay `null`; current config is
not substituted for the model that actually did the work. Legacy evaluations
still load. Corrections append rather than rewriting earlier human feedback.

If `gh` and one matching PR exist, the command posts as the authenticated
**human**, never as the publishing App:

```text
AI-Eval: 1|accept|3|n
Minutes: 18
```

The free-text comment remains local and is not posted to GitHub. Without
timing, the legacy comment contains only the AI-Eval line. Missing `gh`, origin,
or a PR leaves a local-only evaluation; GitHub failures and ambiguous PR matches
are reported explicitly **after** the local record is saved. Do not blindly
repeat the evaluation to retry a failed comment.
The issue remains open after a PR merges; a human closes it after reviewing
the delivery and posting AI-Eval. Neither the App publisher nor `roster eval`
closes issues automatically.

There is no eval tool in the coder seat. Its file tools cannot write
`.roster/evals.jsonl`, and child commands carry `ROSTER_SEAT=coder`; the human
writer rejects agent-seat calls before Git or filesystem access. These are
capability boundaries, not an operating-system sandbox for untrusted tests.

See [estimation](ESTIMATION.md) and [learning](LEARNING.md).
