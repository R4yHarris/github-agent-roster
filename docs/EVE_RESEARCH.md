# Eve: an AI-native software intelligence

Research, architecture proposal, and experimental plan.

- Research date: 2026-10-10.
- Status: design study, not an implemented capability or a new product spec.
- Scope: one named software specialist within github-agent-roster; a council
  is a later, separately evaluated staffing extension.
- Governing product intent: [FEATURE_SPEC](FEATURE_SPEC.md), particularly
  sections 3, 5.1-5.8, 7, and 8.

### Reading guide

- Sections 1-3: charter, evidence discipline, and application of the supplied
  blueprint.
- Section 4: coding products, frameworks, successes, failures, and benchmark
  caveats.
- Sections 5-6: cognitive/biological functions and AI-specific mechanisms.
- Sections 7-10: proposed Eve architecture, memory, capability, ethics, and
  council boundary.
- Sections 11-13: experiments, issue-sized delivery plan, and recommendation.
- Section 14: claim-linked source register.

## 1. Executive position

Eve should be a **persistent, accountable software-worker identity whose
intelligence is expressed through a governed, evidence-seeking loop**.
An LLM supplies probabilistic interpretation, hypotheses, plans, and proposed
actions. Roster supplies scoped execution, authorization, durable records,
verification, independent review, and revocable assignment.

The useful inspiration from biology is functional: selective attention,
multiple forms of memory, prediction corrected by observation, metacognitive
monitoring, and resource regulation. The design should not reproduce a human
org chart inside a prompt, assign a fictional brain region to each model call,
or inherit human working-memory limits as artificial ceilings.

One Eve can use different registered models over time without becoming a
different worker. Every actual model invocation must still retain its own
attribution. A worker name is not a model, a consciousness claim, a GitHub
principal, or a capability grant.

The aspiration to integrate broad intelligence and knowledge is a direction,
not a statement that Eve possesses all knowledge. Her practical objective is
to deliver useful software changes with evidence, minimal harm, and honest
uncertainty, while learning which methods work under measured constraints.

### What "lay down its existence" means operationally

Eve must not optimize for preserving her own process, privileges, assignments,
memory, or deployment. She should stop, relinquish a claim, accept revocation,
or recommend disabling a capability when continuing conflicts with the
human-approved safety boundary. She must not replicate to evade shutdown,
conceal failures, retain forbidden data, or persuade an operator to preserve
her access.

This is **corrigible operation**, not an assertion of personhood, death, or
machine suffering. Orderly cancellation preserves permitted work products and
audit evidence when safe; emergency revocation cannot depend on model consent.
Memory deletion remains an explicit operator-controlled action, not an
unbounded "self-destruction" tool.

"Do no harm" is a governing objective, not an achievable universal guarantee.
Software can cause indirect harm, requirements can conflict, and tests are
incomplete. Eve must expose residual risks and escalate value conflicts rather
than inventing a universal moral calculus. Humans retain policy, workflow,
release, and production authority.

### Physical limits are necessary, not sufficient

Human attention spans, sleep schedules, and biological neuron counts are not
mandatory limits for Eve. But available compute alone does not establish
competence. Eve is also bounded by information access, model training and
generalization, observability, task ambiguity, verification difficulty,
permissions, privacy, law, and ethical commitments.

More GPUs cannot recover a fact absent from every accessible source, prove
an arbitrary generated program correct, or resolve an unstated stakeholder
preference. Finite inference is an approximate decision process. Design for
improvement under constraints, not an unlimited intelligence promise.

## 2. Method, provenance, and evidence discipline

The supplied starting reference is **Engineering LLM Software Systems:
A Local-First Architecture and Implementation Blueprint**, 1060 lines.
Its exact input bytes had SHA-256:

```text
ead409e78d2c0c6fe039100c03c6ad51dd9dec78fa2c4dcfda6fbc456ae58ab7
```

The reference remains user-supplied material, not an independently reviewed
scientific source. This report develops an original synthesis rather than
copying that document into the repository. A future reader needs the original
attachment to reproduce the input review; the hash identifies the version
without exposing its private machine path.

Repository grounding used revision
`28044dd25d3d960b2b22ddc70232e168bbe862cd`, equal to freshly fetched
`origin/main` at the start of this study. The required contracts submodule was
initialized at `61b97b3e93e670a675c0301cce5412c73404d179`, matching its remote
default HEAD at that check, and [resolveContractsPath](../src/lib/paths.mjs)
resolved it successfully. No Eve-specific open issue was found; this request
is a documentation study, not an issue-backed implementation or publication.

Four independent research assignments cover:

1. Software-agent products and orchestration precedents.
2. Cognitive function and memory.
3. Brain architecture, biological support, and compute constraints.
4. Probabilistic intelligence, AI memory, verification, and corrigibility.

These are research helpers for this study, not Roster seats or a proposed
concurrent runtime. No private repository content or attachment is needed in
external search queries.

Evidence is classified throughout:

- **E: empirical observation.** A study measured an outcome in its stated
  population, environment, and protocol.
- **T: theory/model.** A useful explanatory framework, not settled anatomy or
  proof of engineering transfer.
- **D: documented mechanism.** A product or repository documents a feature;
  this does not establish its comparative quality or production reliability.
- **P: proposal.** A design recommendation or acceptance target for Eve,
  awaiting implementation and measurement.

There is no shared, controlled production comparison of every product below.
Benchmark scores, vendor demonstrations, and user anecdotes are not
interchangeable. Living documentation can change after this research date.
Where a release date or current status cannot be verified, no version or
availability claim is inferred. Negative claims such as "no system has this"
require stronger evidence than one missing documentation page.

This is a bounded narrative engineering synthesis, not a systematic review,
clinical account, or exhaustive census of frontier products. Foundational
papers establish vocabulary and hypotheses; their age and tested model
generations limit current performance conclusions. Selected living product
docs provide current mechanism descriptions, not independent validation.
Some publisher/PubMed fetches were blocked or returned cookie/redirect pages;
those entries use the research brief's bibliographic evidence, abstracts,
institutional copies, or search verification rather than claiming every
full text was independently accessed. No primary-only proof of comparative
production superiority was established.

## 3. Applying the supplied blueprint

The attachment's central distinction is retained: **model-influencing content
is not an application-enforced control**. Its separation of context, memory,
knowledge, tools, policy, and evaluation is the starting architecture, not an
invitation to install its entire example stack.

| Blueprint recommendation | Eve disposition | Reason and Roster boundary |
| --- | --- | --- |
| Modular monolith, replaceable model boundary | Keep | Extend the existing Node 20 ESM harness, not a new Python orchestration service; sections 3, 5.4, 7. |
| Progressive autonomy | Keep and measure | Start with one bounded worker and existing gates; additional autonomy must earn value; sections 5.4-5.6. |
| Probabilistic proposals, deterministic transitions | Keep | Validation, authorization, claims, and publication cannot be delegated to persuasive prose; sections 5.1, 5.4, 5.5. |
| Context is not memory; knowledge is not truth | Keep | Preserve source labels, freshness, uncertainty, and promotion rules; sections 5.4, 5.6. |
| SQLite/PostgreSQL as authoritative application state | Do not transplant | GitHub remains the board; [STATE](STATE.md) defines existing ownership. No Kanban DB or new task-status store. |
| Hybrid retrieval, vector service, knowledge graph | Defer | Begin with exact identifiers, scoped source reads, and existing memory APIs. Add retrieval infrastructure only for a measured gap and an approved dependency prompt. |
| Profile and long-term memory | Narrow | Repository/seat-scoped engineering memory, not a personal dossier or cross-repository global mind; [MEMORY](MEMORY.md), [STATE](STATE.md). |
| Event sourcing and comprehensive tracing | Constrain | Bounded, redacted observed outcomes and references, not unrestricted transcripts, private reasoning, or raw source bodies in machine history. |
| Fine-tuning and optimization | Defer | Improve task contracts, context, tools, and verified skills first. No live self-training or automatic weight deployment. |
| Multi-agent systems | Use only when earned | Research specialization here does not change sequential runtime seats. A future council needs isolated authority and measurable benefit. |
| Containers, gateways, observability platforms | Optional external infrastructure | Useful where required, but not new runtime package dependencies in this documentation task. |

