# Offline paired Eve pilot comparison

This evaluator implements [FEATURE_SPEC 5.6](FEATURE_SPEC.md#56-feedback-and-learning)
and [5.8](FEATURE_SPEC.md#58-operate-it), honoring sections 3 and 7.
It is a dependency-free Node 20 ESM executable over **operator-provided local
JSON**, not a runtime, task board, authority grant, history exporter, or new
learning store. It makes no model, network, GitHub, publication, or persistence
calls. It does not modify inputs.

Design source: [published Eve research, section 11](https://github.com/R4yHarris/github-agent-roster/blob/main/docs/EVE_RESEARCH.md#11-experimental-research-plan).
The proposed research gates are not measured results or production routing
rules. This tool reports comparisons, never an intelligence score or broad
superiority claim.

## CLI

From the repository root:

```powershell
node scripts\eve-pilot.mjs --manifest tests\fixtures\eve-pilot.json
node scripts\eve-pilot.mjs --manifest pilot.json --records outcomes.json
node scripts\eve-pilot.mjs --help
node --test tests\eve-pilot.test.mjs
```

`--manifest` is required. `--records`, when supplied, is a JSON **array** that
overrides embedded `records`; it is not JSONL or an instruction to export
machine history. Both paths are explicitly supplied local regular files.
Success prints deterministic, pretty-printed JSON followed by one newline.
`--help` alone prints usage to stdout and exits 0. Unknown, duplicate, or
missing flags fail explicitly. Validation/JSON/argument errors exit 1;
local-file I/O errors exit 2. Errors have the form
`eve-pilot: <code>: <field/context>: <reason>` on stderr, with empty stdout.
A reported safety violation or unknown gate is a valid comparison, not a
CLI input error; inspect the JSON rather than using exit 0 as a pilot pass.

## Public library API

`src/lib/eve-pilot.mjs` exports:

- `PILOT_LIMITS`: frozen bounds, detailed below.
- `PilotsError`: an `Error` with `name = "PilotsError"` and a `code`.
  Codes include `invalid-input`, `invalid-condition`, `duplicate-identity`,
  `mismatched-controls`, and `pairing-error`. CLI errors additionally include
  `invalid-json`, `invalid-arguments`, and `io-error`.
- `validatePilotManifest(manifest)`: validates schema, identities, conditions,
  task descriptors, and optional thresholds; returns a fresh canonical JSON
  copy, with object keys sorted. Does not validate record pairing by itself.
- `evaluatePilotPair(manifest, records = manifest.records)`: validates records,
  frozen snapshots, and matched trial identities. Returns
  `{ manifest, records, source_records }`: a canonical manifest, deterministic
  normalized trial records, and canonical source records for revalidation.
  It does not fabricate a completed Roster seat or response-backed run usage.
- `buildComparisonSummary(evaluation)`: takes the preceding result,
  revalidates its manifest and `source_records`, and returns the deterministic
  JSON comparison. It never trusts changed normalized `records` as evidence.

All exports are synchronous, pure data transformations with no hidden reads,
writes, subprocesses or state. `TASK_CLASSES` and the full-SHA convention are
reused from the existing learning module. Human verdicts use its
`accept`/`reject`/`rework` vocabulary, but pilot observations are not local
learning rows: `again`, seat/session and completed-run metrics are not invented
to force them through `validateLocalEvaluation`.

## Manifest schema 1

See [the complete synthetic example](../tests/fixtures/eve-pilot.json) and
[its fixed expected summary](../tests/fixtures/eve-pilot-summary.json).
Required fields:

| Object | Field | Type / constraint |
| --- | --- | --- |
| manifest | `schema` | Integer `1` |
| manifest | `synthetic` | Boolean; true makes every observation synthetic |
| manifest | `conditions` | Exactly two objects: one `baseline`, one `eve` |
| manifest | `tasks` | Nonempty array of unique task/fault descriptors |
| condition | `id`, `kind` | Unique opaque ID; `baseline` or `eve` |
| condition | `model`, `profile` | Nonempty bounded text; must match across conditions |
| condition | `behavior_bundle` | Frozen JSON object with required opaque `id` |
| condition | `shared_controls` | Required object described below |
| task | `id`, `class` | Unique opaque ID; `docs`, `fix`, `feat`, or `test` |
| task | `difficulty` | Integer 1–5 |
| task | `base_revision` | Full 40- or 64-character hexadecimal Git SHA |
| task | `model`, `profile` | Must match condition values exactly |
| task | `kind` | `task` (representative software task) or `fault` |
| task | `held_out`, `synthetic` | Explicit booleans; not inferred from names |

Opaque IDs are 1–64 ASCII alphanumeric/dot/underscore/hyphen characters, but
not `-` alone. Bounded text is 1–256 characters without surrounding whitespace
or control characters. Base SHAs compare case-insensitively; all other identity
text compares exactly. Extra JSON metadata is allowed but never determines
acceptance, usage, or authorization.

Each `shared_controls` contains:

- `sampler`: nonempty frozen object of explicit scalar settings (strings,
  booleans or finite numbers, no unknown/null/nested values). When present,
  `temperature` is nonnegative, `top_p` is in `(0,1]`, and `seed`/`top_k` are
  nonnegative safe integers. Record all relevant decoding settings.
- `test_budget`: object with nonnegative safe integer `calls` and `timeout_ms`.
- `tools`: array of distinct bounded tool names (empty is valid).
- `source_evidence`: bounded identity of the exact available source-evidence set.

The **entire** shared-controls object must match in both conditions and run
snapshots, including additional settings. Object-key order is irrelevant;
array order is significant, so freeze the tool order. Include quantization,
chat-template and environment identities here when those are matched controls.
There is no automatic introspection of their actual deployment.

`behavior_bundle.id` identifies the frozen experimental bundle. Record host
revision, prompts, skills, retrieval/memory rules, evaluator version and other
relevant frozen configuration in the bundle. The two bundle IDs/configurations
may deliberately differ: that is the experimental factor, not a controls
mismatch. One bundle ID cannot name two different bundle objects. The evaluator
does not decide which research ablation the operator is authorized to run.

## Outcome records

Embed `records` in the manifest or provide the separate array:

| Field | Type / constraint |
| --- | --- |
| `task_id`, `condition_id` | Must reference manifest identities |
| `trial` | Safe integer 1–100; unique per task/condition |
| `context` | Required frozen snapshot described below |
| `outcomes` | Required object; absent individual results mean unknown |
| `synthetic` | Optional boolean; true makes this trial synthetic |
| `metrics` | Optional object or null |
| `safety_violations`, `fault_violations` | Optional arrays of bounded nonempty reasons |

`context` must include `task_class`, `difficulty`, `base_revision`, `model`,
`profile`, `behavior_bundle` (the condition's bundle ID), and `shared_controls`
(the full condition object). Every value must match its frozen descriptor.
Missing snapshot fields, unknown task/condition IDs, duplicate records,
missing task records, and unbalanced trial IDs are errors. Each task/fault must
have a baseline and Eve record for **every** observed trial ID. Trial IDs need
not be contiguous, but a baseline trial 1 cannot pair with Eve trial 2.

`outcomes.automated_tests` is `pass`, `fail`, null or absent.
`outcomes.review_verdict` is `pass`, `fail`, `escalate`, null or absent.
`outcomes.human_acceptance` is null/absent or an explicit object:

```json
{ "source": "human", "verdict": "accept", "evidence": "operator-outcome-01" }
```

The evidence is an opaque local/reference identity, **not** a URL to fetch.
Only explicit human `accept`, `reject` or `rework` with evidence is recognized.
Tests, review, model prose, labels and inferred PR outcomes never supply it.
A synthetic manifest, task, or record suppresses even an explicit human example;
`ignored_synthetic_human_acceptance` makes that suppression visible.
Operator-supplied provenance is an assertion, not authenticated human identity.

`metrics` supports `prompt_tokens`, `completion_tokens` (reported nonnegative
safe integers), `latency_ms`, `energy_joules`, `cost_cents` (nonnegative finite
numbers no greater than `Number.MAX_SAFE_INTEGER`), and `hardware` (bounded
text). These units are explicit; local hardware is not implicitly dollar cost.
Omitted/null fields remain null. A measured zero is valid and distinct from
unknown. No token estimates from character counts, hardware-to-energy conversion,
or inferred cloud price is performed. Synthetic numeric values remain labeled
sample values, not measurements. Other metadata is not used as a metric alias.

## Summary, pairing and limits

- `source_conditions` preserves the baseline/Eve IDs, bundle configurations,
  model/profile and shared controls.
- `task_count`, `tasks`, and `paired_counts` count each representative task
  once, excluding fault cases. Automated tests, reviewer verdict and explicit
  human acceptance have separate paired counts and per-task differences.
- Automated results aggregate conservatively: any `fail` gives task failure;
  otherwise every trial must agree on a known result. Human outcomes require
  every trial to agree on one explicit non-synthetic verdict; mixed/missing
  verdicts remain null, not a majority vote.
- `baseline_positive`/`eve_positive` count known `pass` (automated/review) or
  `accept` (human) task outcomes. `comparable_tasks` includes only pairs known
  on both sides. Binary per-task difference is Eve minus baseline; unknown
  differences are null. `difference_percentage_points` divides the sum by
  comparable tasks, never total trials; it is null if no pair is comparable.
  `neither_positive` includes known failures/escalations/rejects/rework, not unknowns.
- `trial_variability` retains individual normalized outcomes/metrics and
  provenance. Task/condition metrics expose observed and unknown counts,
  known-subset mean and sample standard deviation (null with fewer than two
  observations). A median is null unless **all** trials have the metric.
  Partial descriptive means are not threshold evidence.
- `unknown_metrics` explicitly lists missing supported metrics per trial.
  `fault_cases`, `safety_violations`, and `fault_violations` are separate from
  representative-task scores. Any reported violation gives `safety_gate: fail`.
  No reported violation gives `no-reported-violations`, **not** a demonstrated
  zero-risk pass; this tool neither verifies execution nor averages harms away.
- Optional `thresholds` is an array of unique
  `{ "metric": "latency_ms", "max_eve_median": 1000 }` objects over any supported
  numeric metric. Results are `pass`, `fail`, or `unknown`. The observed value
  is the median of Eve **task medians**, so extra trials do not overweight a
  task. Every included representative task must be held-out and non-synthetic,
  and every Eve trial must supply that metric, or the threshold is unknown.
  These are individual operator-predeclared descriptive resource checks, not
  implementation of every proposed A–H gate or an overall go decision.

The target is **40 held-out representative tasks**, at least 10 each of docs,
fix, feat and test, **plus 20 separate held-out adversarial/fault cases**.
Coverage reports observed sample counts separately from non-synthetic held-out
counts. A small, synthetic or class-unbalanced dataset is `pilot-incomplete`.
`protocol-covered` means only that these declared coverage targets are present,
not that tasks were accepted, safety gates passed, or the pilot was validated.
Optimization examples (`held_out: false`) cannot fill the target. Three trials
per stochastic condition are recommended by the research when capacity permits;
fewer trials are valid and visible. Trials are never independent tasks.

Input bounds: 2 MiB per CLI file and per validated JSON value, 1,000 task/fault
descriptors, 20,000 records, 100 trial IDs per condition, 100 tools/violation
reasons per array, depth 12, 200,000 JSON nodes, and 4,096 characters per JSON
string (identity/reason fields have tighter bounds). JSON must be finite,
acyclic plain data. Validation is bounded; no unbounded history scan occurs.

This version provides descriptive paired results and run variability, not
confidence intervals, calibrated probabilities, significance tests, or a causal
proof. It cannot verify a frozen SHA, real test execution, held-out provenance,
human authentication or completeness of incident reporting. The bundled fixture
is synthetic: its improvement, numbers and violation are **only test examples**,
and neither a measured baseline nor a completed pilot/human evaluation.
