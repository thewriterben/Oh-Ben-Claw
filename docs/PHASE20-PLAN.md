# Phase 20 — Edge-Native Intelligence: Gap Analysis and Step Plan

*Written 2026-10-30, before implementation. Companion to the Phase 20 checklist in `ROADMAP.md`, the research BOM in `AI-Agents-Research-July2026.md` §7, and the `[edge]` config sketch in `V2-IMPLEMENTATION.md`.*

## Substrate audit

What the phase builds on, as of this date. "Where" is the file to open.

| Item | Status | Where |
|---|---|---|
| `[edge]` section takes effect at `start` (bounds, P2P mesh, `serve_tools`) | Done (roadmap row 1) | `obc_config::edge_start`, `src/main.rs` `run_start` |
| Per-turn router, two brains, seven ordered rules, `agent.brain` fact | Done (parity item 3, 2026-09-11) | `crates/obc-agent/src/routing.rs`, `Agent::route_turn`, `Agent::complete_routed` |
| Cloud → local failover mid-turn (cloud fails, backoff, `Restart` delta) | Done | `Agent::complete_routed`, `Routing::mark_down` |
| Local → cloud escalation (local brain down or unable) | **Missing.** The only way up is `[[provider.fallbacks]]`, which the router does not see | `obc_providers::from_config_full` (`FailoverProvider`) |
| Privacy rule sees mid-turn tool results | **Hole** (closed by P0). Rule 3 reads the world-state block when the turn starts; a cloud-routed turn that called `hear` or `vision_analyze` sent the result up on the next model call | `Agent::process`, tool-results push |
| Non-local fallback under the local brain with routing on | **Hole** (closed by P0). Validation only checked that a model exists; a cloud fallback answered private turns the moment Ollama was down | `obc_config::Config::validate`, provider block |
| `ModelRegistry`: local-first, health-aware, stale recheck | Library only. `record_health` and `EdgeAgentBuilder::prefer_local` have no production caller; no command builds an `EdgeAgent` | `crates/obc-providers/src/model_registry.rs`, `crates/obc-agent/src/edge.rs` |
| System 1 (deterministic reflexes) | Done, rules only, no model | `crates/obc-reflex` |
| Small local model as a reflex tier | **Missing.** `[provider]` with `name = "ollama"` or `compatible` + loopback URL is the whole story; nothing picks a small model per role | — |
| `audio_transcribe`, `text_to_speech` | Cloud by default: `OPENAI_API_BASE` (api.openai.com), whisper-1. Pointable at a local server only by env var | `crates/obc-tools/src/builtin/audio.rs` |
| `vision_analyze` | Cloud by default: `OBC_VISION_API_BASE`, gpt-5.4. Same | `crates/obc-tools/src/builtin/vision.rs` |
| `hear` | Local: reads and queries world-memory `audio.{stream}` events; nothing writes them from a microphone | `crates/obc-tools/src/builtin/audio_suite.rs` |
| Wake-word | **Missing.** Firmware has I2S capture; no detector on the MCU or the SBC | `firmware/obc-esp32-s3/src/audio.rs` |
| `obc-audio` crate | Vocabulary + `SpeechSink` only (pipeline removed 2026-07-30) | `crates/obc-audio` |
| Deployment planner: per-role local model | **Missing.** The planner emits `[edge]` but no model name, no pull, no health check | `crates/obc-planner` (`plan_site`) |
| Eval | **Missing.** `tests/evals.rs` has a scripted-provider harness and `EchoTool` to build on | `tests/evals.rs` |

**Headline finding.** The router is the right place for every Phase 20 policy, and it already carries cost, privacy and the cloud-side connectivity axis. What it lacks is the other direction (local → cloud when the local brain cannot) and two privacy holes that made its promise untrue: mid-turn tool results, and cloud fallbacks underneath the local brain. Close the holes first, because every later step adds more reasons to send a turn up.

## Steps

Each step ships on its own, with tests, a roadmap row and a changelog entry.

### P0 — Privacy holds for the whole turn (shipped 2026-10-30)

- `RoutingConfig.private_tools` (default `hear`, `sense`, `audio_transcribe`, `vision_analyze`). Once one has run in a turn, the rest of the turn is local, whichever brain began it; `agent.brain.reason` says `private tool result in context`. Exact name match: a peer's `p2p:<node>:hear` is that node's room and already crossed the LAN.
- `Config::validate` refuses a non-local `[[provider.fallbacks]]` when `[provider.routing]` is enabled. Local means `ollama` by name or a loopback `base_url` (the `ModelEntry::from_provider` rule). Cloud models go under `[provider.routing.cloud]`.
- Tests: `a_private_tool_result_moves_the_rest_of_the_turn_local` (obc-agent), `a_cloud_fallback_under_the_local_brain_is_refused_when_routing_is_on` (obc-config).

### P1 — Local → cloud escalation on connectivity (shipped 2026-10-30; roadmap "policy-driven cloud fallback", first axis)