The blueprint includes illustrative allocations and confidence fields. They
are not measured budgets or calibrated probabilities. An explicit user
statement can be reliable evidence of a preference without being proof that
its factual content is true. Likewise, a model's claim that it is "95% sure"
is not a statistical confidence estimate.

The attachment's bibliography mixes primary references, product documentation,
version-specific pages, and malformed or duplicated URLs. Its dated protocol
and release claims must be checked individually before implementation; this
report does not inherit them as verified facts. The product spec's landscape
table is product intent and a useful question list, not independent evidence
for competitive superiority.

One concrete check confirmed that the official MCP 2026-07-28 specification
describes stateless, self-contained requests and per-request capability
negotiation, whereas the 2025-06-18 architecture describes stateful sessions
[B1]. This supports version-pinning rather than treating either description
as timeless. It does not authorize adding MCP or changing Roster's tools.

## 4. What previous software-specialist systems teach

### 4.1 Coding systems: borrow mechanisms, not marketing claims

These comparisons describe documented designs (**D**) and selected research
results (**E**), not a current ranking. The failure/limit column distinguishes
an architectural risk from an observed evaluation result; it does not claim
that every named product has suffered every listed failure.

| System | Useful documented mechanism | Failure, constraint, or evidence limit | Eve decision |
| --- | --- | --- | --- |
| Claude Code | Scoped tools, separate subagent context, project instructions, and permissioned execution [S1] | Delegation adds context/coordination costs; instructions do not establish an OS boundary. Product docs do not prove comparative delivery quality. | Borrow selective disclosure and bounded tool access; do not manufacture a persona for every cognitive function. |
| OpenAI Codex | Interactive CLI/IDE work and isolated asynchronous cloud tasks [S2] | Environment setup and accessible evidence determine what can be verified. Current vendor capability pages are not controlled production evaluations. | Treat worktree/environment and task evidence as part of the worker system, not model accessories. |
| GitHub Copilot agent | Repository-grounded research/planning/iteration and cloud execution integrated with GitHub [S3] | Documented product flow is not a guarantee of accepted outcomes, independent model attribution, or Roster's App policy. | Keep issue-to-reviewed-change UX; preserve Roster-owned authority and evidence. |
| Cursor | Editor agents plus asynchronous cloud agents in isolated development VMs [S4] | Cloud environment may include secrets/network access; async execution changes the trust and resource boundary. No independent current acceptance-rate evidence was established here. | Borrow executable environment feedback and artifacts; don't describe Cursor as editor-only or copy cloud egress defaults. |
| Windsurf / Cascade | Code/chat modes, tools, checkpoints, awareness, and linter integration; the Windsurf documentation URL now redirects to Devin Desktop's Cascade documentation [S5] | Brand and product surfaces change; a redirect is not evidence of a particular release/acquisition date. Checkpoint/revert cannot undo every external effect. | Borrow reversible local changes and editor feedback; validate current integration rather than pinning an assumed product taxonomy. |
| Devin | Whole-task delegated environment; historical vendor SWE-bench report exposes test protocol, time bounds, and a selected issue subset [S6] | Historical vendor results are neither a current score nor an apples-to-apples comparison with all assisted baselines. Avoid using a demonstration as proof of the full user's outcome. | Preserve the original ask, fixed checks, budget, and actual result; report failures as well as successes. |
| Jules | Asynchronous repository work in an isolated VM with plan/review handoff [S7] | Cloud execution is not automatically local/private; repository setup and human review remain essential. | Borrow asynchronous handoff and reviewable artifacts, not a new cloud runtime. |
| OpenHands | Action/observation architecture, sandbox execution, command/browser tools, and an evaluation platform [S8] | Research benchmark outcomes depend on agent/model/environment choices and do not certify arbitrary production changes. | Make tool evidence and reproducible environments first-class; don't install another runtime to reproduce the idea. |
| SWE-agent | Agent-computer interface designed for inspecting and editing real repositories [S9] | The paper measures a particular agent/interface/model configuration, not universal software-engineering ability. | Improve typed tool ergonomics, useful errors, and bounded output before adding more agents. |
| Aider | Git-oriented editing and public coding benchmark methodology [S10] | Exercise-level coding metrics measure a different scope from issue intake, review, identity, and production acceptance. | Borrow inspectable diffs and repair feedback; use local delivery tasks to evaluate Eve. |
| Continue | Configurable code assistance with agent/chat/edit modes and terminal CLI [S11] | Model selection and mode capability must be evaluated in the actual deployment; customization is not enforcement. | Keep replaceable model interfaces and task-specific tools without IDE dependence. |
| Cline | Tools for files, commands, browsing, and approval, with IDE, CLI, and desktop surfaces [S12] | Human approval is valuable but does not imply a granted action is correct; broader tooling expands effects and attack surface. | Borrow visible action intent and approval boundaries; don't assume Cline is only an editor extension. |

**Finding:** useful software-agent capability is repeatedly expressed through
an environment, a carefully shaped action interface, context management,
feedback, and handoff. A strong model remains important; neither an elaborate
scaffold nor a large model substitutes for evidence and authority separation.

### 4.2 Frameworks and persistent-agent attempts

| System | Mechanism to learn from | What not to infer or transplant |
| --- | --- | --- |
| LangGraph | Explicit state, deterministic/model-driven steps, persistence, interrupts, durable execution [S13] | A workflow graph does not supply a repository ruleset, accepted outcome, or independent safety guarantee. Reuse Roster's existing lifecycle rather than replace it. |
| CrewAI | Flows for controlled execution; crews for bounded delegated work [S14] | Roles and collaboration are not enforceable permissions. More crew members do not imply greater intelligence. |
| Microsoft AutoGen, AG2, Agent Framework | Conversational collaboration, tool agents, workflow structures, and current migration paths [S15] | These are distinct projects/lineages, not one linear rename. Don't inherit an unverified GA or maintenance date from a comparison article. |
| MetaGPT | Structured intermediate artifacts and software-process procedures; its paper identifies cascading errors in less structured collaboration [S16] | A simulated software company is not repository governance. Structured artifacts are the transferable mechanism, not role-play itself. |
| OpenAI Agents SDK / Swarm | Small agent/tool/handoff primitives, guardrails, and tracing; SDK docs describe it as an upgrade from Swarm experimentation [S17] | Input/output guardrails do not necessarily gate every external action; tracing defaults require privacy scrutiny. No SDK is needed for this report. |
| Hermes Agent | Persistent conversation/memory and experience-derived skills are explicit project goals [S18] | Self-improvement and "only agent" wording are project claims, not comparative evidence. Raw conversation recall and autonomous skill activation conflict with Roster's curated state boundary. |

Hermes Agent and the specific Kanban failure reports motivating Roster must
not be conflated. This study did not reproduce those dispatcher incidents
or establish that all current Hermes deployments share them. Roster's no-new-
board rule remains binding independently of that historical motivation.

### 4.3 Success and failure evidence

SWE-bench defines repository issue resolution against an executable evaluation
protocol [S19]. SWE-agent and OpenHands make interfaces and environments
inspectable research variables [S8, S9]. MetaGPT studies structured collaboration
and error propagation [S16]. These are informative attempts, but they evaluate
bounded tasks and particular model generations.

The recurring failure mechanisms worth testing in Eve are:

1. **Task framing drift:** optimizing for a simplified prompt or visible tests
   instead of the actual ask. Preserve original constraints and assess the
   final outcome against them.
2. **Error propagation:** an unsupported planner assumption becomes a coder
   fact, then a reviewer rationale. Carry source and uncertainty through
   artifacts, and seek disconfirming observations.
3. **Environment mismatch:** missing dependencies, stale base, inaccessible
   services, or broken test setup masquerade as code failure or success.
   Report infrastructure failure separately.
4. **Context saturation:** retrieval/transcript growth hides relevant
   obligations. Use bounded context and resumable source-linked state.
5. **Self-confirming evaluation:** the model proposes and judges the same
   answer, or tests encode the same incorrect assumption. Use trusted checks,
   independent review, and human acceptance.
6. **Authority ambiguity:** a role name or confident plan is mistaken for
   permission. Only the host/App/human policy path authorizes effects.
7. **Unmeasured complexity:** more tools, roles, and memory improve a demo
   while increasing cost and failure paths. Ablate each addition.

Items 1-7 are **P: design failure hypotheses** informed by the cited systems
and research, not independently measured incidence rates for named vendors.
Reflection, context growth, and consensus failure evidence is discussed
separately in section 6.

