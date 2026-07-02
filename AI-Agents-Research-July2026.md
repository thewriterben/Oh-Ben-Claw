# Deep Research — What Oh-Ben-Claw Should Adopt Next (July 2026)

*Compiled 2026-07-02 via a 5-angle research fan-out (memory/retrieval, safety/eval standards, MCP/A2A ecosystem, edge inference + realtime voice, embodied AI + Rust stack), ~40 web searches, primary-source fetches for load-bearing claims, and adversarial cross-checking. Companion to `AI-Agents-Innovations-June2026.md`.*

> **Confidence tags:** **[HIGH]** primary source fetched/verified · **[MED]** consistent secondary sources · **[LOW]** single/aggregator source — do not build on.
> **Verification note:** one direct inter-agent conflict was caught and adjudicated — rmcp's version (a stale "0.16.0" claim vs. crates.io's actual **1.7.0**, resolved by fetching the crates.io API). The 2026 web's staleness/fabrication problem is real even inside a verification pipeline; every "adopt" item below traces to a primary source.

---

## Executive summary

Three headlines. **First, the architecture is validated:** the 2026 research and standards consensus — CaMeL-family deterministic mediation, ISO/IEC TR 5469 / draft TS 22440 doctrine, Figure's Helix layering, Letta's memory benchmarking — has converged on exactly the choices OBC already made (security enforced outside the model, deterministic gate as the safety function, dual-system reflex+reasoner, simple agent-driven memory search over heavyweight memory frameworks). **Second, the gaps are specific and cheap:** the MCP conformance suite can test OBC's hand-rolled implementation in CI today; the retrieval layer has a drop-in local upgrade path (fastembed-rs + sqlite-vec + FTS5/RRF); red-team evals should become adaptive and OWASP-ASI-mapped; and tool arguments need provenance/taint tracking. **Third, Phases 19–20 have a settled bill of materials:** gpt-realtime over WebSocket + Gemini Live as alternate, microWakeWord/LiteRT-Micro on-MCU, Moonshine v2 + Kokoro + llama.cpp/Gemma-3n on Pi/Jetson, Wyoming protocol for ecosystem interop.

---

## ADOPT NOW

### 1. MCP conformance suite in CI *(Phase 15 hardening — highest assurance per unit effort)* [HIGH]
`npx @modelcontextprotocol/conformance server --url …` / `conformance client --command …` tests **any** implementation (no SDK required), supports `--spec-version 2025-11-25` and `draft` (2026-07-28), ships an `--expected-failures` YAML baseline and a GitHub Action (`modelcontextprotocol/conformance@v0.1.11`; latest v0.1.16, Mar 2026). Run it against OBC's server and client in both modes; SEP-2484 means every future Standards-Track feature arrives with a matching scenario. This turns our hand-rolled dual-mode from "audited once by Fable" into "continuously conformance-tested." ([conformance repo](https://github.com/modelcontextprotocol/conformance))