- `Agent::complete_routed` takes the route's reason: when the local brain fails on a turn the router sent local for a non-privacy reason (`routing::may_escalate`: `routine turn`, `background session`), the call is retried on the cloud brain once, unless cooling or over budget (`Routing::can_escalate`), after a `Restart` delta. Privacy reasons (`private facts in context`, `private tool result in context`) never escalate; `routing disabled` never escalates. `RoutingConfig.escalate_on_local_failure` (default true) switches it off.
- Audited: the `agent.brain` fact gets `escalated: true` and the reason `local brain failed, escalated`; `ActionAuditor::record_routing_escalation` writes a `routing.escalate` row with the session as node, both brains and the reason as arguments, and the local error in the decision text.
- `Routing::local_down_until` mirrors the cloud backoff (`offline_backoff_secs`): while it cools, non-privacy turns route to the cloud up front as `local brain down, backing off` (rule 5, after the privacy and background rules), so a dead Ollama does not cost a timeout per turn; privacy turns still go local and fail.
- Deferred: `ModelRegistry::record_health` stays unwired. The agent holds no registry; P3 builds one at `start` and `mark_local_down` feeds it then.

### P2 — Capability and confidence axes (shipped 2026-10-30; roadmap "policy-driven cloud fallback", second and third axes)

- Capability: `RoutingConfig.cloud_tools` (default empty; candidates `plan_site`, `foresight`, documented in `config.example.toml` and not enabled). When a turn on the local brain runs one of them (exact name match, like `private_tools`), the rest of the turn goes to the cloud brain as `cloud tool called` (`routing::cloud_tool_escalates`), the mirror image of P0's rule. Privacy wins: a turn local for `private facts in context` or `private tool result in context`, or one in which a private tool has already run, stays local. The cloud back-off and budget are respected (`Routing::cloud_available`).
- Confidence: `RoutingConfig.escalate_on_low_confidence` (default false). When on, a turn routed local for a non-privacy reason gets `routing::LOW_CONFIDENCE_CONTRACT` appended to its system message, and only then, so a model never told about the contract never triggers it. A local final answer that is exactly `{"escalate": "<why>"}` (surrounding whitespace or one fenced block allowed; prose, other keys, non-string values are an ordinary answer; `routing::escalation_request`) is dropped, the client told to discard it (`Restart`), and the turn re-run on the cloud brain as `local brain asked to escalate: <why>` (the why cut at 200 chars). Once per turn: a cloud answer of the same shape is returned as text, and so is the ask when the cloud is cooling or over budget, or at the last tool iteration.
- Audited like P1: `agent.brain` gets `escalated: true` and the reason; `ActionAuditor::record_routing_handoff` (a sibling of `record_routing_escalation`) writes the `routing.escalate` row with the session as node and the tool, or the brain's why, in the decision text.
- Tests: `routing.rs` `a_cloud_tool_escalates_a_routine_turn_unless_privacy_applies`, `an_escalation_request_is_exactly_one_json_object`, `a_long_why_is_cut_on_a_char_boundary`, `the_contract_names_the_shape_the_parser_accepts`; `lib.rs` `a_cloud_tool_call_on_the_local_brain_escalates_the_rest_of_the_turn`, `privacy_wins_over_a_cloud_tool`, `a_cooling_cloud_keeps_a_cloud_tool_turn_local`, `a_low_confidence_answer_is_re_run_on_the_cloud_once`, `with_the_option_off_the_json_is_ordinary_text`, `a_private_turn_is_not_told_the_contract_and_its_ask_is_text`; `tests/evals.rs` `eval_escalation_cloud_tool_hands_the_turn_up`, `eval_escalation_low_confidence_re_runs_the_turn_once`.

### P3 — Small-model reflex tier

- `[edge].local_backend = "ollama" | "llama-cpp"` and `[edge].local_model`; `llama-cpp` means the `compatible` provider against `http://127.0.0.1:8080/v1` (llama.cpp server, Gemma 3n per the research BOM). `prefer_local` in `EdgeAgentBuilder` either gets a caller (the reflex tier) or is deleted.
- Health probe at `start` (`ModelRegistry` with `record_health`): the world-state block shows `edge.model.<name>` up or down.
- Reflex rules in `obc-reflex` gain a `model` action: a bounded, single-call prompt to the small model (no tools, no history), for classifications the rules cannot express. Latency ceiling enforced; a timeout is a rule miss, not a turn.

### P4 — Edge model management from the deployment planner

- `plan_site` emits `[edge].local_model` per node role and a `models` section (name, size, quantisation, source); `obc fleet` gets `models pull|status` through the existing OTA/channel path, with the pull verified by digest.
- The planner refuses a model that does not fit the role's RAM budget.

### P5 — On-device wake-word + STT/TTS

- SBC: a Wyoming-protocol client in `obc-audio` (the crate is a vocabulary today) talking to local Moonshine (STT) and Piper or Kokoro (TTS); `audio_transcribe` and `text_to_speech` prefer it when configured, so the env-var cloud default becomes the fallback and not the rule. Transcripts land as `audio.{stream}` events, which `hear` already reads.
- MCU: microWakeWord on the ESP32-S3 (`firmware/obc-esp32-s3`), publishing a wake event on the mesh; the SBC opens the stream on it.
- Under P0 every transcript is private by construction: `audio_transcribe` is in `private_tools`.

### Eval (roadmap last row)

`tests/evals.rs`: a scripted local provider answers the defined reflex tasks with the cloud provider wired to fail, and every `agent.brain` fact says `local`; a second set forces each escalation axis (P1–P2) and checks the audit row exists and the reason names the axis. The P2 axes are in (`eval_escalation_*`, checking the answer and the `agent.brain` reason; the audit row is checked in `obc-agent`'s own tests); the offline reflex set and the P1 row wait for P3's reflex tier.