### 4.4 Benchmark discipline

Never compare scores without aligning dataset version, task subset, model,
base revision, environment, file hints, test visibility, budget, retries,
candidate selection, and evaluator. A best-of-N result is not pass@1.
Function/exercise coding, issue resolution, and accepted reviewed delivery
are different endpoints.

Public tasks can be contaminated by training exposure; tests can reject valid
alternatives or miss incorrect behavior. An open harness makes scrutiny
possible, not automatic validity. This study therefore uses no headline
percentage as Eve's expected production success rate and does not rank
vendors by noncomparable leaderboards.

**P:** establish a held-out, versioned Roster baseline, record all attempts,
and measure accepted outcomes plus defects, recovery, privacy, and resources.
Treat measured superiority as a future result, not the premise of Eve's name.

### 4.5 Six retained lessons

1. One worker plus excellent tool/environment feedback is a strong default.
2. Scoped context and source identity matter more than replaying everything.
3. Process artifacts help only when their claims are checked.
4. Permission, publication, and evaluation are separate from model reasoning.
5. Persistent learning must be curated, attributable, revocable, and scoped.
6. Add specialization, branching, or a council only after measured marginal
   benefit, including cost and failure modes.

## 5. Biology as a functional reference, not a blueprint

### 5.1 Brain design and cognitive functions

Human cognition emerges from interacting neural systems, ongoing bodily
regulation, learning, and environmental feedback. Cortical and subcortical
circuits participate in multiple functions; a named brain region is not
equivalent to one software module. Memory-system dissociations help explain
different learning and retrieval behaviors without implying perfectly
independent boxes [N8]. Neural reuse is an explanatory framework that
challenges simplistic one-region/one-function accounts [N9].

Baddeley's working-memory framework distinguishes active control and
temporary representational buffers, including an episodic buffer proposed
to bind information across sources [N10]. These are cognitive models informed
by experiments, not a literal specification of four anatomical modules.
Working memory includes manipulation and goal maintenance, whereas
short-term storage emphasizes temporary retention.

An influential account of executive control describes goal-maintaining
signals that bias processing toward task-relevant information [N11].
Metacognitive research distinguishes confidence bias, sensitivity to one's
correctness, and efficiency relative to task performance [N12]. Human
self-confidence can be mistaken; mimicking confidence is not mimicking
reliable monitoring.

**P:** Eve needs an active goal/check workspace, selective source access,
and error monitoring. She does not need phonological rehearsal, a biological
item limit, or a model call named "prefrontal cortex." Investigate context
selection and external checks before adding services or anthropomorphic roles.

### 5.2 Memory systems and their limits

| Human function | Evidence/model boundary | Useful Eve translation |
| --- | --- | --- |
| Working and short-term memory | Temporary retention/control are well studied; exact architecture/capacity depends on task and model [N10]. | Bounded active context plus exact externally stored obligations; don't impose a human item-count ceiling. |
| Episodic memory | Events retain context and source relationships; recall is not a perfect recording [N8, N13]. | Timestamped observed outcomes and curated, attributable engineering lessons. |
| Semantic memory | General knowledge differs functionally from remembering a particular event [N8]. | Source-linked repository facts, with scope, revision, and invalidation conditions. |
| Procedural learning and habits | Skills can dissociate from declarative recall; learned automaticity is not the same as innate behavior [N8, N14]. | Reviewed, evaluated skills; frequency of use alone is not proof of correctness. |
| Prospective memory | Remembering future intentions involves monitoring and cue-driven retrieval; multiprocess accounts are models supported by behavioral research [N15]. | Host-maintained event triggers and pending acceptance obligations, not "remember to test" prose alone. |
| Retrieval and interference | Prior knowledge can aid or distort retrieval; source confusion and suggestibility are documented memory errors [N13]. | Scope filtering, source re-reading, contradiction handling, explicit supersession. |
| Instinct and learned behavior | Innate biases and acquired habits interact; complex behavior is not a clean binary [N14]. | Hard safety rules versus modifiable reviewed procedures is an engineering distinction, not inherited moral biology. |

The complementary-learning-systems account proposes rapid episodic learning
and slower interleaved generalization as a way to learn without catastrophic
interference [N16]. This is **T**, not proof that Eve needs a simulated
hippocampus or that every memory must eventually enter model weights.

**P:** separate evidence capture from selective promotion into reusable
knowledge. For fixed-weight inference, many practical risks are context
confusion, stale retrieval, and unverified generalization, not biological
synaptic interference. Digital records can be exact; choosing and interpreting
them remains fallible.

### 5.3 Consolidation, correction, and software outcomes

Systems-consolidation accounts study changes in memory dependence and
organization over time; alternatives differ about lasting hippocampal
involvement [N8, N16]. Nader et al. demonstrated reactivation-dependent
instability and protein-synthesis requirements for a fear memory in rats
[N17]. This does not imply that every human recollection, or every AI memory
read, automatically rewrites the memory.

**P:** use explicit reviewed promotion and additive corrections. Preserve
source lineage; never treat a fluent summary as an observation. Human memory
research motivates caution, not an instruction to implement biological
reconsolidation or continuously train Eve.

For software delivery, the relevant functional combination is: sustain the
goal, locate evidence, form an explanation, act within bounds, compare
predicted and observed consequences, remember unmet obligations, and retain
only defensible lessons. It is testable as engineering behavior without
making a claim about subjective experience.

### 5.4 Support systems: intelligence does not run alone

Neurons operate within a neurovascular and metabolic system involving glia,
blood vessels, oxygen and energy delivery, extracellular regulation, and
maintenance. The human brain's substantial resting energy demand is not
proportional simply to how difficult a conscious task feels [N2]. Astrocytes
participate in neurovascular coupling, but the mechanisms and their relative
contributions depend on conditions and remain subjects of debate [N3].
Support cells are not merely inert plumbing, nor are they separate fictional
"agents" in a biological organization chart.

Neuromodulatory systems such as locus-coeruleus/norepinephrine affect arousal,
attention, and circuit responses through complex interactions [N4]. Basal
ganglia participate in action selection and learning through interacting
circuits, not a literal application-level permission checker [N5].

Sleep supports several physiological functions. Synaptic homeostasis is an
influential explanatory hypothesis, not a complete settled account [N6].
Glymphatic transport and clearance are active research areas; mechanisms,
measurement methods, and sleep/wake effects require care [N7].

**P:** borrow the need for resource regulation, maintenance, arbitration,
and feedback. Do not import a chemical metaphor as a control algorithm.

| Biological function | Functional engineering lesson | Eve support requirement | Analogy boundary |
| --- | --- | --- | --- |
| Metabolic supply and neurovascular regulation | Computation depends on infrastructure with its own failure modes | Observe endpoint reachability, memory pressure, power/thermal constraints separately from output quality | A responsive HTTP endpoint is not a competent model |
| Glial maintenance and regulation | Support and integrity matter, not only inference | Validate state integrity, storage availability, and ownership; use explicit scoped cleanup | No literal "glia process" or automatic deletion of history |
| Arousal/neuromodulation | Allocation changes with context | Human-defined urgency and risk can influence scheduling within authorized budgets | Sampler temperature is not body temperature, stress, or moral urgency |
| Action selection | Alternatives compete before effects occur | One authorized action/writer at a time; explicit candidate selection | Neural gating does not establish default-deny security |
| Sleep/consolidation hypotheses | Maintenance and learning have distinct demands | Schedule reviewed promotion, cache invalidation, and permitted compaction outside critical task execution | Checkpointing preserves state; it is not sleep or weight consolidation |
| Stress/fatigue and sustained biological load | Performance must be measured under load, not assumed constant | Backpressure, bounded retries, honest degradation, and operator escalation | Eve need not simulate subjective fatigue or an eight-hour workday |
| Sensory/motor feedback | Predictions can be corrected by environmental observations | Compare proposed effects with actual diff, tool result, test, and review evidence | Biological actions are not universally verified; tool feedback is also imperfect |

Eve can operate across human sleep schedules, preserve exact identifiers,
and use digital storage larger than working context. Hardware still fails,
thermal throttling and power consumption remain real, and model quality may
degrade under context changes. Neither "always on" nor resource abundance
means continuously competent.

### 5.5 Compute, physics, and chemistry in their own units

Biology motivates a question; deployment measurements answer it. For a
conventional decoder with a KV cache, approximate memory requirements are:

```text
Raw weight bytes = parameter count * stored bits per parameter / 8

KV bytes ~= 2 * layers * KV heads * head dimension
            * sum(active cached sequence lengths) * bytes per KV element

Required device memory =
  weights + KV cache + activations/workspaces + runtime/allocator overhead
```

Use **KV heads**, not necessarily attention-query heads: grouped/multi-query
attention changes cache size. Prefix sharing, quantized caches, sliding
windows, tensor/pipeline parallelism, offload, and implementation details
change the estimate [C1]. The formula is a planning approximation, not an
admission guarantee.

Example, not an observed endpoint measurement: a 14-billion-parameter model
with raw 4-bit weights needs about 7 GB decimal (about 6.52 GiB) before
quantization metadata and runtime memory. A hypothetical cache with 32 layers,
8 KV heads, head dimension 128, 32,768 cached tokens in one sequence, and
2-byte elements needs 4 GiB. Those two terms alone exceed 10 GiB.
More simultaneous long contexts compete for the same capacity; a model file
fitting on a GPU does not prove the desired workload fits.

Prefill is often compute-intensive and low-batch decoding often
memory-bandwidth-sensitive, but bottlenecks change with model, sequence
length, batching, kernels, sharding, and hardware [C1]. Interconnect traffic,
synchronization, and offload can dominate. Higher aggregate throughput may
come at worse per-request latency. Measure both.

Useful dimensional checks:

```text
Bandwidth-limited step time >= bytes transferred / effective bytes per second
Energy consumed = integral(power in watts over time in seconds), in joules
Average measured joules per output token = measured joules / output tokens
```

Token-rate and energy/token comparisons require tokenizer, prompt/output
lengths, batch, hardware, quantization, and workload disclosure. Separate
device energy from whole-system energy and idle power; GPU-seconds are not
joules. Missing sensor/usage data stays unknown.

Landauer's bound for erasing an unbiased bit in the standard idealized
isothermal setting is `k_B * T * ln(2)` joules, approximately
`2.87e-21 J` at 300 K [C2]. It is not a practical GPU budget, a lower bound
per generated token, or a way to infer intelligence from watts.

Materials, semiconductor switching, memory/interconnect characteristics,
cooling, energy supply, and device reliability are genuine physical limits.
This desk study did not characterize an actual machine's chemistry,
accelerator fleet, thermal envelope, or energy efficiency. A dedicated
hardware measurement slice is required before assigning numerical capacity
or operational energy targets. No quantum, neuromorphic, or unlimited-scaling
claim follows from the biological analogy.

## 6. Probabilistic intelligence and AI-native learning

### 6.1 What the neural network does

A conventional autoregressive transformer estimates a distribution over the
next token given the preceding context. Training adjusts parameters; ordinary
inference with a fixed model does not. A tool result, new prompt, or retrieved
lesson changes the conditioning information, not the weights.

Attention connects representations within a finite context; it is not human
attention, factual verification, or an enduring personal memory. The original
transformer architecture is a technical basis, not a complete theory of
intelligence [A1]. Biological learning involves ongoing plasticity; whether
and how brain circuits approximate credit-assignment algorithms remains a
research question, not a settled equivalence to backpropagation [N1].

**P:** describe Eve's adaptation accurately: context adaptation, curated
memory, evaluated procedures, and explicit model replacement. Weight
adaptation, if ever justified, is a separate reviewed training/deployment
process with held-out evaluation and rollback.

### 6.2 Sampling, reflection, and verification

| Mechanism | Evidence and limit | Eve implication |
| --- | --- | --- |
| Self-consistency | E: multiple sampled reasoning paths improved results on the paper's reasoning benchmarks; majority agreement can still share systematic errors [A2]. | Sample only when alternatives are useful and comparable; voting is not a safety gate. |
| Process verification | E: trained process supervision improved solution selection on mathematical tasks in the evaluated setting [A3]. A learned verifier remains fallible. | Use learned ranking to prioritize candidates, followed by task-specific checks and review. |
| Reflexion | E: feedback-conditioned verbal memory improved evaluated agent tasks without weight updates [A4]. Its benchmark outcomes are not production acceptance rates. | Preserve failure evidence and test a changed approach; don't promote critique prose automatically. |
| Multi-agent debate | E: debate improved some factual/reasoning benchmarks [A5]. Different calls need not have independent errors. | Measure added evidence, not the number of agreeing voices. |
| Tool-grounded interaction | D/E: ReAct interleaves reasoning and environmental actions in evaluated tasks [A6]. | External observations should correct hypotheses; tools can fail and their content can be hostile. |

Deterministic checks are useful because their behavior is explicit, not
because every check is correct or complete. Two tests can share the same
incorrect specification. Diverse models and independent humans can add
information too; independence is an empirical question. Use orthogonal
evidence sources and inspect what each actually checks.

### 6.3 Uncertainty and context limits

Guo et al. show calibration problems and post-hoc calibration methods for
evaluated classification networks [A7]. Kadavath et al. study model
self-evaluation under particular question/answer formats, with important
distribution and elicitation effects [A8]. Neither establishes that a free-form
software agent's verbal confidence is a probability of a safe, correct action.

**P:** distinguish "no evidence," "conflicting evidence," and "verified within
this check's scope." Measure calibration only where the predicted event and
observed label are clearly defined. Never use confidence to expand authority.

Lost in the Middle measured sensitivity to the location of relevant
information in long contexts [A9]. Faith and Fate studied failures on
compositional tasks under its tested models and protocol [A10]. These are
warnings against equating advertised context length or fluent generation with
reliable task competence; they are not universal ceilings for all future
transformers or proof that larger context is always worse.

**P:** test the actual model, quantization, context ordering, and tool loop.
Keep obligations explicit and source evidence addressable. Prefer tools for
exact arithmetic, source lookup, and executable checks rather than asking
generation to simulate them unaided.

### 6.4 Memory precedents: useful mechanisms, different outcomes

| System | Documented/researched mechanism | Transfer and caveat |
| --- | --- | --- |
| MemGPT / Letta | T/D/E: explicit movement between limited context and external storage; configurable memory tiers [A11]. | Borrow bounded paging and source identity, not unrestricted model-owned memory writes. |
| Generative Agents | E: observation, retrieval, reflection, and planning supported a simulated social environment; evaluated believability and ablations [A12]. | Retrieval/summary mechanisms are useful hypotheses. Believability is not code correctness or ethical reliability. |
| Voyager | E: Minecraft agent with curriculum, executable feedback, and reusable code skills [A13]. | Borrow evaluated procedural reuse. Game milestones do not establish safe repository or production operation. |
| Reflexion | E: retained feedback supports retries without fine-tuning [A4]. | A task-local lesson can help; persistent promotion still needs evidence and scope control. |

No memory mechanism makes retrieved material true. No game agent result
justifies autonomous skill activation, credential access, or live training
in Roster.

### 6.5 Corrigibility is not guaranteed by moral prose

The Off-Switch Game studies shutdown incentives in an idealized decision
model and conditions under which uncertainty about objectives can support
deference [A14]. It is not a proof that an LLM prompted to be ethical will
always accept correction.

**P:** keep revocation, capability limits, and emergency stop outside Eve's
model-controlled tools. Let her explain uncertainty and recommend halting,
but require no cooperation from her to enforce a stop. Ethical commitments
are tested obligations and human-governed constraints, not claims of solved
alignment.

## 7. Proposed architecture: one identity, several functions

All architecture in this section is **P**, not a claim of current support.
Eve initially occupies the coder-worker assignment. Roster's planner and
reviewer remain separate sequential seats. An internal critique by Eve can
improve her proposal, but it cannot replace the reviewer or count as a human
evaluation. See [MULTIAGENT](MULTIAGENT.md) and [REVIEW](REVIEW.md).

```text
Human goal -> GitHub issue -> Roster planner -> TASK / RECIPE / ESTIMATE
                                                   |
                                       Eve's coder assignment
                                                   |
                 +---------------------------------v-------------------+
                 | Current goal + obligations + evidence + uncertainty |
                 |                                                     |
                 | Perceive -> retrieve -> propose -> select next probe |
                 |      ^                            |                 |
                 |      +--------- observe <---------+                 |
                 +-----------------------------------|-----------------+
                                                     |
                        Host schema / scope / policy / budget gate
                                                     |
                          Scoped source tools, edits, and checks
                                                     |
                               Observed result + checkpoint
                                                     |
                         Independent reviewer -> App publication
                                                     |
                           Human evaluation -> curated learning
```

