# Oh-Ben-Claw Ecosystem Integration — Deployment Generator + Accelerapp

*Compiled June 23, 2026. Companion to `V2-STRATEGY.md`, `V2-HARDWARE-ECOSYSTEM.md`, and `ACCELERAPP-CROSS-POLLINATION.md`.*

This examines the **OBC-deployment-generator** (`F:\Documents\OBC-deployment-generator`) and lays out how to integrate it with Oh-Ben-Claw and Accelerapp into one coherent ecosystem — turning three overlapping codebases into a single pipeline with one source of truth.

---

## 1. What the deployment generator is

An **Expo / React Native + web app** (SDK 54, Expo Router, NativeWind, tRPC/Express backend, Drizzle/MySQL optional) that is, by its own header comment, a TypeScript port of OBC's deployment subsystem:

- `lib/obc-data.ts` — *"Mirrors: `src/deployment/{inventory,planner,scheme}.rs`"*: a hand-written copy of `BoardInfo`/`AccessoryInfo`/`KNOWN_BOARDS`, the `ItemRole`/`FeatureDesire`/`NodeRole` enums, `planDeployment()`, and `generateConfigToml()`.
- `lib/firmware-generator.ts` — reproduces the ESP32-S3 firmware as a template engine (`Cargo.toml`, `.cargo/config.toml`, `src/main.rs`, `config.rs`) keyed off a `FirmwareConfig` + pin presets.
- A polished **3-step wizard** (Inventory → Feature Desires → Review & Generate), board catalog, scheme history (AsyncStorage), TOML output, firmware zip download — all computed on-device, offline-first.
- A mostly-empty backend (`server/routers.ts`: auth + system router, `// TODO: add feature routers here`).

In short: it's the **cross-platform UX front door** to OBC's planner and firmware generator — but it reimplements that logic rather than sharing it.

## 2. The core problem: three sources of truth that will drift

The same three concepts are now implemented in three languages:

| Concept | Oh-Ben-Claw (Rust) — canonical | Deployment generator (TS) | Accelerapp (Python) |
|---|---|---|---|
| **Hardware registry** | `src/peripherals/registry.rs` (VID/PID, `Connector`, vendor, ~44 boards) | `lib/obc-data.ts` `KNOWN_BOARDS` (no VID/PID, no connectors, ~20 boards) | platform classes + `hardware_devices.yaml` |
| **Deployment planner** | `src/deployment/{planner,scheme,inventory}.rs` | `planDeployment` / `generateConfigToml` | — |
| **Firmware generation** | `firmware/obc-esp32-s3/` | `lib/firmware-generator.ts` | `firmware/`, `platforms/`, `rtos/` codegen |

**Drift is already happening.** The TS registry lacks VID/PID, the new `Connector`/`vendor`/`ecosystem` fields, and the 8 Accelerapp-seeded boards I just added; its `transport` union even includes `"mqtt"` (not in the Rust enum), and some capability lists differ (e.g., Waveshare). Every future scout run that updates `registry.rs` widens the gap. The integration's central job is to **make the Rust project the single source of truth and have the others consume it**, not re-type it.

## 3. The unified architecture

Treat the three projects as **layers of one product**, with Oh-Ben-Claw as the canonical core:

```
            ┌─────────────────────────────────────────────────────────┐
   UX layer │  OBC-deployment-generator (Expo: iOS / Android / web)    │
            │  wizard · board catalog · scheme history · fleet console │
            └───────▲───────────────▲───────────────────▲─────────────┘
        consumes    │ registry.json │ planner (WASM)     │ gateway REST
            ┌───────┴───────────────┴───────────────────┴─────────────┐
   Core     │  Oh-Ben-Claw (Rust) — SINGLE SOURCE OF TRUTH            │
  (canon)   │  registry.rs · src/deployment · firmware · src/gateway   │
            └───────▲───────────────────────────────────▲─────────────┘
       templates    │                                    │ extended codegen
            ┌───────┴────────────────────────────────────┴────────────┐
  Build-time│  Accelerapp (Python) — multi-platform firmware/SDK gen   │
            └──────────────────────────────────────────────────────────┘
```

