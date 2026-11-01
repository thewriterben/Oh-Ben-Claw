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
| `ModelRegistry`: local-first, health-aware, stale recheck | Done (P3). Built at `start` in edge mode, probed, writes `edge.model.<name>`; fed by the local back-off and the reflex tier. `prefer_local` deleted | `crates/obc-providers/src/model_registry.rs`, `src/main.rs` `run_start` |
| System 1 (deterministic reflexes) | Done; P3 added the `model` action | `crates/obc-reflex` |
| Small local model as a reflex tier | Done (P3). `[edge].local_backend`/`local_model`, `obc_agent::reflex_tier::ProviderReflexModel` | `crates/obc-agent/src/reflex_tier.rs` |
| `audio_transcribe`, `text_to_speech` | Done (P5): a local Wyoming server first when `[voice] stt`/`tts` name one, the cloud (`OPENAI_API_BASE`) as the fallback; cloud-only, as before, when neither is set | `crates/obc-tools/src/builtin/audio.rs` |
| `vision_analyze` | Cloud by default: `OBC_VISION_API_BASE`, gpt-5.4. Same | `crates/obc-tools/src/builtin/vision.rs` |
| `hear` | Local: reads and queries world-memory `audio.{stream}` events; nothing writes them from a microphone | `crates/obc-tools/src/builtin/audio_suite.rs` |
| Wake-word | SBC half done (P5): `[voice.wake]` gates `obc/voice/wake/{node}` and answers with `obc/voice/listen/{node}`. **The MCU detector is still missing**; firmware has I2S capture only | `crates/obc-spine/src/wake_bridge.rs`, `firmware/obc-esp32-s3/src/audio.rs` |
| `obc-audio` crate | Vocabulary + `SpeechSink`, and since P5 the Wyoming client, WAV conversion and the wake gate | `crates/obc-audio` |
| Deployment planner: per-role local model | Done (P4). With `edge_inference` the planner writes `[edge].local_model` and `[[edge.models]]` for the host, inside its RAM budget; `fleet models pull|status` and `ota_update` `board_type = "model"` fetch and check by digest | `crates/obc-planner/src/deployment/models.rs`, `crates/obc-tools/src/builtin/edge_models.rs` |
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
- Deferred to P3, since done: `ModelRegistry::record_health` was unwired because the agent held no registry. P3 builds one at `start` and `mark_local_down` feeds it.

### P2 — Capability and confidence axes (shipped 2026-10-30)

- Capability: `RoutingConfig.cloud_tools` (default empty; `plan_site`, `foresight` are candidates). When a local completion calls one of them, on a turn routed local for an escalatable reason, the tool runs here and the rest of the turn goes to the cloud brain (`Agent::hand_up`, unless cooling or over budget), recorded as `cloud tool called, escalated`. Any completion, not only the first: a later call is as much a sign the turn needs the cloud.
- Confidence: on such a turn the local brain gets `routing::ESCALATE_HINT`, an ephemeral system message before the ask. An answer ending in `{"escalate": "<why>"}` (`routing::escalation_request`: bare or fenced, last 1 KiB) drops the local answer behind a `Restart` delta and re-runs the step on the cloud brain once, with one extra iteration, recorded as `local brain unsure, escalated`; the hint leaves the context first, and `routing::cloud_context` keeps it out of P1's retries too. A request that may not go up is stripped from the answer (`I can't answer that well here: <why>.` when nothing else is left). `RoutingConfig.escalate_on_low_confidence` (default true).
- Audit: every axis writes the P1 row; the arguments carry the router's reason, the decision text names the axis (`failed: …`, `called cloud tool …`, `unsure: …`).
- P0 now stops every axis: a private tool result on a local `routine turn` changes the reason to `private tool result in context`, so a later failure, cloud tool or unsure answer stays local (before, the rule only fired on cloud-routed turns, and a `routine turn` that called `hear` and then failed escalated the result).
- Evals: `tests/evals.rs` `escalation_axes` forces P1, capability and confidence, and checks the `agent.brain` reason names the axis and one `routing.escalate` row exists, plus a control row that stays local.

### P3 — Small-model reflex tier (shipped 2026-10-30)

- `[edge].local_backend = "ollama" | "llama-cpp"` and `[edge].local_model`; `llama-cpp` means the `compatible` provider against `http://127.0.0.1:8080/v1` (llama.cpp server, Gemma 3n per the research BOM). `prefer_local` in `EdgeAgentBuilder` either gets a caller (the reflex tier) or is deleted.
- Health probe at `start` (`ModelRegistry` with `record_health`): the world-state block shows `edge.model.<name>` up or down.
- Reflex rules in `obc-reflex` gain a `model` action: a bounded, single-call prompt to the small model (no tools, no history), for classifications the rules cannot express. Latency ceiling enforced; a timeout is a rule miss, not a turn.
- As shipped: `prefer_local` was deleted (the reflex tier takes its model from `[edge]`, not from the provider chain). `Action::Model { prompt, on }` maps labels to actions, one level deep; `ReflexController::with_model(model, ceiling)` runs it under `tokio::time::timeout`, sending the rule's trusted evidence with the question, and counts `reflex.model.{hit,miss,timeout}`. `[edge].reflex_timeout_ms` (default 2000) is the ceiling. `start` probes `[provider]` (if local) and the reflex model into a `ModelRegistry` with world memory; `edge.model.<name>` is `{"status": "up"|"down", "local": bool}`, written on change. `Routing::mark_local_down` marks the local model down there; a local answer marks it up. Eval: `reflex_tier_offline` in `tests/evals.rs` (first half of the roadmap's last row).

### P4 — Edge model management from the deployment planner (shipped 2026-11-01)

