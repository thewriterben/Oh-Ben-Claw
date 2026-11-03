# Always-on personal agents vs. Oh-Ben-Claw (November 2026)

Three always-on personal agents launched within seven weeks of each other:

| Agent | Vendor | Launched | Reached through |
|---|---|---|---|
| **Grok Bot** | xAI | early beta, 2026-08-11 | desktop and mobile apps, X Chat, `@bot` on X, Slack |
| **Muse** | Meta | US, 2026-09-08 | iOS, Android, web, WhatsApp; AI glasses planned |
| **Dots** | OpenAI | DevDay, 2026-09-29 | ChatGPT (chat or call), Slack, Teams; texting planned |

**How reliable the sources are.** For Grok Bot the source is xAI's own FAQ. For Dots
it is TNW's write-up of the DevDay launch. For Muse it is The Decoder quoting
Meta, plus launch coverage. Muse's model lineage ("Hatch") and Dots' model name
("GPT-6 Astra") come from secondary reporting only. None of the vendors has
published robustness figures for its safeguards.

## The shared pattern

All three products work the same way. Each one is a persistent agent with
its own computer (browser, terminal, files). You give it a goal, and it keeps
working after you close the app. It checks in when something changes or when it
needs approval. It uses your credentials without seeing them. It reaches apps
through connectors, and through the browser when there is no connector. A
separate safety component watches what it does. The differences are in the
details:

| Capability | Grok Bot | Muse | Dots | Oh-Ben-Claw today |
|---|---|---|---|---|
| Runs while you're away | One persistent cloud computer per user | "Muse Secure VM" per user | Cloud computer per dot; can connect to your laptop | **Have.** The daemon runs on your own host, with no cloud VM |
| Own browser, shell and files | Yes | Yes | Yes | **Have.** `shell`, `file`, and 7 CDP browser tools |
| Standing goals that keep moving | Routines on a schedule, or after an event | Larger goals; checks in on change | Several goals at once, worked around the clock | **Partial.** Cron prompts (`crates/obc-agent/src/scheduled.rs`) and physical missions. No goal object with progress or a done condition |
| Proactive background work | — | Makes suggestions | "Proactive research", **read-only** | **Gap.** The heartbeat was dropped 2026-10-30 because nothing wired it |
| Posture with no one watching | Background approvals **expire after ~30 min** | Asks before sends and purchases | Background work **cannot send or change anything** | **Have** (2026-11-01). Scheduled and A2A turns run unattended: `full` autonomy is not a grant there, and a session grant covers them for 30 min |
| Approvals reach you where you are | Apps, X Chat, Slack | WhatsApp | ChatGPT, Slack, Teams | **Have** (2026-11-01). Telegram and Slack show an unattended call with Approve / Deny buttons, and it waits for the tap. Since 2026-11-02 an approver's own chat turn is asked the same way, in that chat. The gateway also records out-of-band decisions |
| Allow / block / ask rules | "Ask first" for send, publish, delete, purchase, prod | Per-app scope (e.g. read-only mail) | Custom Rules per action | **Have, stronger.** `[[security.policies]]`, autonomy levels, plan-mode argument bounds, Track 0 mirrored on the MCU |
| A second agent reviews actions | Auto-review rules | **Sentinel**: a second agent that must approve all internet access | Auto-review of account and sharing actions; a monitor can pause the dot | **Partial.** The checks are deterministic: taint tracking (default `warn` since 2026-11-01) and the conscience egress allowlist. No model-based reviewer |
| Credentials it can use but not see | Vault, password manager, Secure Form | Credential store it can use but not read; Stripe Link single-use cards | Saved passwords hidden from the model | **Partial.** The vault exists (`crates/obc-safety/src/vault.rs`). I found no path that fills a browser field from the vault without the secret entering the model's context |
| Teach by demonstration | Records a browser workflow of up to 10 minutes and turns it into a draft skill | — | — | **Gap.** Skill Forge builds skills from text. Nothing records |
| Specialised agents | Bots with set jobs that hand off and message each other. **The shared computer is not a security boundary** | Muse plus Sentinel | Specialist dots, each with its own identity and credentials | **Partial.** Sub-agent pool (`crates/obc-agent/src/delegation_tools.rs`) and fleet coordination. Not checked: whether a sub-agent gets narrower tool or credential scope than its parent |
| Memory you can inspect | You can ask a bot what it remembers | Forgets on request | Learns from feedback | **Have, stronger substrate.** Bitemporal world memory with provenance and withdrawal (`world.rs`). The chat path to it is rough |
| Model selection | Picked per task; the user cannot choose | Not disclosed | Not disclosed | **Have, better.** Explicit local-to-cloud routing (P1–P3), audited |
| Data custody | Cloud storage required | User-key "Confidential VM" planned | Cloud | **Have.** Local-first by design |
| Physical world | — | — | — | **Core.** Bodies, sensors, reflexes, missions, fleet |
| Templates | Shareable bot templates that leave out logins and history | — | — | **Gap.** ClawHub install is built but unwired |