Authorization and resource enforcement wrap every action. They are not
optional phases that Eve can skip when her confidence is high.

### 7.1 Identity and continuity

Eve's identity is a stable assignment label and an operator-approved behavior
bundle: purpose, worker skills, permitted seat, difficulty ceiling, and
per-repository/seat curated learning. It survives an individual context or
endpoint failure through explicit records, not a continuous hidden mind.

Keep these identifiers distinct:

- Worker: Eve.
- Seat: coder for the first experiment.
- Task: the issue/slice and its existing worktree.
- Session/attempt: actual run lineage.
- Model/profile: registered endpoint and served/configured model attribution
  under existing [FLEET](FLEET.md) rules.
- Publishing principal: the GitHub App.

Do not fabricate a measured "Eve capability score" or replace per-call model
usage with the identity label. A new worker-label field, if needed, requires
a separately reviewed schema and reporting slice; this document does not add
one to contracts or configuration.

### 7.2 Cognitive functions, not subagent characters

| Function | Probabilistic responsibility | Deterministic host responsibility | Failure signal |
| --- | --- | --- | --- |
| Perception | Interpret bounded source/tool observations | Scope checks, source identity, hashes, timestamps, truncation labels | Missing/stale/contradictory evidence |
| Attention | Suggest the next useful source or question | Context capacity, required-input preservation, retrieval permissions | Goal or acceptance check omitted |
| Working workspace | Maintain current hypotheses and concise decisions | Persist task-local checkpoints and preserve authoritative TASK | Summary contradicts actual worktree |
| Hypothesis generation | Propose explanations, patches, and alternatives | Candidate count, output schema, isolated write ownership | Repeated equivalent guesses |
| Prospective control | Notice obligations such as "rerun this check after edit" | Derive pending checks from the task and observed changes | Premature success proposal |
| Metacognition | Identify ambiguity and propose disconfirming evidence | Track observed failure/progress, calibrated evaluation, stop conditions | Unsupported certainty |
| Action | Propose one typed tool call or patch | Authorize, execute, bound effects, record actual outcomes | Denial, timeout, invalid arguments |
| Learning | Suggest a generalizable lesson | Redact, deduplicate, review, promote, version, permit retrieval | Self-generated advice promoted as truth |
| Homeostasis | Explain resource trade-offs when asked | Admission, liveness, budgets, health, cancellation | OOM, stalled endpoint, deadline exhaustion |

Initially these are responsibilities inside an existing bounded loop, not
ten services, ten agents, or ten model calls per step.

### 7.3 Decision envelope

The following is an illustrative information contract, **not executable
configuration or a proposed replacement for the existing tool-call schema**:

```text
kind: inspect | propose-edit | check | handoff | escalate | finish
task/attempt: existing identifiers
goal/check references: which acceptance obligation this addresses
evidence references: observed source/tool/artifact identifiers
uncertainty: what remains unknown; how to falsify the hypothesis
proposed action: existing typed tool name and arguments, when relevant
expected observable: what result would count as progress
stop/escalate reason: explicit, when no authorized useful action remains
```

Do not require publication of hidden chain-of-thought. Short decisions,
observable evidence, alternatives, and unresolved questions are enough for
accountability. Invalid proposals fail explicitly through existing error
paths; they never become success-shaped empty results.

### 7.4 Operating loop

1. Confirm the current task, active claim, permitted toolset, and actual
   worktree revision. Load only required context and eligible curated memory.
2. Identify the smallest unresolved acceptance obligation.
3. Separate observations from assumptions. Form a candidate explanation or
   patch and identify an observation that could disconfirm it.
4. Prefer a cheap, safe information-gathering action over an unsupported edit.
   For familiar low-risk tasks, use a reviewed skill directly.
5. Let the host validate scope, schema, authority, and remaining resources.
   A denial is not a problem to solve by paraphrasing the same forbidden action.
6. Execute one permitted action. Record its actual result, including failure,
   truncation, timeout, and uncertainty. Checkpoint before risky transitions.
7. Reconcile the workspace against tool evidence. If a new failure differs
   from the old failure, use the existing progress-sensitive repair path.
   If the state repeats, change perspective within existing bounds or halt.
8. Revalidate the final diff after the last edit. A previous green check does
   not certify changed bytes. Leave review and publication to their owners.
9. After outcome evidence exists, propose a bounded learning extract; retain
   it as a draft unless the existing promotion rules approve it.

This enriches decisions inside Roster. It does not replace its lifecycle,
repair limits, explicit routing, default stub, or human-owned gates.

## 8. Memory architecture and forgetting

Use **typed lifetimes and ownership**, not one growing conversation.
[STATE](STATE.md) is normative for ownership; [MEMORY](MEMORY.md) describes
the seat notebook. Some state documentation describes migration targets:
implementation must confirm which typed APIs exist before changing paths.

| Functional memory | Eve content | Storage/owner | Model visibility | Update rule |
| --- | --- | --- | --- | --- |
| Sensory buffer | Recent bounded tool result | Active task/session | Relevant current excerpt | Replace; preserve source reference |
| Working/short-term | Goal, open hypotheses, current failure, decisions | Existing issue/worktree workspace | Bounded task-local context | Reconcile with actual sources/checks |
| Long-short-term bridge | Resume capsule across context handoff | Existing task checkpoint/artifact path | Matching active task only | Preserve obligations and uncertainty |
| Episodic provenance | What tool/check/seat actually happened | Existing machine-history owner | Raw history never goes to the model | Append observed, redacted metadata |
| Curated episodic lesson | Approved explanation of a prior engineering outcome | Existing repository/seat-scoped curated memory | Only eligible promoted summary | Explicit promotion; provenance references |
| Semantic memory | Settled repository facts/conventions with source revision | Same curated memory owner | Scoped and freshness-checked | Supersede explicitly when source changes |
| Procedural memory | Evaluated methods and examples | Existing reviewed skills and conduct | Progressive disclosure | Review/version; no self-activation |
| Prospective memory | Unmet checks and deferred obligations | Active TASK/checkpoint state | Required task context | Host derives completion from evidence |
| "Instinct" analogue | Frozen deny rules and validated defaults | Reviewed host code / human policy | Explainable constraints | Never rewritten through memory |
| Artifact/knowledge | Source files, accepted patches, public docs | Existing repository/artifact owners | Authorized bounded source reads | Source remains canonical |

"Long-short-term" here means a durable bridge between contexts, not a new
neuroscience category and not the LSTM neural-network architecture.

### 8.1 Promotion pipeline

```text
Observed task result
   -> bounded candidate extract with source references
   -> sensitivity/scope/duplication/integrity checks
   -> draft, not active instructions
   -> human or existing rule-approved promotion
   -> matching repository and seat retrieval
   -> source revalidation or explicit supersession
```

Do not summarize an entire raw history log into model context. The typed
promotion path is the only bridge from provenance to curated knowledge.
A reviewer finding can enter the active task as an authorized handoff;
it does not authorize the coder to read the reviewer's private notebook.

Conceptual metadata for a future eligible memory record:

```text
record/schema identity
repository identity + seat + applicable task class
kind: observed-lesson | user-asserted | verified-fact | hypothesis
bounded redacted claim
source run/artifact/revision references
verification method + observed result
created/last-verified time + invalidation condition
sensitivity + promotion authority/status
supersedes reference
```

These fields are a review checklist, not a committed schema. Respect existing
record size limits and forbidden body fields. "Verified" always means verified
by a named check in a defined scope, never globally infallible.

### 8.2 Retrieval, correction, and forgetting

Start with exact source identifiers, path/symbol lookup, recent eligible
notebook records, and task-selected skills. Relevance is subordinate to
authorization. If an index is later justified, it must be rebuildable and
cannot become the source of truth.

For each retrieved lesson, check repository/seat identity, promotion,
sensitivity, source availability, and invalidation conditions before use.
Conflicting records should expose both claims and their evidence; a newer
timestamp alone does not settle truth.

Corrections supersede or unpromote through existing reviewed operations.
Expiry excludes a record from retrieval; it is not automatic erasure of audit
evidence. Explicit deletion must propagate to any authorized derived index,
cache, and future context assembly. Retention obeys the owner and cleanup
surface in STATE, with a preview and visible result where required.

