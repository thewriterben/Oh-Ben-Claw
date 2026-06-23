# The State of AI Agents — June 2026

*A cited research briefing. Compiled June 23, 2026. Leads with Nous Research's Hermes line, then surveys the broader agent landscape: frameworks, interoperability protocols, reasoning and multi-agent advances, computer-use agents, benchmarks, memory, and notable releases.*

> **How to read confidence tags.** Each claim is tagged **[HIGH]** (primary source — official blog, GitHub, arXiv, vendor announcement), **[MED]** (corroborated by reputable secondary sources, or company-stated metrics), or **[LOW]** (single/aggregator source, or precise figures that look fabricated). The 2026 web is heavily polluted with SEO pages that mix real and invented model names and invent leaderboard scores, so numbers are hedged deliberately.

---

## Executive summary

The defining shift of the last 18 months is that agentic ability stopped being a prompt trick and became a *trained behavior*. Reinforcement-learning-trained reasoning plus test-time compute (DeepSeek-R1, OpenAI's o-series and GPT-5, Claude's extended/interleaved thinking, Gemini 3) is the engine; a two-layer interoperability stack (**MCP** for agent-to-tool, **A2A** for agent-to-agent) is the plumbing, now under neutral governance at the Linux Foundation; and the headline capability metric is **long-horizon autonomy** — METR's finding that the length of task an agent can do reliably is doubling every few months.

Against that backdrop, **Nous Research** is a notable outlier: rather than a frontier closed model, its bet is *open weights + decentralized training + a self-improving open-source agent runtime* — and that runtime, Hermes Agent, became the fastest-growing open-source agent project of 2026.

---

## Part 1 — Nous Research: Hermes models and the Hermes Agent

### The Hermes model line

**Hermes 4 (August 2025)** is Nous's flagship generation: a family of open-weight **hybrid-reasoning** models at 14B, 70B, and 405B, with a technical report on arXiv (2508.18255, Aug 25 2025). **[HIGH]** A key change from earlier generations: the family is no longer Llama-only — the 14B is built on **Qwen3-14B-Base** (Apache-2.0), while the 70B and 405B are built on **Llama-3.1** (llama3 license). **[HIGH]**

What makes Hermes 4 an agent backbone rather than just a chat model:

- **Hybrid reasoning** via explicit `<think>…</think>` segments, toggled with a `thinking=True` flag or a "deep thinking" system prompt. **[HIGH]**
- **Tool calling interleaved with reasoning** — tool calls are emitted as `<tool_call>{...}</tool_call>` within a single assistant turn, with built-in parsers in vLLM and SGLang and `<tool_call>` as dedicated tokens for clean streaming. **[HIGH]**
- **Structured outputs / JSON-schema adherence**, including trained repair of malformed JSON. **[HIGH]**
- A post-training corpus scaled ~50× over Hermes 3 — from ~1M samples / 1.2B tokens to ~5M samples / ~60B tokens, blending reasoning and non-reasoning data. **[HIGH]**