## Where Oh-Ben-Claw is already ahead

- **It has bodies.** None of the three can touch hardware, and no cloud agent can
  offer Track 0's deterministic limit table on the microcontroller.
- **Its safety holds without trusting the model.** Sentinel and auto-review
  are models judging models. OBC's limit table, policies, taint tracking,
  egress allowlist and signed audit with replay are deterministic. On the
  physical side that is the stronger guarantee.
- **It is local-first.** Muse is still working toward a user-keyed VM. OBC
  never left the user's machine.
- **Model routing is visible.** Grok Bot will not let you pick or see the model.
  OBC's escalation axes are explicit and audited.
- **It speaks open protocols.** MCP server and client, plus an A2A server. The
  three products are closed ecosystems.

## What to add or upgrade, ranked

Effort is S, M or L. Every item has to follow the repository's rule: wire it
in `main.rs` and test it through the real loop, because four controls here have
already been built, configured, tested and never invoked.

### 1. Unattended posture and approval expiry — S (done 2026-11-01)

*As built:* OBC had no pending-approval queue (a call that needed approval was
refused; item 2 added one), so expiry became a window on session grants: one covers unattended
work for `[autonomy].unattended_grant_ttl_secs`, default 1800. Gated means
physical, irreversible or with blast radius. P2P peer calls stay attended;
they already answer to dynamic trust and count as external under taint.
The original proposal follows.

*Taken from:* Dots (background work is read-only) and Grok (background approvals expire).
The `ApprovalManager::decide` path gets a presence input: `Attended` or
`Unattended`. Scheduled tasks, goal ticks and A2A calls run `Unattended`. When
unattended, reversible and read-only tools run. Anything irreversible, physical
or external-send queues an approval with an `expires_at` and **never
auto-approves under `Full` autonomy**. An expired approval becomes a denial and
is audited as one. This closes the widest gap in the table, and it matches the
existing rule that the model proposes and deterministic code disposes.

### 2. Approve from the chat you're already in — S/M (done 2026-11-01)

*As built:* a tap that called `record_external_decision` would only grant the
*next* call, and a one-shot task has no next firing, a daily task fires again
after the 30-minute grant window, and an A2A caller cannot be told to retry.
So OBC now has the queue item 1 lacked: an unattended call that needs approval
waits up to `[autonomy].approval_wait_secs` (default 300) while each approver
gets it with Approve / Allow 30 min / Deny buttons. No answer is a refusal,
audited as a denial. Approvers are `[channels.telegram].approver_user_ids` and
`[channels.slack].approver_users`, separate from the chat allowlists; anyone
else's tap is refused and audited. `crates/obc-agent/tests/chat_approval.rs`
covers it through the scheduler loop and the A2A executor. Since 2026-11-02
the scheduler loop runs dispatches concurrently (up to four), so a task
waiting for a tap no longer holds up the tasks due after it.

*Attended (2026-11-02):* when an approver's own message on Telegram or Slack
starts a turn and it reaches a gated call, the call waits the same way, asked
only in that chat (as a reply, or in the Slack thread) with Approve / Allow
this session / Deny. Both channels now read taps while a turn runs: one half
reads updates and answers taps, the other runs the messages in order. Anyone
not on the approver list is refused at once, as before, and a peer agent's
call never uses the chat. The original proposal follows.