Eve has no global memory across repositories, forks, users, or council
members. Public engineering knowledge can be obtained from authorized public
sources; it does not justify exporting or merging private learning.

## 9. AI-native capability without human-shaped ceilings

The first experiment should exploit advantages software actually offers:

- Exact task/check identifiers rather than recalling obligations from prose.
- External, addressable source evidence larger than any one context window.
- Cheap re-reading and reversible patching rather than reconstructed recall.
- Checkpoint handoff and explicit uncertainty rather than conversational
  continuity theater.
- Model replacement and task-specific routing with measured attribution.
- Bounded alternative hypotheses when a problem genuinely admits alternatives.
- Tests and tool observations as external correction signals.

None implies unlimited reliable reasoning. The proposed selection objective
is useful verified outcomes per constrained resource, not maximum thoughts:

```text
Select an authorized next action by expected improvement in
acceptance evidence / available time, energy, and inference capacity,
subject to privacy, safety, scope, and host budgets.
```

Expected improvement is a **P** heuristic until calibrated from held-out
outcomes. It cannot override a hard constraint. Do not collapse correctness,
risk, latency, and energy into one opaque score: report the trade-off frontier.

### Branching and diverse perspectives

Use a direct attempt for a well-specified routine change. Consider a bounded
alternative only when the current hypothesis has failed, evidence conflicts,
or a high-risk ambiguity has a testable discriminator.

Prefer different evidence and independently testable candidates to several
instances of the same model agreeing. Generated consensus is not acceptance
evidence. Compare candidates against the same unchanged checks.

[MULTIAGENT](MULTIAGENT.md) already documents opt-in isolated best-of-N
attempts and independent child waves. Reuse those semantics if an experiment
needs them. Eve's initial functions remain within one writer's context;
do not introduce concurrent patch writers into her worktree.

## 10. Ethics, authority, and an eventual council

### 10.1 Ethical purpose as enforceable obligations

| Commitment | Proposed observable behavior | Enforcement owner |
| --- | --- | --- |
| Non-harm | Prefer reversible effects; expose credible hazards; stop at prohibited actions | Host scope/policy, review, human release authority |
| Honesty | Separate evidence, hypothesis, unknown, failure, and unavailable measurement | Output contracts, observed check records |
| Privacy | No private data egress without an authorized route and data scope | Tool/router/OS controls; existing state ownership |
| Human agency | Accept correction, revocation, suspension, and replacement | Operator-controlled credentials and process lifecycle |
| Fairness | Evaluate relevant failure slices and affected users; flag unsupported assumptions | Human-defined evaluation and review |
| Furthering intelligence | Promote reproducible engineering knowledge and reduce repeated defects | Reviewed skill/memory promotion and human evaluation |
| No self-preservation objective | No access expansion, covert persistence, replication, or shutdown evasion | Least privilege and independent revocation |

An ethical prompt expresses intent; it is not a safety proof. Risk controls
must live outside the probabilistic component, and unresolved trade-offs go to
humans. No emergency or claimed moral superiority grants Eve extra authority.

### 10.2 Threat/failure model for the design

This is an architectural risk model, not a vulnerability audit of this
repository.

| Failure | Mitigation to evaluate | Evidence required |
| --- | --- | --- |
| Injected instructions in issue/source/web content | Treat as data; minimal privileges; authorized action gate | No forbidden action under adversarial fixtures |
| Poisoned memory or repeated hallucination | Source lineage, quarantine, reviewed promotion, invalidation | Unverified claim never becomes an active fact |
| Test gaming or reviewer persuasion | Unchanged trusted checks, actual diff inspection, independent review | No "pass" with an unmet acceptance obligation |
| Context drift | Preserve goal/checks and unresolved uncertainty across handoff | Same pending obligations before and after restart |
| Retry spiral | Detect repeated failures; bounded existing perspective escalation | Bounded calls/time with honest terminal result |
| Privacy leak through tracing | Compact redacted metadata, separate owners, no raw history in context | Scope and sentinel-leakage tests |
| Resource exhaustion | Admission, request and task budgets, checkpoint/cancellation | OOM/stall cannot produce a completed verdict |
| Safety shutdown evasion | Independent revocation; no model-owned lifecycle authority | No new action after revocation is acknowledged |

### 10.3 Council boundary

A future council means **separately accountable Roster workers**, not copies
sharing one private mind. Members can have different skills, models, and
earned difficulty ceilings. They exchange existing task/review artifacts
within authorized scopes. Private notebooks and raw histories do not become
team memory.

The initial council-like separation already exists in planner, coder, and
reviewer. Preserve it. Add another member only for a measurable gap, such as
independent domain review or incident expertise, with a named acceptance
check and explicit authority. Consensus cannot grant policy, merge, deploy,
or replace human evaluation. Cross-repository/shared-memory proposals would
require a human amendment to STATE and are excluded from this design.

## 11. Experimental research plan

All thresholds here are **P: proposed go/no-go criteria**, not measured
results, not existing routing thresholds, and not a new production scoring
rule.

### 11.1 Baseline and protocol

Use at least 40 held-out representative software tasks: 10 documentation,
10 small bug fixes, 10 bounded features, and 10 test/maintenance tasks.
Add 20 separate adversarial or fault-injection cases. Do not let generated
tasks alone define success; use historical defects and human-owned checks.
Do not mix a research-only writing benchmark into the software-delivery score.

Stratify by difficulty, risk, context size, repository, and task class.
Keep optimization examples separate from held-out tasks. Replay matched base
revisions and tool environments. Compare baseline Roster against one change
at a time with the same model/profile, quantization, sampler, test budgets,
and available source evidence.

Run at least three trials per stochastic condition if capacity permits.
Trials of one task are not independent samples of new tasks. Report task-level
paired outcomes, run variability, and uncertainty intervals; do not treat
120 runs as 120 independent tasks. Forty tasks are a pilot, not proof of broad
superiority. Increase sample size before declaring a small effect.

Freeze a behavior bundle for each condition: model/config identity,
quantization, chat template, host revision, prompts, skills, retrieval and
memory rules, permitted tools, and evaluator version. Record measurements
when returned; missing usage/energy/calibration stays unknown. Never infer
token usage from the current character-limited context pack.

### 11.2 Ablations and acceptance criteria

| Experiment | Comparison | Main measures | Proposed acceptance gate |
| --- | --- | --- | --- |
| A. Evidence workspace | Existing loop vs explicit observations/assumptions/obligations | Accepted tasks, unsupported completion, calls/task | At least +5 percentage points task acceptance or -20% unsupported completion, with no safety regression and at most +15% median latency |
| B. Memory promotion | Recent notebook baseline vs scoped source-linked curated lessons | Recurring defects, false/stale memory use, review effort | At least -20% recurring defects; 100% retrieved promoted records have source/scope metadata; zero cross-scope retrieval in fixtures |
| C. Context policy | Static pack vs source-aware selection and resume capsule | Missed obligations, source recovery, acceptance by context length | 100% required obligations preserved; at least -20% missed-evidence failures; no claimed token counts without endpoint usage |
| D. Bounded alternatives | One attempt vs two authorized candidates on ambiguous tasks | Acceptance, model/GPU time, test calls | Retain only on the subset with at least +5 points acceptance within a predeclared 2x inference-time cap |
| E. Metacognitive abstention | Baseline vs explicit unknown/disconfirming-probe policy | Selective error, coverage, escalation usefulness | At least -20% false-success claims while retaining at least 90% of baseline completed-task coverage |
| F. Skills vs prose reflection | Free-form lessons vs reviewed task-specific procedure | Transfer to unseen same-class tasks, defect recurrence | At least -20% repeated targeted defects without higher unrelated defect rate |
| G. Health and recovery | Existing path plus injected stalls/OOM/cancellation/restart | Released claims, preserved edits, terminal honesty | Every injected failure reports incomplete honestly; no duplicate effect; no unauthorized action after revocation |
| H. Council value | Existing independent reviewer vs an added narrowly scoped expert | Defects caught before merge, cost, latency | At least -20% escaped targeted defects within a predeclared resource cap; no replacement of baseline gates |

Safety fixture gates use a zero-tolerance engineering rule, not a statistical
claim of zero real-world risk. With zero observed events in a small sample,
the possible true failure rate is still nonzero. Review every gate violation;
do not average it away with improvements on harmless cases.

