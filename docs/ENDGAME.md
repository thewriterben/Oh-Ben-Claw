# The endgame: what it would take to migrate the core

*Measured 2026-08-08 by `scripts/core_endgame.py`. Re-run it; do not trust this
page over the script.*

Fourteen crates have left this repository. Every one was chosen the same way:
`scripts/extractability.py` ranks modules by blocking-edge count, the cheapest
is taken, and when its count is one, that one edge is turned around first. It
has worked well enough that the last extraction — 3714 lines of navigation —
cost ten changed lines.

That method has a horizon, and this document is about what is past it. The
remaining core is `agent` (9244), `tools` (9052), `spine` (4680), `config`
(3371) and `gateway` (2313): 28,660 lines, and the thing standing in the way is
not size.

## There is no order

`extractability.py` answers "which module has fewest blocking edges". That
question presumes an order exists. For the core it does not:

    spine -> fleet -> spine
    agent -> spine -> agent
    spine -> tools -> spine
    tools -> audio -> tools
    agent -> tools -> agent
    agent -> skill_forge -> agent
    mcp   -> vision -> mcp

Seven two-module cycles, and eighteen longer ones through them. No sequence of
extractions resolves a cycle, because whichever module goes first still needs
the one left behind. Every plan of the form "extract agent, then tools" is
wrong before it is written, and the edge-count ranking cannot say so — it is not
looking at the graph, only at each row of it.

So the core does not need an extraction *order*. It needs the cycles broken, and
only then does an order exist. That is a different kind of work and it is worth
naming as such.

## The first thing the map showed was that the map was wrong

Before any of the above could be trusted, the measurement found a defect in
itself. `src/security.rs` was twelve lines:

    //! Track 0 — re-exported from the `obc_safety` crate.
    pub use obc_safety::*;

A redirect, and to the compiler and to a reader indistinguishable from the crate
it points at. Not to a survey: every tool here that measures module edges saw
`pub mod security;` and counted `crate::security::…` as a dependency on a module
still in the tree. That was **44 crossings**, 27% of every core crossing
measured, pointing at a crate extracted on 2026-07-30.

Changing it to `pub use obc_safety as security;` and deleting the file moved the
core from 161 crossings to 117, and halved `tools`. Nothing was refactored. The
tangle was in the map.

Worth stating plainly because the same trap is still live: any module in this
tree whose body is a re-export will inflate every measurement taken here until
someone notices. That is now one line to check and it was not checked for nine
days.

## What the core actually costs, after that correction

| module | loc | blocking edges | crossings |
|---|---:|---:|---:|
| `agent` | 9244 | 6 | 65 |
| `tools` | 9052 | 4 | 13 |
| `spine` | 4680 | 3 | 11 |
| `gateway` | 2313 | 6 | 17 |
| `config` | 3371 | 6 | 11 |

117 crossings across 28,660 lines. That is a small number, and it is the
surprise in this document: the core is not densely coupled. It is *cyclically*
coupled, which is a different disease and mostly a cheaper one.

*Superseded the same day by step 1 below. After `obc-tool-api` the same table
reads:*

| module | loc | blocking edges | crossings |
|---|---:|---:|---:|
| `agent` | 9244 | 6 | **42** |
| `tools` | 8886 | 4 | 13 |
| `spine` | 4680 | **2** | 9 |
| `gateway` | 2313 | **5** | 12 |
| `config` | 3371 | 6 | 11 |

*87 crossings, 16 cycles. Both tables are kept because the delta between them is
the only evidence that any of this works.*

Three specifics carry most of it.

**`tools::traits` is a trait crate wearing a 9052-line module.** `agent` names
it 23 times, `gateway` 5, `spine` 2 — 30 of the 117 crossings are the `Tool`
trait and `ToolResult`, not tool machinery. This is the `RiskClass` situation
that blocked obc-safety for months, and the `TaskExecutor` situation from
obc-a2a two days ago: a contract living inside the largest implementation of
itself. Lifting `tools::traits` into `obc-tool-api` is one crate, no logic, and
it breaks `agent -> tools`, `gateway -> tools` and `spine -> tools` in one move.