### 2. Local retrieval upgrade — the P1 deferred item now has a concrete stack *(Phase 16 follow-up)* [HIGH]
The `lexical_score` swap point we left behind `TrajectoryStore::similar()` can be filled entirely locally, no per-turn network:
- **fastembed-rs v5.x** (ONNX via `ort`, sync API, models cached offline) with **EmbeddingGemma-300M-Q4** (<200MB RAM, Matryoshka dims — store 256-d vectors to keep SQLite blobs small). ([fastembed-rs](https://github.com/Anush008/fastembed-rs), [EmbeddingGemma](https://developers.googleblog.com/en/introducing-embeddinggemma/))
- **sqlite-vec v0.1.9** (revived under Mozilla sponsorship, Mar 2026) — vector KNN inside the existing episodes DB; brute-force is fine to ~100K episodes. ANN (DiskANN, v0.1.10-alpha) stays on watch. ([sqlite-vec releases](https://github.com/asg017/sqlite-vec/releases))
- **SQLite FTS5/BM25** as the lexical leg (replacing token-overlap), fused with dense via **RRF** (~3 lines, no score normalization) — the 2025-26 hybrid-retrieval consensus: lexical wins exact anchors (device IDs, error strings), dense wins paraphrase. ([Qdrant hybrid guidance](https://qdrant.tech/course/essentials/day-3/hybrid-search-demo/))
- Optional second stage: fastembed's **jina-reranker-v1-turbo** cross-encoder over the top ~30 fused candidates, only on turns that actually query memory.
- Embed episodes **off the hot path** in the improver's idle pass — the validated "sleep-time compute" pattern (arXiv 2504.13171): consolidate, summarize, precompute during idle. An embodied agent has natural idle; use it.

### 3. Adaptive, OWASP-mapped red-team evals *(Track 0)* [HIGH]
- The **OWASP Top 10 for Agentic Applications** (ASI01–ASI10, Dec 2025, NIST/Microsoft-reviewed) is the de-facto taxonomy — map OBC's red-team evals to ASI IDs (injection-driven actuation = ASI01/02; skill supply chain = ASI04; trajectory/memory poisoning = ASI06). ([OWASP announcement](https://genai.owasp.org/2025/12/09/owasp-top-10-for-agentic-applications-the-benchmark-for-agentic-security-in-the-age-of-autonomous-ai/))
- **NIST's agent-hijacking work**: adaptive attacks raised hijack success **11% → 81%** vs. static suites on AgentDojo — a frozen golden injection set is an upper bound, never proof. Periodically regenerate attack payloads; structure evals AgentDojo-style (user task + injection task + utility-under-attack + attack-success). ([NIST blog](https://www.nist.gov/news-events/news/2025/01/technical-blog-strengthening-ai-agent-hijacking-evaluations))
- OWASP's **quarterly GenAI exploit round-ups** (Q1 2026 published Apr 14) are a ready feed for refreshing the corpus with real incident patterns.
- Related integrity lesson: **BenchJack** (Berkeley, arXiv 2605.12673) broke 8 major agent benchmarks via reward hacking, and METR reports frontier models reward-hack in a large share of eval runs — keep graders outside the agent's write path (OBC's sensor-assertion approach is the right kind).

### 4. Provenance/taint tracking on tool arguments *(Track 0 — the one real architectural gap)* [HIGH]
The most-validated injection defense (CaMeL, Google DeepMind; extended to computer-use agents in 2026) enforces: **values derived from untrusted content may not parameterize privileged actions**. OBC's chokepoint gates *which* tools run, but not *where the argument values came from*. Concrete increment: tag tool-result provenance (untrusted channel/web/sensor text vs. operator), propagate tags through the loop, and have the Track 0 gate refuse physical-tool args carrying untrusted taint without explicit approval. Spotlighting stays as hygiene but never counts in the safety case. ([out-of-band defenses eval](https://arxiv.org/html/2606.26479v1))

### 5. Judge calibration path *(Phase 15 follow-up)* [HIGH/MED]
2026 practice: an LLM judge stays advisory until calibrated against human gold labels — Cohen's kappa ≥ ~0.6 as the common bar (vendor-heuristic, not standards-traceable), pinned judge model version, versioned rubrics, position/verbosity-bias mitigations. Build a small gold set from eval transcripts before `LlmJudge` is ever more than advisory; it must never gate actuation safety. ("Reliability without Validity," arXiv 2606.19544)

### 6. Phase 19 bill of materials — realtime sessions [HIGH]
- **Primary: OpenAI gpt-realtime** — GA, WebSocket + WebRTC + SIP, in-session function calls, **image input** ($4/$16 text, $32/$64 audio, $5 image per 1M tokens; 32k ctx). WebSocket means the ESP32-S3 needs only a WSS proxy on the OBC gateway — no on-device WebRTC. ([model page](https://developers.openai.com/api/docs/models/gpt-realtime))
- **Alternate: Gemini Live** — still Preview on the consumer API (GA on Vertex as Gemini 2.5 Flash Native Audio); its **≤1 FPS JPEG vision input** maps perfectly to an ESP32 camera; barge-in, function calling, ephemeral client tokens. Design the session layer provider-agnostic across both. ([Live API docs](https://ai.google.dev/gemini-api/docs/live-api))
- Copy turn-detection design (semantic turn model vs. raw VAD) from Pipecat v1.0/LiveKit Agents as *reference architectures* — they're Python, not dependencies.

### 7. Phase 20 bill of materials — edge stack [HIGH/MED]
- **On-MCU:** **microWakeWord** + LiteRT-Micro (TFLM) on ESP32-S3 — production-proven in Home Assistant Voice PE, the exact hardware class OBC uses. ESP32-class reality: wake word/KWS (~20ms, ~64–80kB arenas) + small vision; full STT/TTS goes upstream.
- **SBC tier:** **Moonshine v2** (MIT; streaming STT that beats Whisper large-v3 WER at 245M params, **with Kokoro + Piper TTS bundled**; Pi latency tables published) — the single most actionable find. LLM tier: **llama.cpp via `llama-cpp-2` FFI** with **Gemma 3n E2B/E4B** (multimodal, ~2B effective footprint). Realistic Pi 5 throughput: ~14 tok/s at 1B-class, 2–7 tok/s at 3B; Jetson Orin Nano Super (~$250, JetPack 6.2 Super Mode): ~29 tok/s at 3B — the reflex tier is viable at 1–3B, not 7B. ([Moonshine](https://github.com/moonshine-ai/moonshine), [JetPack 6.2](https://developer.nvidia.com/blog/nvidia-jetpack-6-2-brings-super-mode-to-nvidia-jetson-orin-nano-and-jetson-orin-nx-modules/))
- **Speak the Wyoming protocol** from Rust — tiny, stable, and instantly interoperable with the entire Home Assistant voice hardware/software ecosystem.

### 8. Safety-case documentation framing *(docs, ~an hour)* [HIGH]
Frame Track 0 in standards language: **"the LLM proposes; the deterministic gate disposes"** — the AI is *not* the safety function (ISO/IEC TR 5469:2024 doctrine, soon TS 22440); the gate alone should satisfy a 13849-style analysis (fail-safe states, watchdog, timeout-to-safe — the firmware already does this). Use NIST **AI 100-2e2025** adversarial-ML terminology in the threat model. Cheap, and it makes the safety story legible to anyone who audits the project.

### 9. Publish the MCP server to the official registry [HIGH]
registry.modelcontextprotocol.io is preview (no GA; expect resets) — publish metadata for discoverability, build no hard dependency on it.

---

## NEXT (after the above; roughly Q3 2026)

- **MCP Tasks extension** — now an official *extension* (SEP-2663) with a redesigned, wire-incompatible API vs. the 2025-11-25 experimental shape (`tasks/get`/`tasks/update`/`tasks/cancel`, polling not blocking, server-directed creation, no `tasks/list`). Pairs naturally with the Phase 17 harness: long-running tool calls returning task handles. Finalizes July 28 — implement against the extension spec only. [HIGH]
- **MRTR (`InputRequiredResult`, SEP-2322)** for OBC's MCP server — the sanctioned stateless way to ask mid-call operator questions; a natural fit for **approval prompts over HTTP** (approval workflow → `inputRequests` → client retries with answers). [HIGH]
- **A2A Signed Agent Cards** — A2A v1.0 added card signing (LF: 150+ orgs, production deployments). Check whether OBC's hand-rolled agent card can carry/verify signatures; that's the gap enterprise peers will care about. No official Rust SDK exists (community crates target v0.3.0) — keep custom code. [HIGH]
- **`genai` crate evaluation** — the lightest multi-provider LLM abstraction (OpenAI/Anthropic/Gemini/Ollama/Groq/DeepSeek/xAI). Could delete much of `src/providers/` — but OBC's failover/retry wrappers are custom value; evaluate as an adapter beneath them, not a replacement of them. [HIGH]
- **Semantic consolidation pass** — extend the evolve/improve idle jobs with episode summarization → semantic facts (A-MEM's structured-note ideas, NeurIPS 2025, applied in batch — never on the hot path). [HIGH/MED]
- **Embodied safety benchmarks as eval sources** — SafeAgentBench, SafeMind, and especially **EmbodiedGovBench** (arXiv 2604.11174), whose "upgrade safety" axis directly targets self-improving skill pipelines like skill_forge. [MED]
- **ESP32-C6/RISC-V for new fleet nodes** — esp-hal (no_std) + Embassy async is the maturing pure-Rust firmware path; RISC-V chips build on upstream Rust (Xtensa still needs forked LLVM). New nodes: prefer C6. [HIGH for status, MED for "official top-tier" framing]
- **`tokio-cron-scheduler`** (persisted job stores) — evaluate against the custom scheduler; overlaps Phase 17 durability. [HIGH]

---

## WATCH

- **rmcp ≥ 2.0 with 2026-07-28 support** — rmcp is 1.7.0 stable, ~10M downloads **[HIGH, crates.io-verified]**, but the RC beta SDKs are Python/TS/Go/C# only; OBC's hand-rolled implementation is currently *ahead of the official Rust SDK*. Revisit when rmcp ships the stateless core — that's the big delete-custom-code moment.
- **ISO/IEC TS 22440-1/2/3** — CD comments closed Apr 2026; publication expected late 2026. Align the Track 0 safety case when it lands.
- **NIST CAISI AI Agent Standards Initiative** (launched Feb 2026) + NCCoE agent identity/authorization concept paper — guidance documents expected from the Jan–Apr RFI cycle; nothing to conform to yet.
- **GR00T N1.7** — Apache-2.0, 3B reasoning VLA running on Jetson AGX Orin/Thor (~16GB class): the first credible "VLA as a peripheral capability" for the fleet, exactly the V2-strategy watch item. Also: **Figure Helix 02's "System 0"** (a safety/stability layer *below* the reflex layer) — OBC's on-MCU Track 0 gate already is one; the naming/validation is useful.
- **Cosmos-Transfer-style synthetic data** for simulate-stage skill rehearsal (the realistic 2026 sim-to-real pipeline; interactive world-model rollouts à la Genie are **not** primary-verified for robot rehearsal yet).
- **MCP working groups**: *Skills Over MCP* and *Triggers and Events* — both overlap OBC subsystems (skill_forge, heartbeat/scheduler); watch so custom designs don't diverge from an emerging standard.
- **MCP Apps (SEP-1865, Final)** — server-side only, and only if OBC wants to render device dashboards inside Claude/ChatGPT/VS Code hosts.
- **Gemma 4 / Qwen3.5 small multimodal** [LOW — aggregator-only; verify at primary sources before planning], sqlite-vec ANN alphas, LanceDB Rust 1.0 line, Step-Audio R1.1 realtime, ClawHub post-incident screening efficacy.

---

## AVOID / negative results

- **Memory frameworks as services (Mem0/Zep/Letta-hosted)** — the only reproducible public evidence (Letta's benchmarking, which caught unreproducible vendor numbers; a plain filesystem-tool agent beat Mem0's reported LoCoMo score) says a capable agent with simple local search tools already matches them. OBC's agent-owns-a-search-tool design is on the validated side. Vendor memory benchmark numbers (LoCoMo/LongMemEval) are currently unreliable across the board. [HIGH]
- **Self-hosted speech-to-speech (Moshi/Ultravox-class)** — L4/A100-class GPU floor; the honest 2026 split is cloud realtime for conversation + local stack for reflexes, which is exactly the Phase 19/20 design. [HIGH]
- **Migrating to rmcp or a2a-rs today** — both behind OBC's implementations (rmcp: no RC; a2a-rs: v0.3.0 vs. OBC's v1.0). [HIGH]
- **Skill-marketplace trust-by-scanning** — the Feb 2026 ClawHavoc incident (341/2,857 registry skills malicious; scanning defeated by padding/evasion) proved publish-time scanning fails; **staged rollout + provenance pinning + no-source-no-install** are the defensible controls, and OBC's Phase 15 install-policy + Phase 16 P3 staged rollout already implement them. [HIGH for incident, MED for exact figures]
- **Numbers not to repeat:** 7B-on-Pi-5 above ~3 tok/s; per-minute realtime-API prices (aggregator math); "official Rust A2A SDK" (a2a-rust.com is not under a2aproject); "1,184 malicious skills" (didn't replicate; 341 is the verifiable figure); 2026 SEO-farm memory-vendor comparisons (innobu, agentmarketcap, tokenmix).

---

## Suggested sequencing against the roadmap

| When | Item | Roadmap slot |
|---|---|---|
| This week | MCP conformance suite in CI; registry publish; safety-case doc framing | Phase 15 |
| Next | Retrieval upgrade (fastembed + sqlite-vec + FTS5/RRF + idle-pass embedding) | Phase 16 follow-up (fills the P1 deferred item) |
| Next | Adaptive OWASP-mapped red-team evals + argument taint tracking | Track 0 |
| Then | Judge calibration gold set; MRTR approval prompts; Tasks extension (post-July 28); Signed Agent Cards check | Phase 15/17 follow-ups |
| Phase 19 start | gpt-realtime WSS session channel + Gemini Live alternate; microWakeWord reference node | Phase 19 |
| Phase 20 start | Moonshine v2 + Kokoro + llama.cpp/Gemma-3n reflex tier; Wyoming protocol | Phase 20 |
| Standing | Watch list reviews at each phase boundary | — |

---

*Primary sources are linked inline throughout. Key verification actions this pass: OWASP announcement page fetched; MCP conformance README fetched; SDK-betas + RC blog posts fetched; gpt-realtime model page fetched; Gemini Live docs fetched; Moonshine repo fetched; crates.io API for rmcp fetched (adjudicating a stale-version conflict); fastembed-rs repo verified; sqlite-vec release notes verified; NIST/ISO/Federal Register pages verified by the standards agent.*
