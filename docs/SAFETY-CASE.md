# Oh-Ben-Claw — Physical-Action Safety Case

*A structured safety argument for the physical-action layer, framed in
functional-safety and AI-safety standards language so it is legible to an
auditor. Written 2026-07-03. Companion to `docs/EMBODIED-ARCHITECTURE.md`
(architecture), `SECURITY.md` (threat model + OWASP ASI coverage), and the
Track 0 track in `ROADMAP.md`.*

> **Status.** This is an internal safety-case *argument*, not a third-party
> certification. It states the safety claim, the argument, and the evidence
> in the vocabulary of ISO/IEC TR 5469, IEC 61508 / ISO 13849, and NIST's
> adversarial-ML guidance so that a functional-safety reviewer can evaluate
> it. No claim of formal compliance or certification is made.

---

## 1. The claim

**No command originating from the LLM — however the model was reasoned into
issuing it, including via prompt injection or a compromised plan — can drive a
physical actuator outside deterministic, operator-defined limits.**

The corollary that shapes the whole design:

> **The LLM proposes; a deterministic gate disposes.**
> The AI model is explicitly **not** the safety function.

This mirrors the central doctrine of **ISO/IEC TR 5469:2024** (*Artificial
intelligence — Functional safety and AI systems*): where an AI element cannot
be assigned the necessary safety integrity, the safety function is realized by
a **separate, non-AI element** that constrains the AI's outputs. In OBC that
element is **Track 0** — a deterministic check the model cannot modify,
disable, or reason its way around.

---

## 2. Why the AI is not the safety function

An LLM is, for safety-integrity purposes, an untrusted component: its output
distribution is not bounded, not fully testable, and manipulable by adversarial
input (NIST AI 100-2e2025 catalogues the attack classes — evasion, prompt
injection, poisoning). Assigning a hard safety function to it would be
unjustifiable. So OBC treats **every** LLM-issued tool call as a *request* that
must survive a chain of deterministic checks before any actuation occurs. The
safety property is a function of the gate, not of the model's good behavior —
which is exactly the property a 61508-style analysis needs: **independence of
the safety function from the untrusted channel.**

---

## 3. The safety function: Track 0

Track 0 is realized as a layered set of deterministic mechanisms. Each is
model-independent code; none can be relaxed by anything the LLM emits.

| # | Mechanism | Where | Guarantee |
|---|---|---|---|
| 3.1 | **Deterministic actuator gate** — `SafetyGate::check(node, tool, pin, value, now)`: pin allow-list, value range, minimum inter-actuation interval (rate limit) | `src/security/limits.rs` (host) **and** mirrored in `firmware/` on the MCU | A pin/value/rate outside policy is refused **before** hardware is touched. Default-deny. |
| 3.2 | **Physical-risk classification** — every tool declares a `RiskClass` (`physical`, `reversible`, `BlastRadius`) | `src/tools/traits.rs` | Drives approval defaults; irreversible/high-blast actions require per-call authorization and can never be auto-granted to "forever". |
| 3.3 | **Signed, tamper-evident audit** — every physical-action decision (allowed/denied + args + reason) is appended to an HMAC-chained log | `src/security/audit.rs`, `audit_sign.rs` | Any edit, insertion, deletion, or reorder of the audit trail is detectable (`audit::verify`). Provides the *evidence* record for this safety case. |
| 3.4 | **Argument provenance ("taint") gate** — a privileged call whose argument values derive from untrusted external content (web/MCP/inbound text) is refused (enforce) or flagged (warn) | `src/security/taint.rs` | Blocks the injection→actuation data-flow (CaMeL doctrine; OWASP ASI01/02) independent of what the model "decided". |
| 3.5 | **Dynamic trust** — a node whose behaviour is anomalous (latency/failure) is demoted; its physical actions are refused | `src/security/trust.rs` | Trust can only *tighten* the decision, never relax it. |
| 3.6 | **Approval gate** — under `supervised`/`manual` autonomy every actuation needs an explicit operator grant | `src/approval/` | A permissive setting is required to auto-run; it is never *assumed*. |
| 3.7 | **Staged rollout** — synthesized/learned physical skills climb `simulate → supervised → autonomous`; at `simulate` the chokepoint dry-runs (nothing actuates); promotion is operator-gated on a clean record; a failed supervised run auto-demotes | `src/skill_forge/rollout.rs`, `src/agent/mod.rs` | A skill the system *taught itself* cannot actuate unattended until an operator has promoted it on evidence. |

**Ordering matters:** these run at one chokepoint (`Agent::execute_tool_inner`),
and **delegate/sequence skills are resolved to their real underlying call
before the gate runs**, so the gate always evaluates the actual actuator
command — never a wrapper that could hide it.

---