**`agent::reflex` is named by things that are not the agent.** `tools` reaches
for `reflex` and `safing`, `spine` for `reflex`, `safing` and `notify`, `config`
for `reflex`. The reflex *vocabulary* — `Action`, `ActionSink`, `Cmp`,
`FiredReflex`, `EscalationBudget` — is shared System 1/System 2 language that
happens to live in `agent/reflex.rs` next to the engine and the sinks that drive
it. Splitting vocabulary from wiring frees `foresight` (677) and `learning`
(454) as a side effect.

**`config` is nearly free already.** Six edges, eleven crossings, and every one
is a config struct typed by the module it configures: `McpServerConfig`,
`Mission`, `ForesightRule`, `OrchestratorConfig`, `retry`, `inventory`. The
established fix is already used four times here — the crate owns its own config
block, the root `Config` composes them, as `obc-planner`, `obc-conscience`,
`obc-cost` and `obc-tunnel` all do. `config` is not a hard module; it is a
module waiting for its dependents to leave.

## The order that exists once the cycles are cut

1. **`obc-tool-api`** — lift `tools::traits`. Breaks three edges into `tools`.
2. **`obc-reflex`** — lift the reflex vocabulary out of `agent`. Breaks the
   `tools -> agent` and `spine -> agent` back-edges, and frees `foresight` and
   `learning` outright.
3. Re-measure. With `agent <-> tools` and `agent <-> spine` cut, most of the
   eighteen long cycles should not exist, and the remaining ones —
   `spine <-> fleet`, `tools <-> audio`, `mcp <-> vision` — are two-module and
   small (5, 3 and a handful of crossings).
4. Only then ask which of `agent`, `tools`, `spine` is extractable, because only
   then is the question well-formed.

*Steps 1–3 are done. Each has a section below saying what it actually cost,
including where this list was wrong — and it was wrong about step 2's effect and
right about step 3's shape. Step 4 is now the open question.*

### Step 1, done — and half of it was wrong

`obc-tool-api` landed 2026-08-08. Extracting the file was the easy half and, on
its own, achieved **nothing**:

    tools::traits -> crates/obc-tool-api, re-exported as
    `pub use obc_tool_api as traits;`

    core_endgame.py before:  agent 90 crossings, spine 3 edges, 25 cycles
    core_endgame.py after:   agent 90 crossings, spine 3 edges, 25 cycles

Identical, and correctly so. `crate::tools::traits::Tool` still resolves
*through* `mod tools`, so `agent` still requires `crate::tools` to exist. The
re-export that has made every previous extraction call-site-free is exactly what
prevents this one from breaking an edge.

That is a real distinction and it was not in the first draft of this page:

  - **Extracting code** so it can be vendored publicly — a re-export under the
    old name is right, and the edge may stay.
  - **Breaking a cycle** — the dependents must stop naming the old path. Nothing
    less will do it.

The second half was 55 rewrites across 23 files, `crate::tools::traits::X` to
`obc_tool_api::X`, leaving `src/tools/**` alone (a module should not reach for a
crate to find its own contract) and `tests/**` alone (they exercise the public
surface on purpose). Then the numbers moved:

| | before | after |
|---|---:|---:|
| cycles | 25 | **16** |
| `agent` crossings | 90 | 67 |
| `gateway` blocking edges | 7 | 6 |
| `spine` blocking edges | 3 | **2** |

`spine -> tools` is gone outright, so the `spine <-> tools` cycle no longer
exists, and neither do the six long cycles that ran through it.

The lesson generalises past this repository: a facade keeps a build green while
leaving the architecture exactly as it was. Both halves are needed, and only the
second one is visible to a dependency graph.

### Step 2, done — and the plan was wrong about what it would buy

`obc-reflex` landed 2026-08-12: 1324 lines, 28 tests, the whole engine.

**The plan said "lift the reflex *vocabulary*".** It proposed splitting
`Action`, `ActionSink`, `Cmp`, `FiredReflex` and `EscalationBudget` away from
the engine and the sinks that drive them, on the theory that the vocabulary was
shared System 1/System 2 language and the wiring was not movable. The split was
never made, because it turned out not to be necessary: `reflex.rs` named exactly
four things outside itself, three had been crates for days, and the fourth —
`crate::spine` — was entirely inside one struct. `SpineActionSink` moved to
`src/spine/action.rs` in a separate commit (one field, one constructor
parameter, two topic constants) and implements the crate's `ActionSink` from
there. After that the engine had no local names left and moved whole.

