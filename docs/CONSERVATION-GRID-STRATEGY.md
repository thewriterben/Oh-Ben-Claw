# Conservation Grid — Ecosystem Gap Analysis & Phased Roadmap

**Scope:** the OBC ecosystem as a next-generation wildlife-conservation data-acquisition
platform: multi-camera mesh networks deployed on a grid overlaid on physical topography,
with weather/climate metering, positioning, mapping, satellite connectivity, orbital
imagery, drone-fleet integration, and federated learning.

**Repos assessed:** `Oh-Ben-Claw` (OBC — Rust embodied-agent stack + ESP32/Heltec
firmware), `ClawCam` (Python FastAPI camera-trap gateway), `Accelerapp` (Python IoT
code-generation platform), `OBC-deployment-generator` (TypeScript/Expo config wizard).

**Date:** 2026-07-07. This document is grounded in a direct source survey; every "Have"
cites the module that implements it, and every "Gap" was confirmed absent from code *and*
forward plan.

---

## 1. Executive summary

The ecosystem already contains a genuinely strong **node/robot mesh and coordination
layer** and a **mature conservation data pipeline**. What it lacks is the thing that ties
the whole vision together: a **geospatial coordinate backbone** — real latitude/longitude
on every device and event, a site model with terrain, and a grid/coverage engine that
reasons over that geography.

Today all "spatial" reasoning is one of two kinds, neither of which is geographic:

- **Image-space** — ClawCam detection zones are normalized `[0,1]` polygons over the
  camera *frame* (`ClawCam/gateway/clawcam_gateway/zones/geometry.py`).
- **Abstract 2D metric-frame** — OBC navigation uses occupancy grids and SE2 poses in a
  local flat plane (`Oh-Ben-Claw/src/navigation/`), with no elevation or earth frame.

Latitude/longitude exists only as a **dormant schema contract**: ClawCam's event schema
carries `location{latitude, longitude, altitude_m}`
(`ClawCam/gateway/clawcam_gateway/schemas/clawcam-event.schema.json`) and the simulator
emits coordinates, but no GPS driver produces them and no table column stores them — they
live and die inside an opaque `payload_json` blob.

**The consequence:** roughly seven of the requested capabilities (grid placement,
coverage optimization, topography, positioning, satellite-imagery overlay, geofenced
weather, geo-tagged conservation records) all depend on that missing backbone. Build it
first and most of the list becomes incremental feature work slotted onto a shared frame.
Skip it and every feature reinvents its own geometry and can never be joined.

**One-line verdict per pillar:**

| Pillar | Status | Home repo |
|---|---|---|
| Multi-node mesh networking | **Have** | Oh-Ben-Claw |
| Fleet coordination (auction, conflict avoidance) | **Have** | Oh-Ben-Claw |
| Multi-camera support | **Have** (off-mesh) | ClawCam |
| Grid deployment optimization (spatial) | **Gap** | — |
| Positioning / geolocation | **Partial** (inert) | OBC + ClawCam |
| Mapping (SLAM / occupancy / costmap) | **Have** (2D only) | Oh-Ben-Claw |
| Topography / terrain overlay | **Gap** | — |
| Weather / climate metering | **Partial** | OBC + ClawCam |
| Satellite connectivity | **Gap** | — |
| Orbital / satellite imagery | **Gap** | — |
| Drone / UAV fleet | **Gap** (adjacent) | Oh-Ben-Claw |
| Federated learning | **Partial** (codegen + skill-sharing) | Accelerapp + OBC |
| Conservation data acquisition | **Have** (strongest) | ClawCam |

*(The table above is the **starting-point** assessment. Delivery progress since is tracked
below.)*

---

## 1b. Delivered since the assessment (2026-07-07)

The geospatial backbone and the first frontier phases have shipped. Phase status against
the G0–G9 plan in §5:

| Phase | Status | What landed |
|---|---|---|
| **G0** — Geospatial foundation | ✅ **Delivered** | OBC `src/geo` (`GeoPoint`, `GeoFrame` ENU⇄lat/lon, `Site`, point-in-polygon; unit-tested). ClawCam: geo columns on `events` + device positions + environment columns (populated, backfilled, indexed); `sites` table + `events_in_site`/`devices_in_site` point-in-polygon; site + device geo surface over REST **and** MCP (`list_sites`, `get_site_events`, `list_device_positions`, `get_site_devices`). |
| **G1** — Grid coverage optimizer | ✅ **Delivered** | OBC `src/siteplan` — greedy max-coverage placement (min-spacing + mesh-connectivity), ENU-frame, emits ENU+geodetic positions, coverage fraction, `to_toml` deployment block. Agent-callable via the `plan_site` tool. |
| **G2** — Camera-onto-mesh bridge | 🟡 **Software bridge done** | ClawCam `mesh.field_summary` — compact, size-bounded field-summary codec (`CC|…`) that fits a LoRa frame. OBC `lora_gateway::parse_clawcam_summary` + ingest — a `CC|…` summary heard on the spine lands as `clawcam.<device>.field` + rollup facts in world memory. *Remaining: the physical LoRa emit from the ClawCam node (firmware) and the store-and-forward buffer wiring.* |
| **G3** — Positioning (real GNSS) | 🔲 Planned | — |
| **G4** — Weather as analytics | ✅ **Delivered** | ClawCam **environment report** (temp/humidity/pressure stats + trend + daily series) and **weather–activity correlation** (exposure-normalized rate vs conditions + Pearson r), both wired through the tool-catalog SSOT + REST. |
| **G5** — Terrain / line-of-sight | ✅ **Delivered** | OBC `siteplan::plan_site_on` + `Heightfield` — coverage respects terrain occlusion (node mast height, ray-vs-ground line-of-sight); flat/`None` reproduces the original exactly. |
| **G6** — Satellite connectivity | 🔲 Planned | — |
| **G7** — Orbital imagery | 🔲 Planned | — |
| **G8** — Drone / aerial tier | 🟡 **Adapter done** | OBC `src/aerial` — maps a drone's geodetic telemetry into a fleet `NodeState` via `geo::GeoFrame`, so a UAV joins the existing auction/exploration as a body-agnostic node; plus `flight_safe` (battery + `Site` geofence) as the aerial Track-0 gate. *Remaining: a real MAVLink/PX4 link feeding `AerialTelemetry`, and a `report_aerial` fleet path.* |
| **G9** — Federated learning | 🟡 **Aggregation + round loop done** | ClawCam `federated` — sample/trust-weighted FedAvg (`fedavg`) **and** the round loop (`round`): each node's review labels → local calibration-threshold update → aggregate → **versioned global model** (`federated_round_from_reviews`). Only weights + counts move, never imagery; a drifting node is trust-down-weighted. *Remaining: distributing the global model back to nodes via the OBC model registry, and Accelerapp's on-device FL codegen for the node trainer.* |

Net effect: the **geospatial coordinate backbone** that §3 identifies as the unlock now
exists on both sides of the wire, the named **grid-optimization** capability is real and
terrain-aware, and **weather** is first-class analytics with a correlation. The remaining
work (G2–G3, G6–G9) is the set of integrations that this foundation makes tractable.

---

## 2. Current-state assessment (with evidence)

### 2.1 Have — the load-bearing foundations already built

**Multi-node mesh networking (Oh-Ben-Claw).** The strongest subsystem. LoRa-mesh spine
transport with a compact frame codec and pluggable radio (`src/spine/lora_mesh.rs`); a
host-side gateway bridge that ingests node link/power/reflex reports into world memory
(`src/spine/lora_gateway.rs`); and a mesh supervisor that derives per-node
Online/Degraded/Offline health each tick and issues rate-limited autonomous recovery,
escalating "presumed-lost" (`src/spine/mesh_supervisor.rs`). Agent-facing `mesh_command`
and `mesh_status` tools (`src/tools/builtin/mesh.rs`), real SX1262 firmware
(`firmware/heltec-lora-linktest/`), and end-to-end tests (`tests/mesh_spine_e2e.rs`,
`tests/mesh_fleet_e2e.rs`).

