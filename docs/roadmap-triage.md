# Roadmap triage: the 39 `[~]` rows from 8dbe031

Commit 8dbe031 changed 39 ticked rows in `ROADMAP.md` to `[~]` because the code they name is not in the tree. This file gives a verdict for each row. Checked against origin/main `7a86b06` on 2026-10-30, using Grep only over `src/` and `crates/`.

- **fold**: code that does the job already exists under another name. Re-point the row to that code, reword it to say what the code really does, and tick it. This is a ROADMAP edit only.
- **drop**: remove the row, or move it to a "removed" note. Nothing in the tree does this job, and nothing currently needs it.
- **rebuild**: write the code. Each one is a small change to an existing file.

Size is for the verdict as given. **S** is under an hour or a doc edit, **M** is a day or less, **L** is several days. The note says what rebuilding would cost wherever the verdict is fold or drop.

**Totals:** 16 fold, 21 drop, 2 rebuild.

| Row (ROADMAP line) | Verdict | Existing code it relates to | Size | Note |
|---|---|---|---|---|
| 131 Sensor fusion (`src/peripherals/fusion.rs`) | fold | `crates/obc-telemetry/src/sensing.rs` `SensingController` (per-quantity quality, `ingest` :151); `crates/obc-navigation/src/pose_fusion.rs` `fuse` :84 | S | Reword it to say "per-quantity classification + pose fusion". The code has no general cross-sensor combiner. |
| 133 Terminal telemetry TUI (`src/dashboard/`) | drop | Gateway web UI `crates/obc-gateway/src/assets/index.html`; status route `routes.rs:829` | S | The web PWA does this job. A ratatui rebuild would be M. |
| 143 Event lifecycle hooks (`src/hooks/`) | fold | `crates/obc-agent/src/handle.rs:40` `AgentEvent` broadcast (Started/Thinking/Token/ToolCall …) | S | The broadcast only observes and cannot intercept anything. Reword the row to say that. Hooks that can veto or rewrite a tool call would be an M rebuild. |
| 144 Multimodal message handling with image markers | fold | `crates/obc-tools/src/builtin/vision.rs` (`EncodedImage`, `to_openai_part`, `to_anthropic_source`) | S | Images go through the `vision` tool. Chat messages in `obc-providers` are text-only, and nothing parses image markers. Putting images into chat messages across 5 providers would be L. |
| 145 RAG over hardware datasheets (`src/rag/`) | drop | `crates/obc-memory/src/embed.rs` `FastEmbedder`; `trajectory.rs` cosine retrieval; experience retrieval `obc-agent/src/lib.rs:410` | S | No datasheet corpus exists. A rebuild on top of `embed.rs` and the trajectory store would be M. |
| 146 Sandboxed tool runtime, native + Docker (`src/runtime/`) | fold | `crates/obc-tools/src/builtin/shell.rs` `ShellBackend::{Local,Docker}`; `ShellConfig` `crates/obc-config/src/lib.rs:~1883` | S | `[shell] backend = "docker"` is the sandbox. Only the shell is sandboxed, and the hardware tools go through the Track 0 gate. |
| 147 Config sections `[autonomy]` `[cost]` `[runtime]` `[multimodal]` | fold | `Config.autonomy`, `Config.cost`, `[shell]` in `crates/obc-config/src/lib.rs` | S | Tick autonomy and cost. Change `[runtime]` to `[shell]`. Strike `[multimodal]`. |
| 196 `PersonalityStore` (SOUL.md + USER.md) | fold | `crates/obc-memory/src/notes.rs` (`MEMORY.md`/`USER.md` targets) | S | USER.md is real. SOUL.md has no counterpart beyond `system_prompt` in config. Adding a SOUL target to `notes.rs` would be S. |
| 197 `build_system_prompt()` | fold | `crates/obc-agent/src/lib.rs:1579` appends notes to `config.system_prompt` | S | The code under a new name does what the row describes. |
| 275 `Config` gains `proxy` + `personality` | fold | `Config.proxy` real | S | Keep the proxy half and strike the personality half. |
| 276 `FeishuConfig` / `ProxyConfig` / `PersonalityConfig` | fold | `FeishuConfig`, `ProxyConfig` in `crates/obc-config/src/lib.rs` | S | Same split: two of the three structs are real. |
| 309 `ClawHubEntry` | drop | none (`obc-skill-forge` has no `registry`) | S | ClawHub group, decide 309–312, 333 and 491–496 together. Rebuilding the registry client with install security would be L. |
| 310 `SkillRegistryIndex` | drop | none | S | ClawHub group. |
| 311 `ClawHubClient` (+ `install()`) | drop | `crates/obc-skill-forge/src/lib.rs:596` `install_skill` (local manifests only) | S | ClawHub group. |
| 312 `pub mod registry` | drop | none | S | ClawHub group. |
| 320 `ImageEntry` | drop | ClawCam percepts into `WorldMemory` (`crates/obc-vision/src/camera_percepts.rs`, `clawcam_ingest.rs`) | S | Image group, 320–322. Camera output becomes facts, not stored images. A SQLite image store would be M. |
| 321 `ImageMemoryStore` | drop | same | S | Image group. |
| 322 `pub mod image` export | drop | none | S | Image group. |
| 323 `vector.rs` return-type fix | drop | `crates/obc-memory/src/embed.rs`, `trajectory.rs` (the store that replaced it) | S | `vector.rs` was deleted on 2026-09-12 (see `obc-memory/Cargo.toml:25`). |
| 333 `ClawHubConfig` `[clawhub]` | drop | none | S | ClawHub group. |
| 334 `Config` gains `browser` + `clawhub` | fold | `BrowserConfig` in `crates/obc-config/src/lib.rs` | S | Keep the browser half and strike the clawhub half. |
| 406 Image source resolution `resolve_image_source()` | fold | `vision.rs` `encode_local_image` :58 / `encode_remote_image` :86 | S | The tool already chooses between a path and a URL. |
| 407 MIME whitelist `validate_mime_type()` | rebuild | `vision.rs`: the local path maps the extension to a MIME type, and the remote path trusts `Content-Type` | S | A real gap: a remote URL can return any content type. Add the whitelist in `vision.rs`. |
| 408 Image size limit `validate_image_size()` | rebuild | `vision.rs` (no byte limit on either path) | S | A real gap: a remote fetch has no size limit. Add a configurable cap in `vision.rs`. |
| 409 Local image fetching `fetch_local_image()` | fold | `vision.rs:58` `encode_local_image` | S | The same code under another name. |
| 410 Batch `prepare_images()` | drop | none | S | Nothing passes more than one image. |
| 419 `WasmRuntime` | drop | Docker shell backend (row 146) | S | WASM group, 419–421. No `wasmtime` dependency. A real WASM sandbox would be L. |
| 420 `WasmConfig` / `RuntimeConfig` | drop | none | S | WASM group. |
| 421 "Framework-ready for wasmtime" | drop | none | S | WASM group. |
| 433 `StreamingToolCallAccumulator` | fold | `crates/obc-providers/src/anthropic.rs:435` accumulates `input_json_delta` into tool-call args; `StreamDelta` / `chat_completion_streaming` `obc-providers/src/lib.rs:199,239` | S | Only Anthropic really streams. OpenAI-compatible providers fall back to the default send-once path, and real streaming for them would be M. |
| 434 `StreamingResponseBuilder` | fold | same | S | Folds into the same row as 433. |
| 435 Integrates with `ToolCall` | fold | `obc-providers/src/lib.rs:123` `ToolCall` | S | Merge into 433. |
| 491 Operator approval for skill installs (`InstallConsent`) | drop | `crates/obc-approval` `ApprovalManager`; `obc-skill-forge` `install_skill` | S | ClawHub group. If ClawHub is rebuilt, run installs through `obc-approval`. |
| 492 Checksum verification of skill packages | drop | none (no sha256 in skill-forge) | S | ClawHub group. |
| 493 Version pinning `[clawhub.install_policy]` | drop | none | S | ClawHub group. |
| 494 `InstallInspection` static flagging | drop | `crates/obc-safety/src/redteam.rs` names `install_policy` in a doc comment | S | ClawHub group. Fix that doc comment when the group is dropped. |
| 495 Local vetted mirror (allowlist) | drop | none | S | ClawHub group. |
| 496 Install audit log (JSONL) | drop | `obc-approval` audit log (in-memory, `lib.rs:155`) | S | ClawHub group. |
| 646 Perception→memory→action wiring (`FusionRegistry::observe_into`) | fold | `crates/obc-telemetry/src/sensing.rs:151` `SensingController::ingest` writes `sensor.{quantity}` facts into `WorldMemory` | S | The row already says to read it this way. Re-point it and tick it. |

## Decision groups

Most of the 39 rows come down to six decisions:

1. **Doc-only folds**: the 16 fold rows. Re-point and tick each one in a single ROADMAP edit.
2. **ClawHub** (309–312, 333, 491–496; 11 rows): drop, or rebuild as an L project.
3. **WASM runtime** (419–421): drop, or rebuild as an L project.
4. **Image memory store** (320–322): drop, or rebuild as M.
5. **Vision hardening** (407, 408): two S code changes in `crates/obc-tools/src/builtin/vision.rs`. These are the only rows where the missing code leaves a real gap.
6. **The rest** (133 TUI, 145 RAG, 323, 410): drop.