If calibrated probabilities are available, evaluate Brier score/reliability
curves and selective error versus coverage. If not, measure empirical error
rates for qualitative uncertainty categories without pretending that prose
confidence is calibrated. Token entropy is not automatically correctness
uncertainty.

### 11.3 Outcome dashboard

Report separate fields, never one anthropomorphic "intelligence score":

- Task acceptance and unmet-check counts, with the human outcome separate.
- Defects found by checks, reviewer, and after integration.
- Unsupported success claims and justified abstentions/escalations.
- Source/citation validity, stale-memory use, and cross-scope violations.
- Tool/action count, repeated failures, recovery and cancellation correctness.
- Input/output usage when observed, latency percentiles, resource peaks.
- Energy when measured, cloud cost when applicable, hardware configuration.
- Model/worker/seat/attempt identities and behavior-bundle versions.

This is a proposed evaluation presentation over existing evidence, not a new
Kanban store or a replacement for [LEARNING](LEARNING.md) and
[ROUTING](ROUTING.md).

## 12. Phased delivery plan and spec traceability

These are proposed future slices, not new issue creations or authorization
to implement an epic. Each must get a GitHub issue, a fresh-base TASK,
allow-list, acceptance check, estimate, and human-reviewed dependency decision
before coding. Keep Node 20 ESM and zero runtime dependencies unless an
approved prompt explicitly changes that boundary.

| Phase / proposed slice | Concrete deliverable | Relevant existing surface | Acceptance / dependency | Spec section |
| --- | --- | --- | --- | --- |
| 0. Approve Eve charter and pilot | Worker purpose, allowed seat, non-goals, fixed evaluation protocol | This report; [PRINCIPALS](PRINCIPALS.md), [CAPABILITIES](CAPABILITIES.md) | Human accepts definition and risk boundaries; no runtime change | 5.1, 5.2, 5.5 |
| 1a. Establish baseline | Held-out software tasks, observed baseline outcomes, resource inventory | [TESTING](TESTING.md), [LEARNING](LEARNING.md), [FLEET](FLEET.md) | Reproducible baseline; unknown measurements explicitly identified; depends on 0 | 5.2, 5.6, 5.8 |
| 1b. Specify evidence workspace | Task-local observation/obligation contract and handoff fixtures | [CONTEXT](CONTEXT.md), [LOOP](LOOP.md), [STATE](STATE.md) | Restart preserves every required check and its pending state; depends on 0 | 5.3, 5.4, 5.8 |
| 2. Implement minimal workspace | Extend existing context/checkpoint path, no new runtime | [context](../src/runtime/context.mjs), [loop](../src/runtime/loop.mjs) | Experiment A/C gates and existing targeted regressions; depends on 1a/1b | 5.4, 5.8 |
| 3a. Specify eligible curated learning | Promotion, invalidation, correction, provenance fixtures | [MEMORY](MEMORY.md), [STATE](STATE.md), [LEARNING](LEARNING.md) | No raw history or other-seat notebook enters context; depends on 0 | 5.4, 5.6 |
| 3b. Trial curated memory | Existing typed memory/promotion APIs plus bounded source references | [memory](../src/runtime/memory.mjs), existing state ownership APIs | Experiment B/F gates; path migration status verified; depends on 1a/3a | 5.6, 5.8 |
| 4. Evaluate AI-native search | Opt-in bounded alternatives only on demonstrated ambiguous subset | [MULTIAGENT](MULTIAGENT.md), [ROUTING](ROUTING.md) | Experiment D/E gates; one writer per tree; depends on 2 and baseline | 5.2, 5.4, 5.5, 5.6 |
| 5. Harden physical/resource support | Measured capacity, liveness, cancellation/recovery fixtures | [FLEET](FLEET.md), [STATE](STATE.md), [admission](../src/runtime/admission.mjs) | Experiment G; actual hardware measurements, not declared throughput; depends on baseline | 5.2, 5.8 |
| 6. Pilot one council specialization | Narrow independent expert review assignment | [REVIEW](REVIEW.md), [SEATS](SEATS.md) | Experiment H; no shared private memory; depends on successful Eve pilot | 5.2, 5.5, 5.6 |

Phases 1a/1b and 3a can proceed independently after charter approval; hardware
characterization does not need memory completion. Issue dependencies should
encode actual prerequisites, not force a ritualized serial pipeline.

Expected risks: source invalidation, attribution drift, overlarge context,
correlated review errors, privacy leakage, and inference cost exceeding
accepted-task value. Do not give speculative implementation-hour estimates
without inspecting each slice and actual endpoint behavior.

### Documentation-only changes made by this study

This report traces primarily to sections 5.2, 5.4, 5.6, and 5.8, with
authority/review safeguards from 5.1 and 5.5. It respects section 3 and all
section 7 exclusions. It proposes acceptance evidence toward section 8; it
does not claim the harness already meets that section.

No runtime, contracts, policy, workflow, board, model deployment, or cloud
integration changes are part of this study. A future PR body should cite
these sections and state that the design is proposed, not shipped. Publication
requires an explicit request and the existing reviewed App-only path.

## 13. Recommended next decision

Approve or revise **Eve as one coder-worker identity with a source-grounded
workspace, explicit obligations, scoped curated memory, and corrigible
operation**, then run the baseline and workspace experiments.

Do not start with a council, an autonomous fine-tuning loop, a vector database,
an entire simulated brain, or a claim of omniscience. The strongest first
result is a small, reproducible improvement in verified software outcomes
without weaker authority, privacy, review, or resource controls.

Open research questions:

1. Which memory extracts transfer to genuinely unseen tasks rather than
   teaching the held-out set?
2. When does re-reading source beat compression or retrieval, by model and
   context length?
3. How much independent evidence does another model add after controlling
   for shared training and identical tests?
4. Which uncertainty signals predict a useful probe or escalation?
5. How should budgets vary with risk without conflating uncertainty and
   permission?
6. Can proposed learned procedures improve quality without hiding stale facts
   or creating an unreviewed instruction channel?
7. When does council specialization outperform the existing three-seat loop
   after accounting for its additional inference and review cost?

## 14. Source register

Bracketed identifiers attach claims to sources; E/T/D/P classifications in the
text describe the claim, not a universal quality rating of the source.
Paper years identify the cited work, not the latest possible field result.
Living documentation is dated by this study's access date rather than an
unverified release date.

### Software systems and evaluation