**Fleet coordination (Oh-Ben-Claw).** `src/fleet/mod.rs` — a registry ingesting node
heartbeats (pose/battery/mode), plus two allocators: nearest-online-idle and a
market-based **sequential single-item auction** (`auction_allocate`) that is
order-independent. Spatial **conflict avoidance** enforces a minimum target separation via
a claims map. Assignments broadcast over the LoRa mesh for off-grid operation and are
advisory — each node keeps its own Track-0 safety gate. Composes with frontier exploration
for coordinated multi-node coverage.

**Mapping (Oh-Ben-Claw).** A full, tested nav/mapping stack in `src/navigation/`:
pose-graph SLAM with loop closure (`slam.rs`), online occupancy mapping via ray-casting
(`mapping.rs`), A* over an occupancy grid (`planning.rs`), Nav2-style costmap inflation
(`costmap.rs`), and frontier exploration (`exploration.rs`). **Caveat:** 2D metric
occupancy only — no terrain, elevation, or earth-referenced frame.

**Conservation data acquisition (ClawCam).** The mature core. Detection pipeline with
MegaDetector abstraction and chain fusion (IoU + NMS, `role` single/chain_member/fused);
non-destructive human review (`review_state`, reviewer, note) with a priority triage
scorer; **twelve pure analytics reports** exposed over MCP + REST (activity, trends,
diversity, abundance/RAI, encounters, comparison + timing-shift, calibration, anomaly,
co-occurrence, site, fused detections, species profile — see `docs/ANALYTICS.md`); CSV
export; a cron/one-shot scheduler; and an alerts engine with severity, de-dup, and digest.
Multi-tenant by a first-class `deployment_id` column carried on every table.

### 2.2 Partial — exists but not wired to the vision

**Positioning / geolocation.** OBC has pose fusion (`src/navigation/pose_fusion.rs`) and a
particle filter with a position-measurement hook (`src/navigation/particle.rs`), and `gps`
is a first-class capability token in the peripheral registry. ClawCam's event/health
schemas carry lat/lon/alt and BirdNET accepts lat/lon hints. **But:** there is no GPS/GNSS
driver, no NMEA parsing, no lat/lon columns in ClawCam's DB (they stay in `payload_json`),
and OBC's "gps" is only ever a weighted *metric-frame* pose source or a planning label —
never a real earth coordinate. Positioning is a contract and a placeholder, not a signal.

**Weather / climate metering.** OBC has a sensing suite ingesting named scalar streams
into bitemporal world memory with quality/anomaly classification (`src/sensing/mod.rs`)
and a real DHT22 temp/humidity driver (`firmware/obc-esp32-s3/src/dht.rs`). ClawCam's
health schema carries `environment{temperature_c, humidity_percent, pressure_hpa}` and the
simulator emits it. **But:** barometric sensors (BME280/BMP388) are *registered but
undriven*; ClawCam persists environment only as a blob (no columns, no time-series, no
weather analytics); and there is no external weather/forecast-API ingestion anywhere.

**Federated learning.** Accelerapp *generates* federated-learning firmware code with a
configurable aggregation method (`federated_averaging`) and privacy level
(`src/accelerapp/agents/tinyml_agent.py`). OBC has on-device association-rule mining that
proposes human-gated reflex rules (`src/learning/mod.rs`), skill synthesis from successful
episodes (`src/skill_forge/synthesis.rs`), and a centralized "ClawHub" skill registry for
sharing (`src/skill_forge/registry.rs`). **But:** no live federated loop — no weight/gradient
aggregation across nodes, no running FL server/rounds. The pieces are codegen + a skill
marketplace, not decentralized training.

### 2.3 Gap — absent from code and forward plan

**Grid deployment optimization (spatial).** Both "deployment" tools plan *logical*
topology, not geography. OBC `src/deployment/` maps a hardware inventory to agent roles and
emits TOML + per-board firmware scaffolds; the Expo `OBC-deployment-generator`
(`lib/obc-data.ts` `planDeployment()`) is a capability-matching wizard that emits agent
topology, TOML, and a complete ESP32-S3 Rust firmware project (`lib/firmware-generator.ts`).
Neither places nodes on a coordinate grid, computes coverage, or reasons about an area.
"Grid" in the code means the nav occupancy grid or CSS layout.