- **Oh-Ben-Claw** owns the data and the logic.
- **The generator** is the UX layer that *consumes* OBC's data/logic (offline) and *talks to* a running OBC over the gateway (online).
- **Accelerapp** supplies extended, multi-platform firmware/SDK codegen the generator can invoke for boards beyond ESP32-S3.

## 4. Integration mechanisms (concrete)

### 4.1 Registry as generated JSON (single source of truth) — *do this first*
Add a tiny Rust exporter (a `cargo test`/`xtask`/`--emit-registry` flag) that serializes `KNOWN_BOARDS` + `KNOWN_ACCESSORIES` (now serde-friendly) to **`registry.json`** with a versioned schema. Commit it (or publish as an artifact).

- The generator **replaces the hand-written `KNOWN_BOARDS` in `obc-data.ts`** with an import of `registry.json` (bundled at build time; refreshable at runtime from the gateway). It instantly gains VID/PID, `Connector`, `vendor`/`ecosystem`, and all 44 boards.
- Accelerapp consumes the same `registry.json` to align its platform list.
- The **weekly hardware scout** updates `registry.rs` → regenerate `registry.json` → every consumer updates with zero re-typing.

> This single change kills the worst drift and is low-effort. Derive `Serialize`/`Deserialize` on `BoardInfo`/`AccessoryInfo`/`Connector` (already `Copy`/`Eq`) to make it trivial.

### 4.2 Deployment planner as the shared engine
The TS `planDeployment`/`generateConfigToml` duplicates `src/deployment`. Two paths, in order of preference:

- **(Preferred) Compile `src/deployment` to WebAssembly** (`wasm-bindgen`, a `wasm32` crate wrapping the planner) and publish a small npm package. The Expo app calls it for offline planning → **guaranteed TOML parity with the runtime**, no duplicated logic. Expo supports WASM on web today; for native, run it in the optional backend or via a wasm runtime.
- **(Bridge) Golden-test parity** until WASM lands: a shared fixture set (`inventory.json → expected.toml`) run in both the Rust test suite and the TS test suite (Vitest), so any divergence fails CI. Keeps the TS planner but pins it to the Rust output.

Either way, **align the schemas now**: drop `"mqtt"` from the TS `transport` union (Rust uses `serial|native|probe|bridge`), and make the generator's `generateConfigToml` emit exactly OBC's `[deployment]` / `[[deployment.hardware]]` / orchestrator config (see `examples/config-nanopi-deployment.toml`) so its output is paste-ready into the real runtime.

### 4.3 Firmware generation: one template set
`firmware-generator.ts` will rot as v2.0 firmware grows (Track 0 `SafetyGate`, Phase 18 reflex engine, Phase 19 streaming). Options:

- **Near-term:** align `FirmwareConfig` and the generated `main.rs`/`config.rs` with the real `firmware/obc-esp32-s3` and the v2.0 firmware design, and add a parity check (generated project must compile against the same `esp-idf-svc` versions).
- **Long-term:** extract the canonical firmware as a **template package** consumed by both the generator and Accelerapp (the "codegen pipeline" play from `ACCELERAPP-CROSS-POLLINATION.md` §3) — and route non-ESP32-S3 / multi-platform targets to **Accelerapp's** richer `platforms/` + `rtos/` generators.

### 4.4 Live-ops bridge: from configurator to fleet console
OBC already exposes a **gateway REST API** (`src/gateway/mod.rs`): `GET /api/v1/status`, `/metrics`, `/nodes`, `/tools`, `POST /chat`, `/tools/{name}`, `GET|POST|PATCH|DELETE /scheduler/tasks`, `/tunnel`. The generator's empty tRPC backend (`server/routers.ts`) is the natural bridge.

