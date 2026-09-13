# LIF fly sandbox

Step 2 of `docs/CONNECTOME-2026-09.md` §4: run the whole-brain fruit-fly model
end to end and *look at* its descending output before designing the spinal
tier's descending-modulation message (§2.2). An experiment, not a feature —
nothing in `crates/` depends on this directory.

## What runs

Shiu et al. 2024, "A Drosophila computational brain model reveals sensorimotor
processing" (Nature, https://doi.org/10.1038/s41586-024-07763-9): every FlyWire
neuron as a leaky integrate-and-fire unit, synapse weight = synapse count × a
sign from the predicted neurotransmitter × one free constant (0.275 mV). Their
`model.py` is vendored unmodified as `upstream_model.py` (MIT). Data is the
FlyWire v783 public release as shipped in their repo, plus the Schlegel et al.
2024 cell-type table so neurons can be named.

## Setup (done 2026-09-12; repeat after a clone)

```powershell
python -m venv .venv
.\.venv\Scripts\python.exe -m pip install brian2 pandas pyarrow joblib
# data/ (gitignored) — from github.com/philshiu/Drosophila_brain_model (MIT):
#   Connectivity_783.parquet  100.8 MB   Completeness_783.csv  3.3 MB
#   upstream_sugarR_100Hz_v630.parquet  (their example output, for the check)
# from github.com/flyconnectome/flywire_annotations (CC-BY):
#   flywire_annotations_783.tsv  31.7 MB
```

Brian2 runs on its numpy target here (no C++ toolchain on the box): ~54 s per
simulated second, 138,639 neurons, 15.1 M synapses. The paper's C++ build does
it in ~4 s. Same numbers, slower.

## Commands

```powershell
.\.venv\Scripts\python.exe run.py drive --group sugar --rate 100 --trials 3
.\.venv\Scripts\python.exe run.py drive --group jon   --rate 100 --trials 3
.\.venv\Scripts\python.exe run.py compare sugar_100Hz jon_100Hz
```

`drive` Poisson-stimulates a sensory population (the paper's own neuron lists:
21 right-labellar sugar GRNs, or 146 Johnston's-organ neurons) and writes
`results/<tag>_rates.csv` (every neuron that spiked, with its cell type) and
`results/<tag>_summary.json` (MN9 rate, how many of the 1,303 descending
neurons responded, top DN types, and provenance: seed, params, package
versions, data-file hashes). `compare` puts two runs' descending population
vectors side by side.

Deterministic: Brian2 seeded per trial from `--seed`, numpy target pinned.

## Results

See `RESULTS.md` — written from `results/*_summary.json`, not by hand.