**Topography / terrain.** No DEM, elevation, slope, heightmap, or contour anywhere. All
spatial reasoning is flat-plane.

**Satellite connectivity (Iridium/Swarm/Starlink).** Zero code, not on any roadmap. The
off-grid story is LoRa mesh + cellular only.

**Orbital / satellite imagery (Sentinel/Landsat).** Zero remote-sensing integration.

**Drone / UAV.** No flight-controller/MAVLink code. **Adjacent, though:** the fleet layer
is body-agnostic (`NodeState` is pose/battery/mode), so aerial nodes could join the
existing auction/exploration with a flight adapter rather than a rewrite.

**Camera-on-mesh.** ClawCam cameras connect to the brain via **MQTT + MCP stdio**
(`brain/oh-ben-claw-adapter/`), not the LoRa mesh. The mesh maturity lives entirely in
Oh-Ben-Claw; ClawCam's own cloud path is one-way media archival (`sync/cloud_store.py`).

---

## 3. The core gap: a geospatial coordinate backbone

Seven requested capabilities are blocked on one absent primitive. A minimal backbone has
three parts:

1. **An earth coordinate layer.** Real `latitude`/`longitude`/`altitude` promoted to
   queryable columns on devices and events (ClawCam) and to a geodetic frame in world
   memory (OBC), with a documented local-tangent-plane (ENU) conversion so the existing 2D
   nav/occupancy math keeps working unchanged around a site origin.
2. **A site model.** A first-class `site` (survey area) entity: boundary polygon, origin,
   terrain reference, and the set of node positions within it. ClawCam's `deployments`
   table is the natural anchor — it is described in `DATA_MODEL.md` as carrying
   "location, settings, covariates" but currently has none of those columns.
3. **A terrain surface.** A DEM (digital elevation model) tile for the site, so coverage,
   line-of-sight, and movement cost can be reasoned over slope rather than a flat plane.

With those three in place, positioning becomes "fill the columns," grid optimization
becomes "solve coverage over the terrain surface," satellite imagery becomes "overlay a
raster on the site," weather becomes "attach a geofenced feed to the site," and every
conservation record becomes joinable in space. Without them, each feature invents its own
geometry and none of them compose.

---

## 4. Target architecture — the Conservation Grid

```
                         ┌─────────────────────────────────────────┐
                         │  SITE MODEL (geo backbone)                │
                         │  boundary • origin(lat,lon) • DEM tile    │
                         │  ENU⇄geodetic • node positions            │
                         └───────────────┬───────────────────────────┘
             ┌───────────────────────────┼───────────────────────────┐
             ▼                           ▼                           ▼
   ┌──────────────────┐        ┌──────────────────┐        ┌──────────────────┐
   │ GRID OPTIMIZER   │        │ MESH BACKBONE    │        │ SENSING / WEATHER│
   │ coverage over    │        │ LoRa spine +     │        │ env columns +    │
   │ terrain + LoS    │        │ camera bridge    │        │ geofenced feed   │
   │ (OBC deployment) │        │ (OBC ↔ ClawCam)  │        │ (OBC + ClawCam)  │
   └────────┬─────────┘        └────────┬─────────┘        └────────┬─────────┘
            │                           │                           │
            ▼                           ▼                           ▼
   ┌──────────────────┐        ┌──────────────────┐        ┌──────────────────┐
   │ POSITIONING      │        │ SATELLITE TIER   │        │ AERIAL TIER      │
   │ real GNSS →      │        │ backhaul (Iridium│        │ drone adapter on │
   │ geodetic frame   │        │ /Swarm) + imagery│        │ body-agnostic    │
   │                  │        │ (Sentinel/Landsat│        │ fleet auction    │
   └──────────────────┘        │  raster overlay) │        └──────────────────┘
                               └──────────────────┘
            ┌───────────────────────────────────────────────────────┐
            │ CONSERVATION DATA (ClawCam, already strong) —          │
            │ now geo-tagged: detections/analytics/exports/alerts    │
            └───────────────────────────────────────────────────────┘
            ┌───────────────────────────────────────────────────────┐
            │ FEDERATED LEARNING — per-node models aggregated across │
            │ the grid (Accelerapp codegen + OBC skill/model share)  │
            └───────────────────────────────────────────────────────┘
```

