# Playbook — ClawCam analytics wake ("today is weird")

**Trigger:** one of the vision-analytics reflexes escalated to System 2 because a
`clawcam.analytics.*` fact crossed a threshold:

- `vision-anomaly-drop` — the latest day's detection z-score fell to `<= -z_alert`
  (`clawcam.analytics.anomaly`). An unusually **quiet** day.
- `vision-anomaly-spike` — the latest day's z-score rose to `>= z_alert`. An unusually
  **busy** day.
- `vision-calibration-drift` — the model is **miscalibrated**
  (`clawcam.analytics.calibration.well_calibrated == "false"`): its confidence no longer
  agrees with human review.

These facts are folded in on a slow cadence (hourly by default) by the ClawCam analytics
poll; the reports themselves are daily aggregates. The escalation reason carries the short
form of the steps below; this document is the full version.

## Context you already have

All the tools here are **read-only** — they observe. Any change to a camera node stays
Track-0 gated; any change to detection/alert thresholds stays operator-gated.

- **`get_anomaly_report`** — per-day detection counts z-scored against the site baseline,
  with each day flagged `spike` / `drop` / `normal`. Confirms *which* day and *how far*
  from normal.
- **`get_site_report`** — the composed picture: activity (when), trends (rising/falling),
  diversity, encounters (real visits), and the alert digest. Use it to see whether a swing
  is site-wide or one subject.
- **`get_node_health`** — per-camera reachability and battery. A silent feed is often a
  camera offline or on low battery, not an empty landscape.
- **`get_calibration_report`** — the confidence-vs-review curve, overall precision, and the
  suggested accept threshold.
- **`get_review_queue`** — unreviewed detections ranked by attention-needed, so ambiguous
  and rare hits lead.
- **`world_memory`** — query/record time-valid facts, incl. `clawcam.analytics.anomaly`,
  `clawcam.analytics.encounters`, `clawcam.analytics.calibration`.

## Procedure

### A. `vision-anomaly-drop` — an unusually quiet day

A drop is the signature of a **knocked-over / obstructed camera or a dead PIR** far more
often than of animals simply staying away.

1. **Confirm.** `get_anomaly_report` — check the flagged day's `z`, `count`, and how it
   compares to the series `mean`. A count at or near zero on a normally-active site is the
   strong signal.
2. **Check the hardware.** `get_node_health` — is a camera `offline`, or on low battery?
   That explains a silent feed directly. Cross-check against `clawcam.node.{id}` facts.
3. **Localise.** `get_site_report` — is the quiet site-wide (points at a systemic cause:
   power, connectivity, weather) or confined to one subject/zone (points at that camera)?
4. **Act.** If a camera is down, record a short note to `world_memory` and alert an
   operator to physically check/clear it. If hardware looks fine and it's genuinely a
   quiet day, note the all-clear and stop — don't re-escalate (the 6 h debounce already
   guards this).

### B. `vision-anomaly-spike` — an unusually busy day

1. **Confirm.** `get_anomaly_report` — the flagged day and its magnitude.
2. **Characterise.** `get_site_report` — *which* species drove the surge, and is it
   `rising` in the trend (a developing pattern) or a one-day blip? Encounters vs raw frames
   tells you whether it's many visits or one lingering subject.
3. **Act.** A genuine, security-relevant surge (e.g. people/vehicles where there should be
   none) may warrant an operator alert. A wildlife surge is usually just worth noting to
   `world_memory` for the trend record.

### C. `vision-calibration-drift` — the model disagrees with human review

The model's confidence no longer predicts correctness: alerts keyed on a fixed confidence
threshold will mislead until retuned.

1. **Read the curve.** `get_calibration_report` — note `overall_precision` vs
   `target_precision`, `well_calibrated`, and the `suggested_threshold` (the lowest
   confidence at which accepting everything above it still meets the target precision).
2. **Retune.** Move detection/alert confidence thresholds toward the `suggested_threshold`
   so auto-accepted detections meet the precision bar again. This is a configuration
   change — **operator-gated**, not something the reflex does on its own.
3. **Clear the backlog.** `get_review_queue` — the ambiguous, borderline, and rare hits
   driving the disagreement are exactly what a human should confirm; clearing them both
   improves the next calibration and sharpens the threshold recommendation.
4. **Record.** Note the retune (old → new threshold, the precision it targets) to
   `world_memory` so the next calibration wake can tell whether it helped.

## Notes

- These wakes are intentionally **debounced on a daily/hours scale** (default 6 h) — the
  underlying facts change once per analytics poll, so rapid re-fires would be noise.
- Absence of data is not a calm day: the ingest records *no* fact when a report has no
  dated detections / nothing reviewed, so these reflexes never fire on an empty site.
