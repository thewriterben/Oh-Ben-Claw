# Results — 2026-09-12

Source of every number: `results/sugar_100Hz_summary.json`,
`results/jon_100Hz_summary.json`, `results/compare_sugar_100Hz_jon_100Hz.csv`.
3 trials × 1 s each, seed `0xC0FFEE`, Brian2 2.10.1 numpy target, FlyWire v783.

## 1. Does the model reproduce here?

Sugar GRNs (right labellum, 20 neurons) at 100 Hz → **MN9-left 63.3 Hz**.
The paper's own example output for the same stimulus on v630 (30 trials,
`upstream_sugarR_100Hz_v630.parquet`) gives **67.0 Hz**. Different connectome
release, ten times fewer trials, one GRN missing from v783: within 6%. The
proboscis motor neurons fire, the fly "feeds". Good enough to trust the
descending readout that follows.

## 2. What a descending signal looks like

| stimulus | neurons responding | DNs active (of 1,303) | DN types | MN9-L | motor neurons active |
|---|---|---|---|---|---|
| sugar, 100 Hz | 348 | **61** (4.7%) | 36 | 63.3 Hz | 27 |
| JON (antennal mechanosensory), 100 Hz | 371 | **48** (3.7%) | 42 | 0.0 Hz | 2 |

DN rates are graded, not binary: sugar median 17 Hz, 95th percentile 51 Hz,
max 94 Hz; JON median 7 Hz, 95th 46 Hz, max 73 Hz.

The two descending populations are almost disjoint: **cosine 0.038** between
the DN-type rate vectors, **Jaccard 0.04** between the active sets — 3 types
shared out of 87 active across both runs.

Two routes out of the brain, and they differ by behaviour: feeding drives
brain-resident motor neurons *directly* (27 active, MN9 among them) *and* a
DN population; grooming drives almost no brain motor neuron (2) — the DNs are
the whole output, and the ventral nerve cord does the rest.

## 3. What this says about the spinal tier (§2.2 of the connectome doc)

Three properties of the signal the spinal tier's *descending modulation*
message should copy, each measured above rather than assumed:

1. **Sparse.** ~4–5% of descending channels carry a behaviour. A dense
   vector over every channel is the wrong shape; a short list of
   `(channel, level)` pairs is the right one, and its size on LoRa is bounded
   by that sparsity, not by the channel count.
2. **Graded.** Levels span an order of magnitude within one behaviour. The
   message carries a level per channel, not a bit.
3. **Near-orthogonal across behaviours.** Different behaviours recruit
   different channels rather than different levels on shared channels, so a
   channel *is* a behaviour module, not an actuator. The node-side
   converter maps channels to actuation locally; the brain never names an
   actuator.

And one about the interface: the brain keeps a direct line to a few effectors
(feeding) alongside the descending path. The spinal tier should not be the
*only* route — some fast-reflex primitives will still want a direct trigger.

## Not claimed

Absolute rates (the authors say theirs are not accurate either; everything
above is relative). That 3 trials resolve anything below ~5 Hz. That JON and
sugar are representative of the whole descending repertoire — two stimuli is
a look, and vision (the ClawCam analogue) was not run: the paper's model has
no visual input list to borrow, and inventing one is a separate piece of
work. `run.py drive` accepts new groups when one exists.