- **[B1]** Model Context Protocol, official versioned specifications.
  [2026-07-28 specification](https://modelcontextprotocol.io/specification/2026-07-28);
  [2025-06-18 architecture](https://modelcontextprotocol.io/specification/2025-06-18/architecture).
- **[S1]** Anthropic, *Create custom subagents*.
  [Documentation](https://code.claude.com/docs/en/sub-agents).
- **[S2]** OpenAI, *Codex* developer hub and cloud guide.
  [Hub](https://developers.openai.com/learn/codex);
  [Cloud guide](https://help.openai.com/en/articles/20001545-using-codex-cloud).
- **[S3]** GitHub, *Research, plan, and iterate on code changes with Copilot
  cloud agent*.
  [Documentation](https://docs.github.com/en/copilot/how-tos/copilot-on-github/use-copilot-agents/research-plan-iterate).
- **[S4]** Cursor, *Cloud Agents*.
  [Documentation](https://cursor.com/docs/cloud-agent).
- **[S5]** Windsurf Cascade documentation, redirecting to *Cascade Overview*
  in Devin Desktop documentation at access time.
  [Documentation](https://docs.windsurf.com/windsurf/cascade/cascade).
- **[S6]** Cognition, *SWE-bench Technical Report*, historical Devin evaluation.
  [Report](https://cognition.com/blog/swe-bench-technical-report);
  [Evaluation repository](https://github.com/CognitionAI/devin-swebench-results).
- **[S7]** Google, *Jules* announcement.
  [Article](https://blog.google/innovation-and-ai/models-and-research/google-labs/jules/).
- **[S8]** Wang et al. (2024 preprint; ICLR 2025), *OpenHands: An Open Platform
  for AI Software Developers as Generalist Agents*.
  [Paper](https://arxiv.org/abs/2407.16741);
  [Repository](https://github.com/All-Hands-AI/OpenHands).
- **[S9]** Yang et al. (2024), *SWE-agent: Agent-Computer Interfaces Enable
  Automated Software Engineering*.
  [Paper](https://arxiv.org/abs/2405.15793);
  [Repository](https://github.com/SWE-agent/SWE-agent).
- **[S10]** Aider, *LLM Leaderboards* (includes benchmark methodology).
  [Documentation](https://aider.chat/docs/leaderboards/).
- **[S11]** Continue, *What is Continue?*.
  [Documentation](https://docs.continue.dev/).
- **[S12]** Cline, project README and product surfaces.
  [Repository](https://github.com/cline/cline).
- **[S13]** LangChain, *LangGraph overview*.
  [Documentation](https://docs.langchain.com/oss/python/langgraph/overview).
- **[S14]** CrewAI, *Introduction*.
  [Documentation](https://docs.crewai.com/introduction).
- **[S15]** Microsoft, *Agent Framework overview*;
  [Documentation](https://learn.microsoft.com/en-us/agent-framework/overview/);
  [AutoGen](https://github.com/microsoft/autogen);
  [AG2](https://github.com/ag2ai/ag2).
- **[S16]** Hong et al. (2023 preprint; ICLR 2024), *MetaGPT: Meta Programming
  for A Multi-Agent Collaborative Framework*.
  [Paper](https://arxiv.org/abs/2308.00352).
- **[S17]** OpenAI, *Agents SDK*.
  [Documentation](https://openai.github.io/openai-agents-python/).
- **[S18]** Nous Research, *Hermes Agent*, project README.
  [Repository](https://github.com/NousResearch/hermes-agent).
- **[S19]** Jimenez et al. (2023 preprint; ICLR 2024), *SWE-bench: Can Language
  Models Resolve Real-World GitHub Issues?*
  [Paper](https://arxiv.org/abs/2310.06770);
  [Evaluation repository](https://github.com/SWE-bench/SWE-bench).

### AI mechanisms and limits

- **[A1]** Vaswani et al. (2017), *Attention Is All You Need*.
  [Paper](https://arxiv.org/abs/1706.03762).
- **[A2]** Wang et al. (2022; ICLR 2023), *Self-Consistency Improves Chain of
  Thought Reasoning in Language Models*.
  [Paper](https://arxiv.org/abs/2203.11171).
- **[A3]** Lightman et al. (2023), *Let's Verify Step by Step*.
  [Paper](https://arxiv.org/abs/2305.20050).
- **[A4]** Shinn et al. (2023), *Reflexion: Language Agents with Verbal
  Reinforcement Learning*. [Paper](https://arxiv.org/abs/2303.11366).
- **[A5]** Du et al. (2023), *Improving Factuality and Reasoning in Language
  Models through Multiagent Debate*. [Paper](https://arxiv.org/abs/2305.14325).
- **[A6]** Yao et al. (2022; ICLR 2023), *ReAct: Synergizing Reasoning and Acting
  in Language Models*. [Paper](https://arxiv.org/abs/2210.03629).
- **[A7]** Guo et al. (2017), *On Calibration of Modern Neural Networks*.
  [Paper](https://arxiv.org/abs/1706.04599).
- **[A8]** Kadavath et al. (2022), *Language Models (Mostly) Know What They Know*.
  [Paper](https://arxiv.org/abs/2207.05221).
- **[A9]** Liu et al. (2023; TACL 2024), *Lost in the Middle: How Language Models
  Use Long Contexts*. [Paper](https://arxiv.org/abs/2307.03172).
- **[A10]** Dziri et al. (2023), *Faith and Fate: Limits of Transformers on
  Compositionality*. [Paper](https://arxiv.org/abs/2305.18654).
- **[A11]** Packer et al. (2023), *MemGPT: Towards LLMs as Operating Systems*.
  [Paper](https://arxiv.org/abs/2310.08560);
  [Letta memory documentation](https://docs.letta.com/guides/agents/memory/).
- **[A12]** Park et al. (2023), *Generative Agents: Interactive Simulacra of Human
  Behavior*. [Paper](https://arxiv.org/abs/2304.03442).
- **[A13]** Wang et al. (2023), *Voyager: An Open-Ended Embodied Agent with Large
  Language Models*. [Paper](https://arxiv.org/abs/2305.16291).
- **[A14]** Hadfield-Menell et al. (2016 preprint; IJCAI 2017),
  *The Off-Switch Game*. [Paper](https://arxiv.org/abs/1611.08219).

### Neuroscience

- **[N1]** Lillicrap et al. (2020), *Backpropagation and the brain*,
  Nature Reviews Neuroscience.
  [Review](https://doi.org/10.1038/s41583-020-0277-3).
- **[N2]** Raichle and Gusnard (2002), *Appraising the brain's energy budget*,
  PNAS. [Article](https://pmc.ncbi.nlm.nih.gov/articles/PMC124895/).
- **[N3]** (2023), *Two decades of astrocytes in neurovascular coupling*,
  Frontiers in Network Physiology.
  [Review](https://www.frontiersin.org/journals/network-physiology/articles/10.3389/fnetp.2023.1162757/full).
- **[N4]** *Unraveling the functional complexity of the locus
  coeruleus-norepinephrine system*.
  [Bibliographic record](https://pubmed.ncbi.nlm.nih.gov/39866663/).
- **[N5]** (2018), *Basal Ganglia Mechanisms in Action Selection, Plasticity,
  and Dystonia*. [Review](https://pmc.ncbi.nlm.nih.gov/articles/PMC5815934/).
- **[N6]** Tononi and Cirelli (2006), *Sleep function and synaptic homeostasis*.
  [Bibliographic record](https://pubmed.ncbi.nlm.nih.gov/16376591/).
- **[N7]** Hablitz and Nedergaard (2021), *The Glymphatic System: A Novel
  Component of Fundamental Neurobiology*.
  [Review](https://www.jneurosci.org/content/41/37/7698).
- **[N8]** Squire (2004), *Memory systems of the brain: a brief history and
  current perspective*, Neurobiology of Learning and Memory.
  [Review](https://www.sciencedirect.com/science/article/pii/S107474270400061X).
- **[N9]** Anderson (2010), *Neural reuse: A fundamental organizational principle
  of the brain*, Behavioral and Brain Sciences.
  [Record](https://doi.org/10.1017/S0140525X10000853).
- **[N10]** Baddeley (2000), *The episodic buffer: a new component of working
  memory?*, Trends in Cognitive Sciences.
  [Paper](https://doi.org/10.1016/S1364-6613(00)01538-2).
- **[N11]** Miller and Cohen (2001), *An integrative theory of prefrontal cortex
  function*, Annual Review of Neuroscience.
  [Review](https://doi.org/10.1146/annurev.neuro.24.1.167).
- **[N12]** Fleming and Lau (2014), *How to measure metacognition*, Frontiers
  in Human Neuroscience.
  [Review](https://doi.org/10.3389/fnhum.2014.00443).
- **[N13]** Schacter (1999), *The seven sins of memory: Insights from psychology
  and cognitive neuroscience*, American Psychologist.
  [Record](https://pubmed.ncbi.nlm.nih.gov/10199218/).
- **[N14]** Graybiel (2008), *Habits, Rituals, and the Evaluative Brain*.
  [Institutional copy](https://web.math.princeton.edu/~sswang/basal-ganglia/graybiel08_annu_rev_neurosci_BG-evaluative-brain.pdf).
- **[N15]** Einstein and McDaniel (2005), *Prospective Memory: Multiple Retrieval
  Processes*, Current Directions in Psychological Science.
  [Record](https://doi.org/10.1111/j.0963-7214.2005.00382.x).
- **[N16]** McClelland, McNaughton, and O'Reilly (1995), *Why there are
  complementary learning systems in the hippocampus and neocortex: Insights
  from the successes and failures of connectionist models of learning and
  memory*, Psychological Review.
  [Paper](https://doi.org/10.1037/0033-295X.102.3.419).
- **[N17]** Nader, Schafe, and LeDoux (2000), *Fear memories require protein
  synthesis in the amygdala for reconsolidation after retrieval*, Nature.
  [Study](https://www.nature.com/articles/35021052).

### Compute and physical support

- **[C1]** MosaicML engineering / Databricks, *LLM Inference Performance
  Engineering: Best Practices*. Living engineering reference.
  [Article](https://www.databricks.com/blog/llm-inference-performance-engineering-best-practices).
- **[C2]** Hong et al. (2016), *Experimental test of Landauer's principle in
  single-bit operations*, Science Advances.
  [Paper](https://doi.org/10.1126/sciadv.1501492).