The two existing strengths — the **mesh backbone** and the **conservation pipeline** —
become the load-bearing walls; the geospatial backbone is the foundation they get poured
onto; and satellite, aerial, and federated-learning are the upper floors that only make
sense once the foundation exists.

---

## 5. Phased roadmap

Phases are ordered by dependency. G0–G2 are the unlock and deliver visible value quickly;
G3–G5 harden the grid; G6–G9 are the frontier tiers that were entirely absent. Each phase
names its home repo(s) and its exit criterion.

### Phase G0 — Geospatial foundation *(unlock; do first)*
- **ClawCam:** promote `latitude`/`longitude`/`altitude_m` to real columns on `devices`
  and `events`; backfill from `payload_json`; add geo to CSV export. Add a `sites` table
  (boundary polygon, origin lat/lon, DEM ref) and link `deployment_id → site`.
- **OBC:** add a geodetic frame to world memory with an ENU⇄lat/lon conversion anchored at
  a site origin, so `src/navigation/` keeps operating in local metric coordinates.
- **Exit:** a detection can be queried "within this polygon"; a node has a real position.

### Phase G1 — Grid deployment + coverage optimizer *(the named gap)*
- **OBC `src/deployment/` (extend) or a new `src/siteplan/`:** given a site polygon,
  terrain, and a node budget, generate candidate positions on a grid/lattice and optimize
  for detection coverage + mesh connectivity (greedy max-coverage first, then simulated
  annealing). Reuse `src/fleet/` conflict-avoidance geometry for spacing.
- **OBC-deployment-generator (TS):** add a map step that renders the optimized layout and
  feeds positions into the existing TOML/firmware codegen.
- **Exit:** "here's a 40-hectare reserve and 12 nodes" → an optimized, connectivity-checked
  placement with per-node config.

### Phase G2 — Camera-onto-mesh bridge
- **ClawCam ↔ OBC:** let ClawCam nodes ride the OBC LoRa spine for low-bandwidth field
  summaries (detection counts, health) when MQTT/cellular is unavailable, using the
  existing `src/spine/lora_gateway.rs` ingest path and ClawCam's `cloud_uploads`
  store-and-forward queue as the buffer.
- **Exit:** a camera with no IP backhaul still reports summaries to the gateway over mesh.

### Phase G3 — Positioning (real GNSS)
- **OBC firmware + ClawCam firmware:** a real GPS/GNSS driver (NMEA parse) on GPS-capable
  boards (T-Beam/SIM7600); feed fixes into OBC pose fusion as a true geodetic source and
  into ClawCam's now-real geo columns.
- **Exit:** node positions are measured, not configured; the site model self-populates.

### Phase G4 — Environmental / weather layer (first-class)
- **ClawCam:** promote `environment{temperature_c, humidity_percent, pressure_hpa, lux}`
  to columns + a time-series; add a weather analytics report and detection-vs-weather
  correlation. Drive the registered-but-undriven BME280/BMP388.
- **OBC:** barometric-trend + a geofenced external weather-feed adapter attached to the
  site.
- **Exit:** "does fox activity track temperature/pressure here?" is answerable.

### Phase G5 — Topography / terrain-aware reasoning
- **OBC:** ingest a DEM tile per site; extend `src/navigation/costmap.rs` with slope cost
  and add line-of-sight to the grid optimizer (G1) so coverage and mesh links respect
  terrain occlusion.
- **Exit:** placement and movement reason over elevation, not a flat plane.

### Phase G6 — Satellite connectivity (backhaul)
- **OBC firmware + a new `src/spine/sat_gateway.rs`:** Iridium SBD / Swarm short-message
  backhaul as a spine transport peer to LoRa, for sites with no cellular. Reuse the
  store-and-forward and command-bridge patterns already in `src/spine/`.
- **Exit:** a fully off-grid site delivers daily summaries via satellite.