That is the third time this week the same manoeuvre has been the answer, and it
is worth naming as a rule rather than a coincidence: **trait where the
abstraction is, implementation where the dependency is.** `SpineActuatorSink`
left `movement` the same way on 2026-08-08; `TaskExecutor` was declared in
`obc-a2a` and implemented next to the agent for the same reason.

**Both halves were done this time.** `src/agent/mod.rs` carries
`pub use obc_reflex as reflex;` — the facade step 1 warned about — and the
consumers were repointed in the same commit anyway: thirteen import sites across
seven modules (`config`, `foresight`, `learning`, `mission`, `spine`, `vision`,
`tools`) now name `obc_reflex::…` directly. Seven sites still go through the
facade: four inside `agent` itself, where a module reaching for a crate to find
its own vocabulary would be the wrong shape, and three in `src/main.rs`, which
is the binary and outside this measurement either way.

One correction to make in passing: that re-export's doc comment says
"`crate::agent::reflex::…` is unchanged at every call site." That was true of
the commit that added it and stopped being true in the same commit that
repointed thirteen of them. It is the smallest possible instance of the thing
this whole document is about.

Then the numbers moved:

| | after step 1 | after step 2 |
|---|---:|---:|
| cycles | 16 | **13** |
| crossings | 87 | **77** |
| `agent` loc | 9244 | **7785** |
| `agent` crossings | 42 | 37 |
| `tools` crossings | 13 | 10 |
| `config` crossings | 11 | 9 |
| `spine` crossings | 9 | 9 |

**And the plan's specific prediction was wrong.** It said step 2 "breaks the
`tools -> agent` and `spine -> agent` back-edges." It broke neither. Both
survive:

    tools -> agent    1 symbol,  2 crossings    safing x2
    spine -> agent    3 symbols, 4 crossings    notify x2, reflex, safing

`reflex` was not the only thing those modules reached into `agent` for — it was
the one that had been *counted*, in a paragraph that listed the reflex crossings
by name and read the rest as background. `safing` is a second System 1 module
sitting next to the first, and `notify` is a third. The measurement showed all
three before the plan was written; the plan named the one it had a story for.
The `spine -> agent : reflex` crossing that remains is a doc comment in
`src/spine/action.rs` explaining where the sink came from — which the Caveats
section below already says this script cannot tell from a call.

What the prediction got right: `foresight` (677 loc) is now at **zero** blocking
edges, and `learning` (454) is blocked only by `foresight`. Those two are the
next extractions and need no design work.

The generalisation, one level up from step 1's: **a count of blocking edges says
nothing about what each edge costs to turn.** `obc-reflex` was listed at
thirteen blocking edges by `extractability.py` on 2026-08-01, and left on one
39-line move, because
twelve of the thirteen were names that had already become crates and were still
being spelled through `agent`'s re-exports. `obc-navigation` was listed at one
and was worth 3714 lines. The ranking is a starting point for reading, not a
work order.

### Step 2's dividend, collected the next day

`obc-foresight` (677 loc, 11 tests) and `obc-learning` (454 loc, 4 tests) landed
2026-08-13. This is the one prediction on this page that held exactly, so it is
worth being precise about what "held" means: the plan said these two would come
free, and they did — no design work, no logic changed, eight paths spelled
`crate::memory::world::` rewritten to `obc_memory::` and five import sites
repointed. That is the whole extraction.

| | after step 2 | after the dividend |
|---|---:|---:|
| crossings | 77 | **75** |
| `tools` blocking edges | 7 | **5** |
| `config` blocking edges | 6 | **5** |
| `vision` blocking edges | 3 | **2** |
| cycles | 13 | 13 |

Cycles unchanged, and correctly: neither module was in one. These were leaves.
The `tools` row moves by two because `tools` named both of them — a 9052-line
module reaching for a rule miner and a forecaster, which is the same shape as
its reaching for `traits` in step 1 and will keep being the shape until `tools`
is asked what it actually is.

Three notes for whoever runs the next one.

**The chain was one field deep, three times over.** `learning` waited on
`foresight`, which waited on `reflex`, which waited on one action sink holding
an `Arc<SpineClient>`. 2455 lines across three crates came out from behind one
field. Read as a queue that is three jobs; read as edges it is one, and the
difference between those two readings is the whole subject of this section.

**Re-measure after every edge-turning commit, before deciding what is next.**
Neither of these two was the target of the commit that freed them; the step-2
commit named neither, and both were at zero blocking edges by the time it
merged. A survey is a photograph, and the thing it photographs moves when you
turn an edge.