On capability benchmarks, the 405B reports strong reasoning/coding numbers (MATH-500 ~96, AIME'24 ~82, GPQA Diamond ~70, LiveCodeBench ~61) **[MED — relayed via aggregators of the arXiv PDF]**. Nous also created **RefusalBench**, a self-defined benchmark measuring willingness to help in commonly-disallowed scenarios; it is a Nous metric, so treat "SOTA" framing as vendor-defined. **[HIGH for the numbers as reported; MED as an objective claim]**

**Hermes 4.3 36B (December 2, 2025)** is the most recent model and arguably the more strategically interesting one. **[HIGH]** It is built on **ByteDance Seed-OSS-36B-Base**, is Apache-2.0, and extends context to **512K tokens**. It is positioned to nearly match the Hermes 4 70B at roughly half the parameters, sized so quantized GGUFs fit on off-the-shelf consumer/enterprise GPUs. **[HIGH]** Its headline significance: it is **Nous's first production model post-trained entirely on the Psyche decentralized network** — and the decentralized run reportedly *outperformed* a centrally-trained control version, while staying stable at ~144K tokens/sec across 24 nodes. **[HIGH — primary Nous blog + HF card]**

For context, **Hermes 3 (August 2024)** — 8B/70B/405B full fine-tunes of Llama 3.1, 128K context — was the generation that introduced Nous's structured function calling and agentic reasoning (arXiv 2408.11857). **[HIGH]**

### The decentralized training stack behind Hermes

Nous's differentiation is infrastructure as much as models:

- **DisTrO** — a distributed-training optimizer that drastically cuts inter-node bandwidth, making training over the open internet feasible. **[HIGH]**
- **Psyche** — a decentralized training *network* built on DisTrO, using a Solana smart contract for consensus state and a custom P2P mesh for gradient exchange; open-source (PsycheFoundation/psyche), with a live dashboard. **[HIGH]**
- **Atropos** (RL environments framework) and **DataForge** (synthetic-data pipeline), both tagged on the Hermes 4 model cards as part of the training stack. **[HIGH that they exist; MED on specifics like "1,200 tasks"]**
- **Consilience 40B**, a 40B model pre-trained on Psyche toward ~20T tokens, described as the largest publicly-verifiable decentralized pre-training run. **[MED — secondary sources only]**
- Funding: a **$50M Series A led by Paradigm (~April 2025)** at roughly a $1B valuation. **[MED-HIGH]**

### Hermes Agent — how Hermes is used autonomously

The piece that ties Nous to the "AI agents" story is **Hermes Agent** (github.com/NousResearch/hermes-agent), an open-source **self-improving** agent framework, MIT-licensed, mostly Python. **[HIGH]** It became the **fastest-growing open-source agent framework of 2026**, crossing roughly **188K–193K GitHub stars** by mid-June 2026 after a late-February launch — corroborated across multiple trackers (Dealroom, star-history, The Agent Report). **[HIGH on the scale/trajectory; exact star count moves daily]**

Notable design points:

- **Self-improvement loop**: after each task, it evaluates the outcome, extracts reusable reasoning patterns, and stores them as **skill files**; on similar future tasks it loads the relevant skill instead of reasoning from scratch. A companion repo (`hermes-agent-self-evolution`) optimizes skills/prompts/code using DSPy + GEPA. **[HIGH/MED]**
- **Model-agnostic**: it works across 17+ providers (Nous Portal, OpenRouter, NVIDIA NIM, OpenAI, etc.), so **Hermes-the-model is one backbone option, not a requirement** — an important nuance for the "Hermes powers agents" framing. **[HIGH]**
- **Capabilities**: 40+ tools, native **MCP** client support, subagent spawning / multi-agent Kanban orchestration, cron automation, messaging gateways (Telegram, Discord, Slack, WhatsApp, Signal, email), and serverless terminal backends (Modal, Daytona). **[HIGH]**
- **Ecosystem**: a "Skills Hub" reportedly listing **90,000+ skills**, a **Hermes Desktop** native app (~June 2026, 40K+ beta users), and a reported selection by **NVIDIA as the reference runtime for its Nemotron 3 Ultra (550B)** model. **[MED — secondary sources; directionally consistent but not all primary-verified]**
- **Research flywheel**: the agent does batch trajectory generation and "trajectory compression for training the next generation of tool-calling models" — i.e., it explicitly feeds data back into future Hermes tool-use training. **[HIGH]**

> ⚠️ **Caution on third-party trackers.** SEO/AI-generated sites gave conflicting Hermes Agent stats (release dates, commit counts, version numbers). Where the primary GitHub repo and reputable trackers agree (MIT license, ~190K stars, fast 2026 growth), confidence is high; precise commit/contributor/skill counts from blog "trackers" should be treated as approximate.

---

## Part 2 — Agent frameworks and orchestration

The framework layer consolidated and grew up in late 2025–2026, with several projects shipping their first *stable* major versions:

- **LangChain 1.0 / LangGraph 1.0 — GA October 2025**, the first stable majors, with a no-breaking-changes pledge until 2.0. LangChain 1.0 added a "middleware" concept; LangGraph 1.0 is the durable low-level runtime (persistence, human-in-the-loop, durable state). LangChain also became a unicorn — a **$125M Series B (Oct 2025) at a $1.25B valuation** led by IVP. **[HIGH]**
- **OpenAI Agents SDK** — shipped March 2025 as the production successor to the experimental *Swarm*; a major **April 2026** update added native sandboxed execution (BYO or via Daytona/E2B/Modal/Vercel/others), a **subagent** primitive (beta), a planned "code mode," Codex-style filesystem tools, and first-class MCP support. **[HIGH]**
- **Microsoft Agent Framework — 1.0 GA, early April 2026** (.NET and Python), the unified successor merging **AutoGen + Semantic Kernel**; both predecessors are now in maintenance mode. It supports both MCP and A2A. **[HIGH]**
- **Google ADK (Agent Development Kit)** — open-source, code-first (Python/Go/Java/TypeScript), paired with the managed Vertex AI Agent Engine; at Cloud Next 2026 the Vertex AI Agent Builder was rebranded the "Gemini Enterprise Agent Platform." **[HIGH; download stats MED]**
- **Anthropic Claude Agent SDK** — the renamed Claude Code SDK, exposing the same agent loop, tools, and context management that power Claude Code (Python + TypeScript); recent additions include validated JSON outputs, fallback-model handling, and self-hosted sandboxes in beta. **[HIGH]**
- **CrewAI** — role-based multi-agent framework + enterprise control plane; $18M total funding. Its "nearly half the Fortune 500 / ~2B executions" adoption claims are company marketing. **[HIGH on structure; LOW on adoption claims]**
- **Pydantic AI, smolagents, LlamaIndex** — Pydantic AI reached a stable V1 (type-safety/structured-output-first) and is the fastest-growing newer entrant; smolagents remains a ~1,000-line lightweight code-execution framework; LlamaIndex stays primarily the retrieval/RAG + workflow layer. **[MED]**

**Adoption reality check.** Enterprise interest is broad but production conversion lags. Survey figures conflict wildly by methodology (claims range from ~31% to ~70% of organizations "in production," with widely-cited warnings that a large share of pilots fail to graduate and that 40%+ of agentic projects risk cancellation by 2027). Treat any single percentage as unreliable; the *qualitative* signal — high interest, weak production conversion, with evaluation and governance as the top blockers — is robust. **[LOW on specific numbers; HIGH on the qualitative pattern]**

---

## Part 3 — Interoperability protocols: the agent "internet"

A two-layer standard stack has consolidated, and — importantly — moved to **neutral governance**.

### Model Context Protocol (MCP) — agent-to-tool

- Introduced by Anthropic in **November 2024**; the current stable spec is dated **2025-11-25** (one-year anniversary), which expanded MCP beyond synchronous tool calls to **async operations, statelessness, and server identity**, plus security/authorization SEPs. **[HIGH]**
- A **June 18, 2025** spec update reworked authorization, classifying MCP servers as OAuth Resource Servers (RFC 8707 Resource Indicators). **[HIGH]**
- **OpenAI adopted MCP in March 2025**; first-class client support now spans ChatGPT, Claude, Cursor, Gemini, Microsoft Copilot, and VS Code. Reported scale at the Dec 2025 donation: ~97M+ monthly SDK downloads and ~10,000 active servers. **[HIGH; exact figures approximate]**
- An **official MCP Registry** launched in preview (Sept 8, 2025); GitHub launched its own registry days later. **[HIGH]**
- **December 9, 2025: MCP was donated to the new Agentic AI Foundation (AAIF) under the Linux Foundation**, co-founded by Anthropic, Block, and OpenAI with support from Google, Microsoft, AWS, Cloudflare, and Bloomberg. Block's `goose` and OpenAI's `AGENTS.md` were also contributed. **[HIGH]**

### Agent2Agent (A2A) — agent-to-agent

- Announced by **Google on April 9, 2025** (Apache-2.0) for cross-vendor agent discovery, task delegation, and coordination, with 50+ (later 100+) partners. **[HIGH]**
- **Donated to the Linux Foundation; the Agent2Agent project launched June 23, 2025** for vendor-neutral governance. **[HIGH]**
- MCP and A2A are **complementary, not competing** — MCP connects agents to tools, A2A connects agents to each other; production systems commonly run both. **[HIGH]**

### Other protocols

- **ACP (Agent Communication Protocol)** — IBM Research, May 2025; REST-native, fully OpenAPI-specified, no special SDK; donated to the Linux Foundation via the Cisco-organized **AGNTCY** collective (~July 2025). Note a naming collision: Zed's "Agent Client Protocol" is a different ACP. **[HIGH/MED]**
- **AP2 (Agent Payments Protocol)** — Google, September 16, 2025; lets an agent cryptographically prove a user authorized a specific purchase using verifiable digital credentials ("mandates"); usable as an extension of A2A/MCP; 60+ partners including Mastercard, Amex, PayPal, Coinbase. **[HIGH]**

### Security — the open wound

The central unsolved problem is **prompt injection via the tool layer**. **Tool poisoning** hides malicious instructions in MCP tool descriptions/metadata that get passed unvalidated into the model's context. **[HIGH]** Documented issues include MCP-related CVEs (e.g., MCPoison / CurXecute, 2025) **[MED]** and a real-world Supabase/Cursor incident where an agent with privileged DB access executed attacker text embedded in a support ticket. **[MED]** Function-calling itself standardized around OpenAI-style JSON-Schema + Structured Outputs (`strict: true`), with MCP serving as the cross-vendor tool-definition layer. **[HIGH]**

---

## Part 4 — Reasoning, planning, and multi-agent systems

**RL-trained reasoning + test-time compute is the dominant paradigm.** DeepSeek-R1 (Jan 2025) showed strong reasoning can be elicited by large-scale RL — R1-Zero used *pure* RL with no SFT, lifting AIME'24 pass@1 from 15.6% to 71.0% and matching o1-level performance. **[HIGH]** Extending reasoning length at inference reliably raises accuracy, now productized as "deep thinking" tiers across o-series, GPT-5 (Aug 2025), Claude's extended/**interleaved** thinking (reasoning between tool calls), and Gemini 3. **[HIGH]**

**Planning is still a distinct weakness.** Self-correction methods (Reflexion, Self-Refine, CRITIC, Chain-of-Verification) are foundational but limited: the bottleneck is error *detection*, not correction, and intrinsic self-correction is unreliable without an external validation signal. The field is shifting to **RL-trained** self-correction (e.g., SCoRe). A recognized theme: *reasoning ability does not automatically confer planning ability* for long-horizon tasks. **[HIGH on the theme; MED on specific figures]**

**Multi-agent orchestration is validated but expensive.** Anthropic's orchestrator-worker Research system (a lead agent + parallel subagents) beat single-agent Opus 4 by **90.2%** on its internal research eval — but used **~15× more tokens** than chat, and Anthropic explicitly notes coding is a poor fit (few parallelizable subtasks, weak real-time coordination). Token usage alone explained ~80% of performance variance on BrowseComp. **[HIGH]** Production systems are converging on ~5 patterns — orchestrator-worker, swarm, mesh, hierarchical, pipeline — with the orchestrator as the recognized latency floor and single point of failure. **[MED]**

---

## Part 5 — Computer-use and browser agents

2025 was the year computer-use agents consolidated:

- **Anthropic** introduced Computer Use with Claude 3.5 Sonnet (Oct 2024); **Claude for Chrome** launched as a research preview (Aug 2025, ~1,000 testers), expanding to Max subscribers by Nov 2025. Safety work cut prompt-injection attack success from 23.6% to 11.2% in autonomous mode. **[HIGH]**
- **OpenAI** launched **ChatGPT agent (July 17, 2025)**, merging Operator (browser action-taking via a virtual computer), deep research, and conversation into one system — effectively absorbing the standalone Operator. **[HIGH]**
- **Google** released the **Gemini 2.5 Computer Use** model (public preview, model card Oct 7 2025) via the Gemini API, with Project Mariner's technology folding into Gemini "Agent Mode." A blog claim that Google "killed Project Mariner" in May 2026 contradicts all primary evidence and is **likely false**. **[HIGH; the shutdown claim is LOW/likely false]**

Prompt-injection robustness is the central open problem across all three.

---

## Part 6 — Benchmarks, memory, and notable releases

### Long-horizon autonomy: the headline metric

**METR**'s result is the most-cited quantitative trend: the length of software task a frontier agent completes with 50% reliability has **doubled roughly every 7 months** over ~6 years, and *faster* (~3–4 months) since 2024. **[HIGH]** METR released "Time Horizon 1.1" (Jan 29, 2026), expanding to 228 tasks. **[MED-HIGH]** Specific per-model horizon figures circulating in secondary sources should be checked against metr.org/time-horizons before quoting. **[LOW]**

### Agent benchmarks (and their caveats)

- **SWE-bench Verified** (500 real GitHub issues) is the de facto coding-agent benchmark; frontier models clustered in the mid-to-high 70s through late 2025, with **Claude Opus 4.5** claiming the lead (~80%+). Competing leaderboards report different numbers for the same models. **[HIGH on the cluster; MED on exact scores]**
- **τ²-bench (tau2-bench)** — Sierra's benchmark measuring *policy adherence*, not just task completion. **[HIGH]**
- **GAIA** — 466 real-world assistant tasks; scores swing from ~44% to ~92% depending on bare-model vs. scaffolded vs. full-system setups, so numbers are **not comparable across leaderboards**. **[HIGH caveat]**
- **OSWorld** — 369 desktop tasks, human baseline ~72%; Claude Sonnet 4.5 = 61.4% (primary); Simular's Agent S2 reportedly first to cross the human baseline (Dec 2025). **[HIGH/MED]**
- **BrowseComp** — OpenAI's browsing benchmark (April 2025, 1,266 hard questions); OpenAI Deep Research scored 51.5% vs 1.9% for GPT-4o-with-browsing. A fairer **BrowseComp-Plus** variant is now used by labs including Anthropic. **[HIGH]**
- **Terminal-Bench** (v2.x), **Vending-Bench 2** (long-horizon business management), and Princeton's **Holistic Agent Leaderboard (HAL)** are notable additions, the last explicitly addressing cross-leaderboard inconsistency. **[MED-HIGH]**

### Memory systems

Four open-source approaches lead in 2026, architecturally distinct: **Mem0** (vector-first bolt-on layer; ~$24M Series A Oct 2025), **Zep/Graphiti** (temporal knowledge graph for changing facts), **Letta** (full agent runtime, MemGPT lineage, model pages its own memory OS-style; ~$10M seed), and **Cognee**. On LongMemEval, graph-based Zep (~63.8%) notably beat Mem0 (~49.0%) on temporal fact retrieval. **[MED-HIGH on framework taxonomy; MED on funding/scores]** Meanwhile frontier vendors are **building memory into the models** — Anthropic shipped a memory tool plus context compaction in Opus 4.5 — partially overlapping with third-party memory products. **[HIGH]**

### Notable agentic model/product releases (2025–2026)

| Release | Date | Agentic note | Confidence |
|---|---|---|---|
| **Claude Sonnet 4.5** | Sep 29 2025 | SOTA SWE-bench at launch; OSWorld 61.4% | HIGH |
| **Gemini 3 Pro** | Nov 18 2025 | Strong agentic/coding (SWE-bench ~76.2%); launched with Antigravity agentic IDE | HIGH |
| **Claude Opus 4.5** | Nov 24 2025 | "Best for coding, agents, computer use"; effort parameter, context compaction, memory tool; deep-research eval rose 70.5%→85.3% with memory+context tools | HIGH |
| **GPT-5** | Aug 2025 | Agentic coding focus (~74.9% SWE-bench per secondary) | HIGH release / MED score |
| **Manus** | Mar 6 2025 | Autonomous general agent; reportedly acquired by Meta (~$2–3B, Dec 2025) | HIGH launch / MED acquisition |
| **Genspark Super Agent** | Apr 2 2025 | 9 LLMs + 80+ tools; GAIA claims | MED |
| **Devin (Cognition)** | ongoing | Autonomous SWE agent, billed in "Agent Compute Units" | MED |

> ⚠️ **Fabrication watch.** Aggregators in mid-2026 confidently cite models like "Opus 4.6/4.8," "GPT-5.5," "Fable 5," "Mythos 5," and "GLM-4.7-Flash" with precise leaderboard scores. Some of these names appear to be **real next-generation models** (Anthropic's site references Fable and Mythos, and a ~June 2026 US export-control directive reportedly suspended foreign-national access to them), but **their specific benchmark scores are aggregator-only and unverified**. Do not cite those scores as fact.

---

## Synthesis: five things that actually changed

1. **Agentic skill is now trained, not prompted.** RL on reasoning traces + test-time compute is the through-line connecting R1, GPT-5, Claude 4.x, and Gemini 3.
2. **The interoperability war is over — and it's a layered peace.** MCP (tools) + A2A (agents), both now under the Linux Foundation's Agentic AI Foundation, with AP2 extending to payments.
3. **Autonomy is measurable and rising fast.** METR's doubling-time metric reframed "how capable is this agent" into "how long a task can it own," and the curve is steepening.
4. **Multi-agent works where parallelism is real, and is wasteful where it isn't.** Validated for research (Anthropic's 90.2%), explicitly weak for coding; ~15× the token cost.
5. **Open + decentralized is a live alternative path.** Nous Research is the clearest example: open-weight Hermes models, decentralized training (Psyche/DisTrO), and a self-improving open-source runtime (Hermes Agent) that became 2026's fastest-growing agent project.

**The shared bottleneck across all of it is security** — prompt injection through the tool/computer-use layer remains the unsolved problem gating real autonomy.

---

## Sources

**Nous Research / Hermes**
- https://huggingface.co/NousResearch/Hermes-4-70B
- https://huggingface.co/NousResearch/Hermes-4-14B
- https://huggingface.co/NousResearch/Hermes-4.3-36B
- https://huggingface.co/NousResearch/Hermes-3-Llama-3.1-405B
- https://arxiv.org/abs/2508.18255
- https://arxiv.org/pdf/2408.11857
- https://nousresearch.com/introducing-hermes-4-3
- https://nousresearch.com/hermes3
- https://psyche.network
- https://github.com/PsycheFoundation/psyche
- https://github.com/NousResearch/hermes-agent
- https://github.com/NousResearch/hermes-agent/releases
- https://github.com/NousResearch/hermes-agent-self-evolution
- https://openrouter.ai/nousresearch/hermes-4-405b
- https://oakresearch.io/en/analyses/innovations/nous-research-psyche-open-source-decentralized-ai-revolution
- https://app.dealroom.co/news/note/hermes-agent-hits-99k-github-stars-in-8-weeks-fastest-growing-open-source-agent-framework-of-2026
- https://www.star-history.com/nousresearch/hermes-agent/
- https://the-agent-report.com/2026/06/hermes-agent-188k-stars-90k-skills-ecosystem-june2026/
- https://www.marktechpost.com/2026/06/03/nous-research-releases-hermes-desktop-a-native-cross-platform-front-end-for-hermes-agent-v0-15-2-with-streaming-tool-output/

**Frameworks**
- https://www.langchain.com/blog/langchain-langgraph-1dot0
- https://changelog.langchain.com/announcements/langgraph-1-0-is-now-generally-available
- https://fortune.com/2025/10/20/exclusive-early-ai-darling-langchain-is-now-a-unicorn-with-a-fresh-125-million-in-funding/
- https://openai.com/index/the-next-evolution-of-the-agents-sdk/
- https://techcrunch.com/2026/04/15/openai-updates-its-agents-sdk-to-help-enterprises-build-safer-more-capable-agents/
- https://learn.microsoft.com/en-us/agent-framework/overview/
- https://visualstudiomagazine.com/articles/2026/04/06/microsoft-ships-production-ready-agent-framework-1-0-for-net-and-python.aspx
- https://google.github.io/adk-docs/
- https://cloud.google.com/products/agent-builder
- https://code.claude.com/docs/en/agent-sdk/overview
- https://www.anthropic.com/engineering/building-agents-with-the-claude-agent-sdk
- https://github.com/crewaiinc/crewai
- https://github.com/pydantic/pydantic-ai

**Protocols & security**
- https://modelcontextprotocol.io/specification/2025-11-25
- https://blog.modelcontextprotocol.io/posts/2025-11-25-first-mcp-anniversary/
- https://blog.modelcontextprotocol.io/posts/2025-09-08-mcp-registry-preview/
- https://www.anthropic.com/news/donating-the-model-context-protocol-and-establishing-of-the-agentic-ai-foundation
- https://www.linuxfoundation.org/press/linux-foundation-announces-the-formation-of-the-agentic-ai-foundation
- https://techcrunch.com/2025/12/09/openai-anthropic-and-block-join-new-linux-foundation-effort-to-standardize-the-ai-agent-era/
- https://www.linuxfoundation.org/press/linux-foundation-launches-the-agent2agent-protocol-project-to-enable-secure-intelligent-communication-between-ai-agents
- https://developers.googleblog.com/en/google-cloud-donates-a2a-to-linux-foundation/
- https://auth0.com/blog/mcp-specs-update-all-about-auth/
- https://cloud.google.com/blog/products/ai-machine-learning/announcing-agents-to-payments-ap2-protocol
- https://ap2-protocol.org/
- https://simonwillison.net/2025/Apr/9/mcp-prompt-injection/
- https://www.truefoundry.com/blog/blog-mcp-tool-poisoning-gateway-defense

**Reasoning, multi-agent, computer use**
- https://arxiv.org/html/2501.12948v1
- https://www.anthropic.com/news/claude-opus-4-5
- https://platform.claude.com/docs/en/build-with-claude/extended-thinking
- https://www.anthropic.com/engineering/multi-agent-research-system
- https://www.anthropic.com/news/claude-for-chrome
- https://www.anthropic.com/news/3-5-models-and-computer-use
- https://openai.com/index/introducing-chatgpt-agent/
- https://blog.google/innovation-and-ai/models-and-research/google-deepmind/gemini-computer-use-model/
- https://metr.org/blog/2025-03-19-measuring-ai-ability-to-complete-long-tasks/
- https://metr.org/time-horizons/
- https://metr.org/blog/2026-1-29-time-horizon-1-1/

**Benchmarks, memory, releases**
- https://www.anthropic.com/news/claude-sonnet-4-5
- https://blog.google/products/gemini/gemini-3/
- https://openai.com/index/introducing-gpt-5/
- https://github.com/sierra-research/tau2-bench
- https://leaderboard.steel.dev/leaderboards/swe-bench-verified/
- https://www.tbench.ai/leaderboard
- https://os-world.github.io/
- https://arxiv.org/pdf/2504.12516
- https://github.com/texttron/BrowseComp-Plus
- https://andonlabs.com/evals/vending-bench-2
- https://arxiv.org/pdf/2510.11977
- https://particula.tech/blog/agent-memory-frameworks-tested-mem0-zep-letta-cognee-2026
- https://mem0.ai/blog/state-of-ai-agent-memory-2026
- https://www.vellum.ai/blog/claude-opus-4-5-benchmarks

*Compiled via a multi-agent deep-research pass: five parallel search angles, source triangulation, and a verification sweep on load-bearing claims. Confidence tags and fabrication warnings reflect deliberate skepticism toward the SEO-polluted 2026 web — verify LOW-tagged figures against the linked primary sources before relying on them.*