### Phase G7 — Orbital imagery integration
- **ClawCam + a new geospatial service:** fetch Sentinel-2/Landsat tiles for a site,
  overlay on the map, and expose land-cover/NDVI as a site covariate joinable to
  detections. Overlay-and-correlate, not on-board imagery.
- **Exit:** detections can be analyzed against habitat/land-cover context.

### Phase G8 — Drone / aerial tier
- **OBC:** a flight adapter (MAVLink/PX4) presenting an aerial node as a standard fleet
  `NodeState`, so drones join the existing `auction_allocate` + frontier exploration for
  aerial survey and gap-filling. No new coordination layer needed.
- **Exit:** a drone accepts a survey task from the fleet auction and reports geo-tagged
  captures into the same pipeline.

### Phase G9 — Federated learning loop
- **Accelerapp (codegen) + OBC (`src/skill_forge/`, `src/providers/model_registry.rs`):**
  turn per-node model updates into a real aggregation round across the grid, using
  Accelerapp's `federated_averaging` code path for the on-device side and OBC's model
  registry + ClawHub for distribution, gated by the existing human-approval + verification
  path.
- **Exit:** node detectors improve from local review labels without centralizing raw
  imagery.

---

## 6. Cross-repo ownership

| Layer | Primary repo | Why |
|---|---|---|
| Site model, geo columns, geo-tagged conservation data | **ClawCam** | owns the DB + analytics + export |
| Grid/coverage optimizer, terrain cost, LoS | **Oh-Ben-Claw** | owns deployment planning + nav geometry |
| Mesh backbone + satellite backhaul + camera bridge | **Oh-Ben-Claw** | owns the spine transports |
| Positioning (GNSS driver) | **Oh-Ben-Claw** firmware + **ClawCam** firmware | shared board targets |
| Weather layer | **ClawCam** (analytics) + **Oh-Ben-Claw** (sensing/feed) | split by role |
| Orbital imagery service | **ClawCam** (+ new geospatial module) | joins to detections |
| Drone adapter | **Oh-Ben-Claw** | owns the fleet |
| Federated learning | **Accelerapp** (codegen) + **Oh-Ben-Claw** (registry/gate) | split by role |
| Map UI + per-node config/firmware codegen | **OBC-deployment-generator** | already the config surface |

---

## 7. Sequencing rationale, quick wins, and risks

**Why this order.** G0 is non-negotiable and unglamorous: it is a handful of columns and a
coordinate conversion, but it is the join key for everything downstream. G1 (the capability
you named) is deliberately second because a coverage optimizer is only meaningful once
positions are real coordinates on a site. G2 makes the *existing* two strengths — mesh and
cameras — finally touch, which is high value for low new code. The frontier tiers (G6–G9)
are last not because they're hard-first, but because each is only coherent on top of the
foundation; attempting them early means each reinvents geo.

**Quick wins (days, not weeks).**
- Promote ClawCam geo + environment columns and add them to CSV export (G0/G4 slice) — the
  data is already arriving in `payload_json`; this is unblocking existing dormant signal.
- Render the *existing* OBC deployment output on a map in the Expo generator (G1 slice)
  before the optimizer exists — immediate situational value.
- A geofenced weather-feed adapter attached to a site (G4 slice) needs no new hardware.

**Risks / watch-items.**
- **Don't fork geometry.** Define the ENU⇄geodetic conversion once (OBC) and have ClawCam
  consume it; two coordinate conventions will silently diverge.
- **Keep 2D nav intact.** The geodetic layer must wrap, not replace, the working
  `src/navigation/` metric frame — anchor at a site origin and convert at the edges.
- **Parallel-author coordination.** OBC has active parallel development; the geo frame,
  fleet, and spine are shared surfaces — land the site-model contract first so both sides
  build against a fixed schema.
- **Scope the frontier tiers as integrations, not inventions.** Satellite imagery, weather,
  and (largely) federated learning are third-party-service or codegen integrations; resist
  building bespoke where an overlay or an existing Accelerapp template suffices.

**Bottom line.** The mesh and the data pipeline are real and strong. Pour the geospatial
foundation (G0–G1), bridge the two strengths (G2), and the ambitious remainder becomes a
sequence of well-scoped integrations rather than a moonshot.