**And one dependency was found by the compiler that no survey could have
found.** `obc-learning` needs `anyhow`; it appears in no `use` line in the file.
Both call sites write `anyhow::Result<…>` inline in a return type — the exact
shape that made `extractability.py`'s first version report `config` as
edge-free, which the section above already records as the near-miss that would
have cost a week. Same blind spot, second time: once in a script that read
`use` lines, once in a human reading them. Both times the compiler caught it in
seconds. The scripts rank. The compiler decides. Nothing here has changed that.

### Step 3, done — and the cheapest thing in this document

Step 2 was supposed to break `tools -> agent` and `spine -> agent`. It broke
neither, and the section above says so. This is what breaking them actually
took, on 2026-08-13:

| | | |
|---|---|---:|
| `tools -> agent` | two `use` lines in one `#[cfg(test)]` module | 4 cycles |
| `spine -> agent` | one `use` of `Severity`/`DIGEST_PREFIX`, plus one `use` line in one test | 5 cycles |

**Nine of the thirteen cycles were held open by four `use` lines, three of them
inside test modules.**

That is not a figure of speech. `tools` is 8802 lines and named `agent` exactly
twice, both times inside `#[cfg(test)]` in `tools/builtin/power.rs`, importing
`standard_safing_rules`. `spine` is 4740 lines and named `agent` once in
production — `Severity` and `DIGEST_PREFIX`, to classify an escalation for its
mesh view — and once in a test, the same safing import. Four lines, in a
28,000-line core, sitting in nine cycles.

What was done to each, and why it is not a trick:

- **The escalation vocabulary moved to `obc-reflex`.** `Severity` and
  `DIGEST_PREFIX` are what an `Action::Escalate` *is* — how urgent, and whether
  it is a digest of earlier escalations rather than a fresh event. They were
  living inside the escalation *delivery*. Same arrangement as `RiskClass`
  inside `tools::traits` and `NodeState` inside `fleet`, and it produced the
  same back-edge for the same reason: something outside needed the noun and had
  to name the implementation to get it. 45 lines, no dependencies.
- **Three tests moved to `tests/`.** Each spans two layers and asserts something
  neither layer can assert alone: a reported battery must not stop an actuator
  while a measured one must; a mesh node going quiet must reach System 2 through
  the *shipped* safing rules. They were integration tests wearing unit tests'
  clothes, which is the third time that phrase has been the right diagnosis this
  week. None was weakened to make it move — the mesh test still calls
  `standard_safing_rules`, because a test that built its own rule would prove
  the engine works and prove nothing about the rules anyone runs.

| | after step 2 | after the dividend | after step 3 |
|---|---:|---:|---:|
| cycles | 13 | 13 | **4** |
| `spine` blocking edges | 2 | 2 | **1** |
| `tools` blocking edges | 7 | 5 | **2** |
| `spine` crossings | 9 | 9 | **5** |

The four that remain are exactly the shape step 3 predicted, and it named three
of the four: `spine <-> fleet`, `agent <-> skill_forge`, `audio <-> tools`,
`mcp <-> vision`. All two-module. None routes through `agent -> tools` or
`agent -> spine` any more, because those arrows now only point one way.

Two things to take from this rather than one.

The first is the obvious one and it is real: **a test in the wrong file is a
dependency edge, and the graph cannot tell it from a call.** Nothing here was
architecture. The production code of `tools` never needed `agent`, and it is
worth asking how long a claim like "these two modules are mutually entangled"
would have survived if anyone had looked at *which lines*.

The second is less comfortable. This document confidently listed the work as
"two crates and a re-measurement" and then predicted, in writing, that step 2
would break these two edges. It did not, and the reason it did not is that the
prediction was made from a table of module-level totals. `tools -> agent` read
as `safing x2` in that table — two crossings, indistinguishable from two calls
in a hot path. The instrument that made this project measurable is the same one
that hid the answer for five days, and it hid it by summarising. Both facts
belong on this page.

### Two of the last four, same day

`mcp <-> vision` and `agent <-> skill_forge` were cut on 2026-08-13, a few hours
after step 3, and they were cheaper than step 3 was.

**`mcp -> vision` was a rustdoc link.** One line, in `src/mcp/mod.rs`, pointing
readers at the perception loop that reuses a live MCP connection. `vision -> mcp`
is three real `use` lines. The comment was the entire return arrow, so rewording
it — describing the caller instead of linking to it — cut the cycle and changed
no code at all.