- `plan_site` emits `[edge].local_model` per node role and a `models` section (name, size, quantisation, source); `obc fleet` gets `models pull|status` through the existing OTA/channel path, with the pull verified by digest.
- The planner refuses a model that does not fit the role's RAM budget.
- As shipped: only the host runs a model (the sub-agents are microcontrollers), so "per node role" is the orchestrator's board. `obc_planner::deployment::models` holds a six-model catalogue (Ollama tags, sizes and quantisations as listed 2026-11-01) and the rule: a model needs its weights plus a quarter plus 256 MB; the host's budget is its RAM (`ram_mb` on the host item, else a 1 GB floor for nanopi-neo3 and raspberry-pi-4/5) less 384 MB. With the `edge_inference` desire on an edge host the planner writes `local_backend = "ollama"`, `local_model` and one `[[edge.models]]` entry (name, size_mb, ram_mb, quantisation, `source = "ollama://<tag>"`). `[deployment] local_model` asks for one by name; one that does not fit, or is not in the catalogue, is a warning and no model, never a substitute. Unset, the largest reflex-class model that fits is chosen (larger models are in the catalogue for operators to ask for, but do not answer inside the 2 s reflex ceiling). Without the desire the output is byte-identical, so the nanopi goldens hold.
- `obc_tools::builtin::edge_models`: `[[edge.models]]` entries take `source = "ollama://<tag>"` or an `https://` GGUF URL, and `digest = "sha256:<hex>"`. An `https` source must carry a digest (validation refuses one without). Ollama pulls go through `/api/pull`; the digest checked is the manifest's, which pins every layer. A mismatch is deleted (`/api/delete`, or the `.part` file), not left for the reflex tier to load. An unpinned pull succeeds and prints the line to pin. `oh-ben-claw fleet models status|pull [name]` runs it on this node; `ota_update` with `board_type = "model"` and a `model` name does the same for the agent, and only for a listed model, so the digest comes from the config and never from the call. `Config::validate` warns when `local_model` is not in the list, or its source does not match `local_backend`.
- Tests: `the_largest_reflex_model_that_fits_is_chosen`, `a_requested_model_that_does_not_fit_is_refused_not_swapped`, `edge_inference_writes_the_model_into_the_edge_section`, `a_model_that_does_not_fit_is_a_warning_and_no_model`, `without_edge_inference_no_model_is_planned` (obc-planner); `edge_inference_output_names_the_model_it_lists` and the `nanopi_edge_inference` scenario in `tests/planner_config_loads.rs`; the `edge_models` tests (pinned pull skipped when installed, mismatch removed, https download verified and a bad one deleted) and `a_model_pull_needs_the_model_listed_in_the_config` (obc-tools); `validate_checks_the_edge_model_list` (obc-config).
- Not done here: fleet-wide fan-out (asking every node over the spine to pull) and the TypeScript planner port and its fixture in OBC-Prime, which must learn `local_model`/`ram_mb` and the `[edge]` model lines before a parity fixture can carry `edge_inference`.

### P5 — On-device wake-word + STT/TTS (SBC side shipped 2026-11-01)

- SBC: a Wyoming-protocol client in `obc-audio` (the crate is a vocabulary today) talking to local Moonshine (STT) and Piper or Kokoro (TTS); `audio_transcribe` and `text_to_speech` prefer it when configured, so the env-var cloud default becomes the fallback and not the rule. Transcripts land as `audio.{stream}` events, which `hear` already reads.
- MCU: microWakeWord on the ESP32-S3 (`firmware/obc-esp32-s3`), publishing a wake event on the mesh; the SBC opens the stream on it.
- Under P0 every transcript is private by construction: `audio_transcribe` is in `private_tools`.
- As shipped: a new `[voice]` section (not `[edge]`, which is the node's role and brain, nor `[audio_suite]`, which switches `hear`/`speak`): `stt` and `tts` are Wyoming endpoints (`tcp://host:port`), with `tts_voice`, `cloud_fallback` (default true), `timeout_ms` (default 30000) and `stream` (default `mic0`). `obc_audio::wyoming` is the client: one TCP exchange per call under the deadline, size caps on every header, data block and payload. `obc_audio::wav` converts between the tools' files and the protocol's PCM. `stt_plan` and `tts_plan` in `obc-tools` choose the backends in order. The Wyoming server comes first when the input is a PCM WAV (STT) or the format is `wav` (TTS, now the default on-device). The cloud follows only with `cloud_fallback`, and `use_local` (whisper.cpp) still never reaches it. `start` and both MCP paths swap the two tools in place. With the audio suite on, each transcript is recorded as `audio.{stream}` (source `audio_transcribe: <backend>`). Wake: `[voice.wake]` (`words`, `min_confidence`, `listen_ms`, `cooldown_ms`) runs `obc_audio::wake::WakeGate` over `obc/voice/wake/{node}`, where the node is the topic's last segment and never the payload's. An accepted wake is recorded as an `audio.{stream}` event labelled `wake` and answered on `obc/voice/listen/{node}` with the window. `validate` refuses bad endpoints, a zero timeout or listen window, an out-of-range or NaN confidence and empty wake words.
- Not done here: the microWakeWord detector and the mic streaming in `firmware/obc-esp32-s3`. The SBC half of the wake contract is in place and waits for the firmware. No resampling or compressed-audio decoding on-device, so non-WAV audio goes to the cloud or fails without `cloud_fallback`. Voice models are not managed through P4's `[[edge.models]]`. `obc doctor` does not probe the Wyoming servers.

### Eval (roadmap last row)

`tests/evals.rs`: a scripted local provider answers the defined reflex tasks with the cloud provider wired to fail, and every `agent.brain` fact says `local`; a second set forces each escalation axis (P1–P2) and checks the audit row exists and the reason names the axis.
