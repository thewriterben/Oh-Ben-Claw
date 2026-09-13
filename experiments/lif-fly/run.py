"""LIF fly sandbox — drive the whole-brain fly model and read its descending output.

What this is for (docs/CONNECTOME-2026-09.md §2.4): before designing the
spinal tier's *descending modulation* message, look at what a population-
coded descending signal actually is in the one brain we can run end to end.
The model is Shiu et al. 2024 (Nature, https://doi.org/10.1038/s41586-024-07763-9)
exactly as published — `upstream_model.py` is their `model.py`, unmodified —
on the FlyWire v783 public release. Cell types come from Schlegel et al. 2024
(https://github.com/flyconnectome/flywire_annotations).

Two commands:

    run.py drive  --group sugar|jon --rate 100 --trials 3   # simulate, save rates
    run.py compare sugar jon                                 # DN population vectors

Determinism: Brian2 is seeded per trial (`--seed` + trial index), the code
generation target is pinned to numpy, and every result file records the
seed, parameters, package versions and data-file hashes that produced it.
Same inputs → identical spike trains.

Not claimed: that the model's absolute rates are right (the authors say so
themselves), or that anything here transfers to OBC without the spinal tier
existing first. This is a look, not a result.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import platform
import sys
import time
from pathlib import Path

import numpy as np
import pandas as pd

HERE = Path(__file__).resolve().parent
DATA = HERE / "data"
RESULTS = HERE / "results"
COMP = DATA / "Completeness_783.csv"
CON = DATA / "Connectivity_783.parquet"
ANN = DATA / "flywire_annotations_783.tsv"

# --- inputs, from the paper's figures.ipynb (FlyWire root ids) -------------
# Labellar sugar-sensing GRNs, right hemisphere (Fig. 1D). 20 of 21 survive
# into v783; the missing one is skipped and reported.
SUGAR_R = [
    720575940624963786, 720575940630233916, 720575940637568838, 720575940638202345,
    720575940617000768, 720575940630797113, 720575940632889389, 720575940621754367,
    720575940621502051, 720575940640649691, 720575940639332736, 720575940616885538,
    720575940639198653, 720575940620900446, 720575940617937543, 720575940632425919,
    720575940633143833, 720575940612670570, 720575940628853239, 720575940629176663,
    720575940611875570,
]
# Johnston's organ neurons, CE + F + D/m subsets (Fig. 5B) — the antennal
# mechanosensory input whose grooming circuit the paper recovered.
JON_ALL = [
    720575940619341105, 720575940630122015, 720575940611061526, 720575940615848788,
    720575940628444667, 720575940627941431, 720575940632449619, 720575940650244342,
    720575940631866508, 720575940638681845, 720575940628978450, 720575940609522461,
    720575940621442224, 720575940602506208, 720575940629022149, 720575940627109991,
    720575940630020111, 720575940615986459, 720575940618684481, 720575940620382889,
    720575940630080071, 720575940626565455, 720575940630319671, 720575940602720940,
    720575940630564179, 720575940637632419, 720575940615809349, 720575940626042149,
    720575940637054835, 720575940602132509, 720575940614188149, 720575940616951124,
    720575940628101126, 720575940629055721, 720575940616589878, 720575940622449388,
    720575940614427195, 720575940625797617, 720575940638664437, 720575940618467195,
    720575940621729757, 720575940613971485, 720575940627585688, 720575940629650997,
    720575940630059847, 720575940608742409, 720575940614351477, 720575940633153375,
    720575940622937528, 720575940604753437, 720575940611783464, 720575940618599872,
    720575940609541917, 720575940637410869, 720575940630070343, 720575940621397417,
    720575940614035485, 720575940610018266, 720575940626307902, 720575940634634606,
    720575940614060829, 720575940624799290, 720575940641921421, 720575940623298559,
    720575940625559358, 720575940629138959, 720575940621625597, 720575940625962568,
    720575940632767383, 720575940624915230,
    720575940606239243, 720575940626956777, 720575940604973746, 720575940622222856,
    720575940642517284, 720575940629719404, 720575940616613022, 720575940604299454,
    720575940615473186, 720575940622217992, 720575940606800341, 720575940629267498,
    720575940637366335, 720575940624224408, 720575940609543197, 720575940633364179,
    720575940629502009, 720575940606431189, 720575940625733960, 720575940638529525,
    720575940617524053, 720575940628935564, 720575940624308355, 720575940631170346,
    720575940627704375, 720575940625885512, 720575940614929245, 720575940647493241,
    720575940618888368, 720575940625087546, 720575940606657493, 720575940617273560,
    720575940640591861, 720575940639410035, 720575940621532413, 720575940627523584,
    720575940621521917, 720575940621097398, 720575940625915338, 720575940606222428,
    720575940627868471, 720575940622179497, 720575940608297774, 720575940614026269,
    720575940613012959, 720575940628100614, 720575940606611401, 720575940628649465,
    720575940610008217, 720575940623791152, 720575940625571240, 720575940634923621,
    720575940609530653, 720575940635968745, 720575940625703434, 720575940613105311,
    720575940629386819, 720575940623077389, 720575940625763015, 720575940628359017,
    720575940630834171, 720575940622892988, 720575940621289537, 720575940641395163,
    720575940616064546, 720575940628978409, 720575940652566177, 720575940627493096,
    720575940619085397, 720575940635545310, 720575940645728803, 720575940629141775,
    720575940626557995, 720575940631098338, 720575940639904475, 720575940635067034,
]
GROUPS = {"sugar": SUGAR_R, "jon": JON_ALL}
# Proboscis motor neuron 9, left (Fig. 1E). The right one's id changed in v783.
MN9_L = 720575940660219265
ACTIVE_HZ = 1.0  # a neuron "responds" above this mean rate


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def annotations() -> pd.DataFrame:
    a = pd.read_csv(ANN, sep="\t", low_memory=False)
    return a.set_index("root_id")[["super_class", "cell_class", "cell_type", "side"]]


def drive(group: str, rate_hz: float, trials: int, t_run_ms: int, seed: int) -> Path:
    # Import here so `compare` needs no Brian2.
    from brian2 import Hz, ms, prefs
    from brian2 import seed as brian_seed

    sys.path.insert(0, str(HERE))
    import upstream_model as m  # noqa: E402  (the paper's model.py, unmodified)

    prefs.codegen.target = "numpy"  # no C++ toolchain on this box; slower, same numbers

    comp = pd.read_csv(COMP, index_col=0)
    fly2i = {fid: i for i, fid in enumerate(comp.index)}
    i2fly = {i: fid for fid, i in fly2i.items()}
    wanted = GROUPS[group]
    exc = [fly2i[f] for f in wanted if f in fly2i]
    missing = [f for f in wanted if f not in fly2i]

    params = dict(m.default_params)
    params["t_run"] = t_run_ms * ms
    params["r_poi"] = rate_hz * Hz
    params["n_run"] = trials

    print(f">>> {group}: {len(exc)} neurons at {rate_hz} Hz, {trials} × {t_run_ms} ms "
          f"(missing in v783: {len(missing)})")
    counts: dict[int, np.ndarray] = {}
    wall = []
    for k in range(trials):
        brian_seed(seed + k)
        t0 = time.time()
        spk = m.run_trial(exc, [], [], str(COMP), str(CON), params)
        wall.append(time.time() - t0)
        for i, ts in spk.items():
            counts.setdefault(i, np.zeros(trials))[k] = len(ts)
        print(f"    trial {k}: {wall[-1]:.1f} s, {len(spk)} neurons spiked")

    t_s = t_run_ms / 1000.0
    rates = pd.DataFrame(
        {
            "flywire_id": [i2fly[i] for i in counts],
            "rate_hz": [c.mean() / t_s for c in counts.values()],
            "rate_std_hz": [c.std() / t_s for c in counts.values()],
        }
    ).set_index("flywire_id")
    rates = rates.join(annotations(), how="left")
    stimulated = set(wanted)
    rates["stimulated"] = rates.index.isin(stimulated)

    RESULTS.mkdir(exist_ok=True)
    tag = f"{group}_{int(rate_hz)}Hz"
    rates.sort_values("rate_hz", ascending=False).to_csv(RESULTS / f"{tag}_rates.csv")

    dn = rates[(rates["super_class"] == "descending") & (rates["rate_hz"] >= ACTIVE_HZ)]
    summary = {
        "group": group,
        "rate_hz": rate_hz,
        "trials": trials,
        "t_run_ms": t_run_ms,
        "seed": seed,
        "stimulated": len(exc),
        "missing_in_v783": missing,
        "neurons_responding": int((rates["rate_hz"] >= ACTIVE_HZ).sum()),
        "mn9_left_hz": float(rates["rate_hz"].get(MN9_L, 0.0)),
        "descending_active": int(len(dn)),
        "descending_total_in_annotations": 1303,
        "descending_top_types": dn.groupby("cell_type")["rate_hz"].sum().sort_values(ascending=False).head(15).round(1).to_dict(),
        "wall_s_per_trial": [round(w, 1) for w in wall],
        "provenance": {
            "model": "Shiu et al. 2024, model.py unmodified; params default except t_run/r_poi/n_run",
            "codegen_target": "numpy",
            "python": platform.python_version(),
            "brian2": __import__("brian2").__version__,
            "numpy": np.__version__,
            "pandas": pd.__version__,
            "data": {p.name: sha256(p) for p in (COMP, CON, ANN)},
        },
    }
    out = RESULTS / f"{tag}_summary.json"
    out.write_text(json.dumps(summary, indent=2))
    print(json.dumps({k: v for k, v in summary.items() if k != "provenance"}, indent=2))
    return out


def dn_vector(tag: str) -> pd.Series:
    r = pd.read_csv(RESULTS / f"{tag}_rates.csv", index_col=0)
    d = r[r["super_class"] == "descending"]
    return d.groupby("cell_type")["rate_hz"].sum()


def compare(tags: list[str]) -> None:
    vecs = {t: dn_vector(t) for t in tags}
    types = sorted(set().union(*(v.index for v in vecs.values())))
    mat = pd.DataFrame({t: v.reindex(types).fillna(0.0) for t, v in vecs.items()})
    print(f"descending cell types active in any run: {len(types)} of ~1303 DNs' types")
    for t in tags:
        v = mat[t]
        print(f"  {t}: {int((v >= ACTIVE_HZ).sum())} types active, "
              f"top: {v.sort_values(ascending=False).head(6).round(1).to_dict()}")
    if len(tags) == 2:
        a, b = mat[tags[0]].values, mat[tags[1]].values
        cos = float(a @ b / (np.linalg.norm(a) * np.linalg.norm(b) + 1e-12))
        sa, sb = set(mat.index[a >= ACTIVE_HZ]), set(mat.index[b >= ACTIVE_HZ])
        jac = len(sa & sb) / max(1, len(sa | sb))
        print(f"  cosine between DN population vectors: {cos:.3f}; "
              f"Jaccard of active DN-type sets: {jac:.3f} ({len(sa & sb)} shared)")
    mat.round(2).to_csv(RESULTS / f"compare_{'_'.join(tags)}.csv")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    d = sub.add_parser("drive")
    d.add_argument("--group", choices=sorted(GROUPS), required=True)
    d.add_argument("--rate", type=float, default=100.0)
    d.add_argument("--trials", type=int, default=3)
    d.add_argument("--t-run-ms", type=int, default=1000)
    d.add_argument("--seed", type=int, default=0xC0FFEE)
    c = sub.add_parser("compare")
    c.add_argument("tags", nargs="+", help="e.g. sugar_100Hz jon_100Hz")
    args = ap.parse_args()
    if args.cmd == "drive":
        drive(args.group, args.rate, args.trials, args.t_run_ms, args.seed)
    else:
        compare(args.tags)


if __name__ == "__main__":
    main()