Wire the generator's backend to proxy the OBC gateway, turning the mobile/web app into a **remote fleet console** that complements the Tauri desktop GUI:

- **Plan → push:** after generating a scheme, POST the config to a running OBC (or hand off via the gateway) instead of only copying TOML.
- **Monitor:** `/api/v1/nodes` → live node cards (online/quarantined, capabilities, heartbeat); `/api/v1/metrics` → fleet telemetry (reuse the no-op-fallback pattern for offline).
- **Operate:** approve pending physical actions (Track 0) from your phone; run a tool; manage scheduled tasks.
- **Catalog refresh:** fetch `registry.json` from the gateway so the app's board catalog auto-updates from the live runtime.

This makes the generator the front door for the **whole lifecycle**: plan → flash → deploy → observe → operate.

### 4.5 Security (gateway control is a Track 0 concern)
Remote fleet control means the gateway becomes an attack surface for *physical* actions. Reuse OBC's existing pieces and the v2.0 plan: gateway auth (the generator already has OAuth + session cookies), scoped approvals over the API, the Track 0 signed-action audit for anything pushed remotely, and a "read-only by default; explicit elevation to operate" mode in the app. Don't let the phone bypass the physical-action safety layer.

## 5. Phased integration plan

| Phase | Deliverable | Where |
|---|---|---|
| **I1 — Registry SSOT** | `Serialize` on registry types + `registry.json` exporter; generator imports it; drop hand-written `KNOWN_BOARDS`; align `transport` enum | OBC `registry.rs` + generator `obc-data.ts` |
| **I2 — Planner parity** | Shared `inventory→toml` golden fixtures run in both test suites; align TOML to real `[deployment]` schema | OBC `tests/` + generator `tests/` |
| **I3 — Live bridge (read)** | Generator backend proxies gateway `/nodes` + `/metrics` + `/status`; app shows live fleet (read-only) | generator `server/` + OBC `src/gateway` |
| **I4 — Live bridge (operate)** | Push scheme/config; approve Track 0 actions; run tools; manage scheduler — all auth-gated | generator + gateway |
| **I5 — Planner as WASM** | Compile `src/deployment` to wasm npm pkg; generator drops the TS planner | OBC `wasm` crate + generator |
| **I6 — Unified codegen** | Shared firmware template set; route multi-platform targets to Accelerapp codegen | OBC firmware + Accelerapp |

Sequencing rationale: I1 is highest-value/lowest-effort (kills drift now) and unblocks everything; I2 locks correctness; I3–I4 deliver the visible product upgrade (a phone fleet console); I5–I6 retire the remaining duplication.

## 6. Risks & decisions

- **Preserve offline-first.** The generator's appeal is on-device, no-network planning. Registry JSON bundles fine; the planner-as-WASM keeps offline parity; the gateway bridge must be strictly additive (online features degrade gracefully when no runtime is reachable).
- **WASM vs golden-test.** WASM is the clean end state but has Expo-native packaging cost; ship golden-test parity first so correctness isn't blocked on the WASM toolchain.
- **Schema versioning.** `registry.json` and the scheme/TOML format need a `schema_version` so an older app and a newer runtime fail loudly, not silently.
- **Don't fork the registry again.** Once `registry.json` exists, the TS and Python copies must be *generated*, never hand-edited — enforce with a CI check that fails if `obc-data.ts` contains a hand-written board list.

## 7. Bottom line

The deployment generator is already the right **UX layer** for the ecosystem — it just reimplements logic that should be shared. The integration is mostly *consolidation*: make Oh-Ben-Claw the single source of truth (registry JSON, planner WASM, firmware templates, gateway API), have the generator consume it offline and drive it online, and let Accelerapp supply extended codegen. The payoff is a single pipeline — **plan on your phone → generate firmware → push to the fleet → monitor and operate it** — with no three-way drift, and the embodied story finally has a front door on every platform.