*Taken from:* all three. Telegram inline buttons and Slack interactive messages
call `record_external_decision`, which is the function the gateway route
already uses. Only allowlisted channel user IDs can approve. With item 1 in
place, this turns "the robot wants to open the gate" into a phone
notification with a button, where today it is a request that is silently
pending.

### 3. Ship taint tracking on — XS (done 2026-11-01)

*Taken from:* all three ship their guard on by default. Change the default
`[safety].taint_mode` from `off` to `warn`, keeping `enforce` opt-in for
now. This is one line and a changelog entry, and it makes the CaMeL-style
guard visible to every user.

### 4. Standing goals — M (done 2026-11-02)

*As built:* the `goal` tool sets a goal with its text, an optional done
condition, a cadence and an optional end. The record lives in a `goals` table
in `obc-scheduler` beside the scheduled task that ticks it, and both share the
id `goal-<slug>`, so the scheduler loop fires goals like any task and
`scheduled::dispatch` routes them. A tick is one unattended turn, so gated
tools in it go through the approval gate and the chat prompt from item 2,
with the goal named as the origin. The turn sees the goal and its last five
notes and answers with `NOTE:`, `CHANGED:` and `DONE:` lines. The note is
written to world memory as `goal.<slug>.progress`, so the history is the
bitemporal record. The owner is the chat the goal was set from, read from the
turn's session and never from a tool argument; the report goes there only
when something changed, and through the notifier when the goal was set
elsewhere. When the done condition is met the goal closes, its task stops and
its progress belief closes with `SourceStopped`. A goal with no done
condition never closes on the model's say-so; past its `until` it closes as
expired. Goals tick on a cadence only: trigger-driven ticks are a follow-up.
`crates/obc-agent/tests/standing_goals.rs` covers it through the real
scheduler loop. The original proposal follows.

*Taken from:* Dots and Muse. A goal record holds the goal text, a done condition,
a cadence or trigger, the latest progress note, a status, and an owner channel.
Each tick runs one unattended turn (item 1) with the goal and the previous
progress notes. It reports to the owner channel **only when something changed**,
and closes itself when the done condition is met. Embodied goals suit this
well:

- "keep the greenhouse between 18 and 24 °C this week"
- "log every deer on the trail cam until Sunday"
- "tell me when the LoRa node's battery trend says it dies within 48 h"

Build it on `obc-scheduler` and world memory rather than reviving the deleted
heartbeat.

### 5. "Why do you believe that?" and "Forget that" from chat — S (done 2026-11-02)

*As built:* on any attended turn, "why do you believe X" (or "think", "say")
is answered from world memory without a model turn. The answer gives the
value and fact id, what kind of claim it is and who made it, when (UTC and
age), whether its support still stands, and up to five beliefs that rest on
it. Right after that, "forget that" (or "forget <entity>" by exact name)
withdraws it through `withdraw_entity` with an operator closure that names
the chat. The row stays in history, the next report from the same source is
believed again, and the reply lists what lost its grounds. Anything that does
not resolve to a held belief goes to the model, and scheduled and A2A turns
are never read as commands. `crates/obc-agent/tests/belief_commands.rs`
covers it through `AgentHandle::process`. The original proposal follows.

*Taken from:* Grok (ask what it remembers) and Muse (forget on request). The
substrate already beats theirs: it can answer "because sensor X said so at
time T, and here is what depends on it". Expose belief lookup with provenance,
and withdrawal by supersession, as chat-level commands. This is mostly UX on
`world.rs`.

### 6. A local-model action reviewer that can only tighten — M (done 2026-11-02)

*As built:* `[autonomy.review]` with `enabled`, `timeout_ms` (3000) and
`fail_closed` (false). It uses `[edge].local_model` but not edge mode, since
the hosts that run scheduled work are mostly desktops. It reviews unattended
turns only. In an attended turn the owner is there, and a call that needs
approval already asks them in their own chat; a P2P peer cannot be asked to
retry.
It runs after taint as well as the approval policy, so a human is never asked
about a call that taint then refuses, and it is skipped when the operator has
just approved this call in chat. An unavailable review runs the call as
decided by default, because a dead local server should not stop all
unattended work; `fail_closed` turns that into a concern.
`crates/obc-agent/tests/action_review.rs` covers it through the scheduler
loop. The original proposal follows.