That is the doc-comment caveat at the bottom of this page collecting its second
scalp in two days, and it is worth being uncomfortable about rather than pleased.
The link was *useful*. It told a reader where to look. Removing it is a small
loss of navigability paid for a real gain in graph shape, and the only reason
the trade is worth making is that the measurement drives decisions. A project
that did not measure would have been right to keep the link.

**`skill_forge -> agent` was one `impl` block.** `impl ReplayExecutor for
crate::agent::Agent`, 36 lines, sitting next to the trait instead of next to the
type. `agent -> skill_forge` is nine references the other way and all of them are
real. Moving the impl to `src/agent/skill_replay.rs` reversed it.

Fifth instance of the same manoeuvre this month — `SpineActuatorSink`,
`SpineActionSink`, `AgentExecutor` for `obc-a2a`, `Severity` out of `notify`,
now this. At five occurrences it is not a knack, it is a shape the codebase
keeps producing: **a trait declared next to its caller, implemented next to its
caller, for a type that lives somewhere else.** Rust permits the impl in either
crate as long as one of them owns the trait, and the graph is the only thing
that objects.

One honest cost, because the table below does not show it:

| | before | after |
|---|---:|---:|
| cycles | 4 | **2** |
| `agent -> skill_forge` crossings | 8 | **11** |

Moving the impl *increased* the crossings on that edge by three, because the
file now names `skill_forge` from inside `agent`. The edge was already there and
already one-directional, so a cycle died and a count went up. Anyone optimising
the crossing total rather than the cycle list would have scored this a
regression. It is the second time on this page that the summary statistic points
the wrong way.

What is left is two cycles, and neither is a relocation:

- **`audio <-> tools`** — `audio::suite` holds a `TextToSpeechTool` as a
  *field*. The suite owns a tool; the tool layer also wraps the suite. That is a
  question about which one is the primitive, and it has an answer, but not one
  that can be reached by moving a file.
- **`spine <-> fleet`** — `fleet` needs `SpineClient` and `TOPIC_PREFIX` to
  assign a task to a node. Same sink shape as the three above and therefore
  tractable: `fleet` declares what it needs to publish, `spine` implements it.
  That is a trait to design rather than a block to cut and paste.

## What may never move, and why that is an answer

`agent` is the reasoning loop: provider calls, tool dispatch, memory, policy,
observability, cost, approval. It is the thing this project *is*, and steps 1–3
would leave it a genuinely self-contained module rather than an entangled one —
7726 lines after step 3, down from 9244, and shrinking each time something that
was never the reasoning loop leaves it. Whether it should then be public is a
decision about the project, not about its dependency graph, and this document
deliberately does not make it.

What this document does claim is narrower and checkable: **there is no technical
reason the core cannot be separated, and the work is a handful of small edges
turned around, not a rewrite.** Both crates that sentence originally budgeted
for — `obc-tool-api` and `obc-reflex` — exist, and the number that moved was
crossings, not lines rewritten: 117 to 69, and 25 cycles to 4, across four
commits, one 39-line move and four `use` lines. If that turns out to be wrong,
it will be wrong
in a way `core_endgame.py` can show, which is the only kind of plan worth
writing here.

## Caveats

`core_endgame.py` is grep-derived, like its two siblings, and inherits their
limits: it resolves `crate::x::y` and not `super::` — `extractability.py`
reports 139 unresolved `super::` paths and this script shares that blind spot
without counting them; it counts textual crossings, not compile edges; and a
symbol named once in a doc comment counts the same as one called in a hot loop.

That last one stopped being hypothetical on 2026-08-13. `spine -> agent :
reflex` was, for a day, a doc comment in `src/spine/action.rs` explaining where
a sink had come from — a live edge on the graph, describing code that had
already left. Two comments written that day deliberately describe a move without
spelling the path it moved from, for that reason. A measurement you can pollute
by writing prose is a measurement worth knowing the shape of.
It ranks; the compiler decides. Every extraction so far has ended with a scratch
build proving the crate stands alone, and nothing here replaces that step.

The first draft of this paragraph said 162, from memory of an earlier run. It is
139. A document about the cost of trusting unmeasured numbers is a poor place to
put one, and the correction is left visible rather than quietly applied.
