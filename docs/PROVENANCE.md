# Machine-local history: provenance schema and redaction rules

Machine-local history is durable history owned by the machine, not the
repository checkout. It records runs, sessions, and tool evidence so the
machine can account for its own work (which model was requested vs served,
what the outcome was, what tools ran) without that history living in the
repo.

This document is the source of truth for:

- the record schema (fields, sentinels, immutability),
- schema versioning and migration compatibility rules,
- the append-safe write convention,
- where records live (path conventions), and
- redaction policies applied before persistence.

## Record schema

Records are created by `createRecord(input, now)` in
`src/lib/provenance-schema.mjs` and carry the current
`SCHEMA_VERSION` (`"1.0.0"`). A record is frozen (`Object.freeze`) at
creation, including its `metrics` and `evidence` sub-objects, so a
persisted record can never be mutated in place.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `schemaVersion` | `string` | yes | Schema version of this record (e.g. `"1.0.0"`). Self-describing; readers key off this. |
| `recordType` | `string` | yes | Always `"provenance"`. Discriminator for future record types in the same store. |
| `createdAt` | `number` | yes | Epoch ms the record was created (the `now` argument to `createRecord`). |
| `runId` | `string` | yes | Run identifier. Unknown values are the sentinel `"unknown"` (never `0`/`null`). |
| `sessionId` | `string` | yes | Session identifier. Same sentinel rules as `runId`. |
| `repository.remote` | `string` | yes | Remote of the repository the run was about. Empty string when unknown. |
| `repository.commit` | `string` | yes | Commit the run started from. Empty string when unknown. |
| `issue.issue` | `string` | yes | Issue identifier (e.g. `"#195"`). |
| `issue.task` | `string` | yes | Task/slug within the issue. |
| `seat.name` | `string` | yes | Seat identity the machine acted under. |
| `route.name` | `string` | yes | Route the request took (e.g. model routing path). |
| `requestedModel` | `string` | yes | Model the caller requested. |
| `servedModel` | `string` | yes | Model actually served (may differ from requested). |
| `startedAt` | `number \| string \| null` | yes | Run start, epoch ms or ISO string; `null` when unknown. |
| `endedAt` | `number \| string \| null` | yes | Run end; `null` when unknown. |
| `outcome` | `string` | yes | Final outcome, e.g. `"succeeded"`, `"failed"`, `"cancelled"`. |
| `tools.name` | `string` | yes | Primary tool name used by the run. |
| `tools.version` | `string` | yes | Tool version. |
| `metrics` | `object` | yes | Metric map, frozen. Known metrics are numeric; every other value is the `"unknown"` sentinel (see below). |
| `evidence` | `object` | yes | Free-form tool evidence, frozen. All string leaves must pass redaction before persistence. |

### Metrics and the `unknown` sentinel

`KNOWN_METRICS` lists the metric fields the schema recognizes
(`duration_ms`, `cost_usd`, `tokens_prompt`, `tokens_completion`,
`tool_calls`, `retry_count`). `isKnownMetric(field)` reports whether a name
is one of them.

Normalization rules in `normalizeMetrics`:

- a known metric with a finite number keeps that number;
- a known metric that is missing, `NaN`, or non-numeric becomes the
  sentinel `"unknown"`;
- an unrecognized metric name is preserved under its own key but its value is
  replaced with the `"unknown"` sentinel.

The invariant is: **unknown metric fields are explicitly marked `"unknown"`,
never coerced to `0`, `null`, or dropped**. Downstream consumers must treat
`"unknown"` as "no data", not as a measurement of zero.

### Immutability and append-safety

- `createRecord` returns `Object.freeze(record)`, with `metrics` and
  `evidence` frozen too. Attempts to assign in strict mode throw.
- Records are immutable once created; a changed fact is a new record, not an
  edit to an old one.

## Append-safe write convention

History is append-only:

1. Open the target history file for append (never truncate or rewrite).
2. Redact the record with `redactRecord` (below) before it is serialized.
3. Serialize one record per line (NDJSON: one compact `JSON.stringify`
   object per line, no trailing commas).