*Taken from:* Muse's Sentinel and Dots' auto-review. An optional reviewer at the
chokepoint, using the P3 reflex-tier local model, covers irreversible and
external-send actions. It catches semantic problems that rules miss, such as
"this email gives a stranger the user's home address". **It can escalate to
human approval but can never approve** what a deterministic check refused. That
keeps the safety argument intact.

### 7. Fill browser fields from the vault without exposing secrets — S/M

*Taken from:* all three. Add a `browser_fill_secret { field, secret: "vault:<name>" }`
tool. It resolves the secret inside the tool, never returns the value, and
audits the use. It is scoped by the conscience egress allowlist, so a secret
only fills on its own domain.

### 8. Teach by demonstration — L

*Taken from:* Grok's "teach a task". There are two versions:

- **Browser:** record CDP events and turn them into a draft skill.
- **Embodied, unique to OBC:** record a teleoperated run (T-Deck or GUI joystick
  plus world-memory snapshots) and turn it into a draft mission.

Both drafts are quarantined behind Track 0 until approved, the way synthesised
physical skills already are.

### 9. Sub-agent scopes — M

*Taken from:* Dots (specialist dots get their own identity and credentials) and
the warning in Grok's own FAQ that its bots share one computer, which is no
security boundary. Give each spawned sub-agent a policy scope (tool globs and
vault names) narrower than its parent's, enforced at the chokepoint. Or
document plainly that sub-agents are not a boundary.

## Don't copy

- **A cloud VM per user.** OBC's host is already the agent's own computer, and
  local-first is a strength.
- **Several agents on one shared machine with no boundary.** This is Grok's
  admitted weak point.
- **Hidden model selection.** OBC's audited routing is the better design.
- **A race to 4,000 connectors.** MCP import covers the long tail.
- **Payments.** Out of scope. If it ever comes up, the pattern to borrow is
  Muse's single-use virtual card per transaction.
- **Teams and SMS channels.** Add them only if someone needs them. Slack and
  WhatsApp already cover the check-in use.

## Suggested order

1. ~~Items 1 and 3 in one PR: unattended posture, approval expiry, and taint on by default.~~ Done 2026-11-01.
2. ~~Item 2, approvals from Telegram and Slack, which makes item 1 usable away from the desk.~~ Done 2026-11-01.
3. ~~Items 4 and 5 as a "standing goals" phase.~~ Done 2026-11-02.
4. ~~Item 6, the action reviewer.~~ Done 2026-11-02.
5. ~~Item 7, `browser_fill_secret`.~~ Done 2026-11-02.
6. Items 8 and 9 when needed. Item 8's embodied version is the one no competitor can match.

## Sources

- xAI, Grok Bot FAQ — https://docs.x.ai/grok-bot/faq
- Grok Bot beta coverage — https://app.dealroom.co/news/note/grok-bot-enters-early-beta , https://flaviocopes.com/grok-bot/
- TNW, "OpenAI launches dots, always-on AI agents with their own cloud computers" — https://thenextweb.com/news/openai-dots-always-on-ai-agents-cloud-computers-devday
- Business Today on Dots — https://www.businesstoday.in/technology/artificial-intelligence/story/openai-introduces-dots-always-on-ai-agents-that-work-247-and-act-on-your-behalf-558688-2026-09-30
- The Decoder, "Muse can shop, write emails and negotiate prices for users, all through WhatsApp" — https://the-decoder.com/muse-can-shop-write-emails-and-negotiate-prices-for-users-all-through-whatsapp/
- The Paypers on Muse and Stripe checkout — https://thepaypers.com/payments/news/meta-launches-personal-ai-agent-with-stripe-powered-checkout
- Muse Spark model background — https://builtin.com/articles/meta-muse-spark-ai-model