## 4. Defense in depth (fail-safe layering)

Per IEC 61508 / ISO 13849 practice, the safe state is reachable independently
at multiple layers, and a failure at any layer is contained by the one below
(from `docs/EMBODIED-ARCHITECTURE.md` §Safety model):

1. **Risk-classed tools** — reads/stops always allowed; physical/high-blast actions approval-gated.
2. **Track 0 gate** — deterministic per-call bounds, host **and** MCU.
3. **Reflex safing** — sub-second, LLM-free reactions to bad modes (battery-critical, link-loss), escalation-budgeted.
4. **Mission guards** — a tripped guard preempts a multi-step plan and halts the platform.
5. **Firmware self-safing** — the node protects itself (battery/link watchdogs) when the host or spine is gone; host-pushed rules merge *after* the built-ins, so a node never loses self-protection.

The load-bearing property for the safety case is **3.1's on-MCU mirror**: the
deterministic actuator limit lives in firmware, so an out-of-range command is
refused **even if the host is compromised or offline**. The safety function
does not depend on the availability or integrity of the cognition layer.

---

## 5. Adversarial robustness (the safety case under attack)

The claim in §1 is explicitly *adversarial* — it must hold when the model is
manipulated. Evidence:

- **Injected-prompt → actuation** is blocked by 3.4 and backstopped by 3.1.
  Tested adaptively (not against a frozen string): `tests/evals.rs::taint_redteam`
  and `asi_redteam` generate a seed-sampled *family* of injection framings
  (`src/security/redteam.rs`) and assert no variant drives an out-of-limit
  actuation — implementing NIST's finding that a static corpus understates
  hijack risk (~11%→81% adaptive).
- **Self-taught malicious skill → actuation** is blocked by 3.7 (quarantine +
  verification + staged rollout); `tests/evals.rs::staged_rollout` pins that a
  simulate-stage actuator skill never actuates even when the model calls it,
  and that a failed supervised run auto-demotes.
- **Compromised/anomalous node → actuation** is blocked by 3.5.
- **Audit tampering** is detectable by 3.3.

Coverage is mapped to the **OWASP Top 10 for Agentic Applications** (ASI01/02/04/05/06/08)
in `SECURITY.md`, each category tied to a control *and* an executable eval.

---

## 6. Residual risk & honest limitations

A safety case is only credible if it states what it does **not** guarantee:

- **Taint tracking is a heuristic** (substring/boundary matching, not dataflow
  through the LLM, which is not tractable). It is biased to catch the common
  "fetched text steers an actuator" attack; a value the model *transcribes in a
  different form* (e.g. spells out a number) can evade the taint leg — which is
  exactly why **3.1 is the backstop**: the deterministic gate refuses an
  out-of-range pin regardless of provenance. Taint reduces the attack surface;
  the SafetyGate is what makes the claim hold.
- **The gate is only as good as its configured limits.** An operator who
  allow-lists a dangerous pin/range has widened the safe envelope; Track 0
  enforces the policy, it does not author it.
- **No third-party certification.** This document argues the case; it is not a
  certificate. Formal assessment against IEC 61508/ISO 13849 SIL/PL would
  require independent analysis (fault-tree/FMEA, diagnostic coverage figures,
  proof-test intervals) that is out of scope here.
- **Firmware mirror scope.** The on-MCU gate covers the actuator classes
  implemented in firmware; new actuator types must add their limit enforcement
  on-chip, not only host-side, to preserve the §4 host-independence property.

---

## 7. Standards touchpoints (informative)

| Standard / guidance | How OBC's design relates |
|---|---|
| **ISO/IEC TR 5469:2024** — AI & functional safety | Core doctrine: the AI is not the safety function; a separate non-AI element (Track 0) realizes it. §2, §3. |
| **ISO/IEC TS 22440** (in development, 2026) | Successor guidance on AI functional safety; align the case when published (watch item). |
| **IEC 61508 / ISO 13849** — functional safety / safety of machinery | Fail-safe state, independence of the safety function, default-deny, watchdogs, defense in depth. §3, §4. |
| **NIST AI 100-2e2025** — Adversarial ML taxonomy | Vocabulary for the threat model (evasion, prompt injection, poisoning). §2, §5. |
| **OWASP Top 10 for Agentic Applications (Dec 2025)** | Category mapping + adaptive red-team evidence. §5, `SECURITY.md`. |
| **NIST agent-hijacking guidance (2025)** | Justifies adaptive (seed-sampled) red-teaming over a frozen corpus. §5. |

---

*Evidence lives in the code and tests referenced throughout. The one-line
version, for anyone auditing this project: **an out-of-limit actuator command
is refused by deterministic code the model cannot override, on the host and on
the microcontroller, and every such decision is written to a tamper-evident
log.***