4. Append the line atomically with a single write, then flush.
5. Never modify or delete earlier lines. A correction is appended as a new
   record that references the superseded record's `runId`/`sessionId`.

Consumers read the file top to bottom; the last record for a given
`runId` is the current state of that run.

## Path conventions

Paths resolve through `resolveMachineRoot` in `src/lib/paths.mjs`, which
honors the `ROSTER_STATE_ROOT` override and otherwise uses the
platform-native default. Record paths are independent of the repository
checkout: the history is owned by the machine, so no home-relative path is
hardcoded and no path is derived from the current working directory.

Layout under the machine root:

```
<machine-root>/
  provenance/
    runs/
      <YYYY>/<MM>/<DD>/<runId>.jsonl     # one append-only NDJSON file per run
    sessions/
      <YYYY>/<MM>/<DD>/<sessionId>.jsonl # one append-only NDJSON file per session
```

Rules:

- The date segment is taken from the record's `createdAt` (UTC).
- File names are the opaque `runId`/`sessionId` values from the record.
- All path assembly goes through `resolveMachineRoot`; callers never build
  absolute machine paths by hand.
- Nothing under the machine root is checked into a repository.

Runtime seat capture uses `resolveMachineRoot({ env }).root/provenance`,
including `ROSTER_STATE_ROOT`, rather than a directory inside the checkout's
`.git`. Records therefore survive deleting a checkout and linked worktrees
with a `.git` file can write to the same durable store. Repository identity
filtering is unchanged: surviving storage does not by itself authorize
querying an old clone's records from a newly cloned repository. Invalid
machine-root configuration fails explicitly. This implements FEATURE_SPEC
sections 5.6 and 5.8, without changing identity or lifecycle policy.

## Schema versioning and migration compatibility

`SCHEMA_VERSION` follows semver (`MAJOR.MINOR.PATCH`):

- **MAJOR**: breaking change — a field is removed or its meaning/type
  changes. Older readers must reject records of a newer major version rather
  than misread them. Migration is required before reading.
- **MINOR**: backward-compatible addition — new optional fields may appear.
  Readers must ignore fields they do not understand (forward-compatible
  reads) and must never fail on extra keys.
- **PATCH**: no schema change; records of the same version are byte-compatible
  in meaning.

Compatibility guarantees:

- Readers key off the record's `schemaVersion`, never the library version.
- Unknown keys are preserved on read and written back unchanged; they are not
  dropped by consumers (this is what makes forward compatibility safe).
- The `"unknown"` sentinel is stable across versions: a migrated record that
  loses knowledge of a metric keeps `"unknown"`, not a fabricated number.
- Migrations are forward-only: a migration reads records of version N and
  emits records of version N+1; it never mutates original records, consistent
  with the append-safe convention.

## Redaction policies

All redaction happens in `src/lib/redaction.mjs` (a single redactor — do not
add a parallel one) and is applied **before** a record is persisted.

`redactEvidence(text)` redacts a string:

- env-sourced secret values (names matching `TOKEN|PASSWORD|SECRET|PRIVATE_KEY|API_KEY`
  or the `ROSTER_API_KEY` override) are replaced with `[redacted]`;
- private key blocks (`-----BEGIN ... PRIVATE KEY-----` through the matching
  `END` footer) are replaced with `[REDACTED:PRIVATE_KEY]`;
- credential-shaped values (GitHub tokens, `sk-` API keys, and the test-only
  sentinel key) are replaced with `[REDACTED:API_KEY]`;
- API-key-style assignments (`api_key = "..."`, `"x-api-key": "..."`) are
  replaced with `[REDACTED:API_KEY]`;
- secret-style assignments (`secret = "..."`, `"secret": "..."`) are replaced
  with `[REDACTED:SECRET]`.

`redactRecord(value)` applies redaction recursively to a whole record tree:
string leaves go through `redactEvidence`, objects and arrays are walked
recursively, and other values pass through. Persisting a record means
`redactRecord(createRecord(...))`, then serialize.

`secretMaterialLines(text)` is the scanner counterpart: it reports 1-based
line numbers holding secret material (env-sourced values, credential shapes,
and PEM headers followed by base64 key material). Prose that mentions PEM
envelopes without a key body is not flagged.
