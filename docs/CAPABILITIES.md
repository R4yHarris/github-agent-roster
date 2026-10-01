# Capability priors, not benchmarks

The tracked [capabilities example](../examples/capabilities.yml) is a
**starting guess** for first-run routing when no human evaluations exist.
It is not a benchmark, verified model capacity, leaderboard, or performance
promise. No internet scores are scraped. A prior cannot register an endpoint
or make a model eligible without that model in the private [fleet](FLEET.md).

Each record targets either a `profile_id` or `model_id`, not both, and one
task class:

```yaml
capabilities:
  - profile_id: default
    task_class: fix
    suggested_difficulty: 2
    context_max: 8192
    concurrency: 1
    notes: "Local starting guess; revise after human evaluations."
```

`task_class` is `feat`, `fix`, `docs`, or `test`; `suggested_difficulty`
is an integer 1-5. Context and concurrency are positive safe integers.
Notes are short single-line text without IP addresses. Repeated selectors
may describe different task classes, but a selector/class pair is unique.
Fictional model/profile records are illustrative and do not select a live
service automatically.

The operator-supplied `deepseek-v4.1-flash` prior is a strong coder with
`context_max: 1048576` for every task class. A suggested difficulty of 4-5
means strong, 3 means standard, and 1-2 means limited; an absent model prior
is unknown. The fleet example describes `spark-4` hardware as 512GB class
but uses an illustrative endpoint, not a configured service or throughput claim.

Every ask selects effort from task difficulty versus its model prior, not from
README filenames or docs mode. Strong models use low for difficulty 1-2;
difficulty 4-5 uses high. Intermediate tasks use medium when not exceeding the prior's
suggested difficulty, otherwise high; unknown models use a conservative
difficulty-2 prior. Local DeepSeek maps medium to high. An explicit effort
override still applies, but a docs slice is always capped at high, including
overrides and failed-review retries. Output-token budgets remain separate.
The coder logs `Drafting at low effort. Model prior: strong.` for that selection.
Prior context is not substituted for measured usage or configured capacity.

[`loadCapabilities`](../src/lib/capabilities.mjs) loads the installed example
first, then overlays ignored `.roster/capabilities.yml` when present.
A matching selector/class can override only specified fields; a new record
must provide every field. Missing optional overlays are normal, but invalid,
duplicate, incomplete, unreadable, non-file, oversized, or symlinked input
fails explicitly. There is no fallback that silently skips malformed data.
For another project, keep this private overlay ignored by Git too.

The routing policy uses priors for first-run hints, then prefers local
human evidence once at least **three distinct evaluations for that class**
qualify for a fleet model. `roster recommend` is read-only; execution routing
is opt-in through `roster run --auto-model`. Concurrency is only a weak
tie-break, not a throughput benchmark or instruction to launch parallel seats.
Passing tests never create a human acceptance. See [learning](LEARNING.md)
for the evidence threshold and defect-to-reject rules.
The [implemented selector](ROUTING.md) prints `source=prior` when using
these guesses and `source=evals` for qualifying human evidence. Unknown
catalog context is never filled in from a guessed prior for AI-Run or a
positive required context threshold.

Run `node --test tests/capabilities.test.mjs` to validate example and overlay
behavior. Loading priors contacts neither a model endpoint nor the internet.
