#!/usr/bin/env python3
"""A theme's score (docs/inputs.md §6.11), as its score.toml declares it: each metric's 0-1 value
and the weighted mean over the metrics a piece states. The producers don't score pieces (the build
step is to, from the same file); this prints what a score.toml does, for tuning its weights.

usage: score.py <score.toml> --examples
       score.py <score.toml> <metric>=<value> ... [-- <metric>=<value> ...]...
"""
import math
import sys
import tomllib


def value(m, v):
    """A metric's 0-1 value of `v` (None: not stated, or a word its `values` don't hold)."""
    if v is None:
        return None
    if "values" in m:
        return m["values"].get(v)
    worst, best, v = float(m["worst"]), float(m["best"]), float(v)
    if m["scale"] == "log":
        worst, best, v = math.log(worst), math.log(best), math.log(v)
    return min(1.0, max(0.0, (v - worst) / (best - worst)))


def score(decl, piece):
    """(score or None, {metric: value or None}) of a piece's metrics."""
    vals = {k: value(m, piece.get(k)) for k, m in decl["metric"].items()}
    known = {k: v for k, v in vals.items() if v is not None}
    w = sum(decl["metric"][k]["weight"] for k in known)
    if not w:
        return None, vals
    return sum(decl["metric"][k]["weight"] * v for k, v in known.items()) / w, vals


EXAMPLES = [
    ("a 2 m surface model against a 5 m bare-earth one, both of 2020, accuracy not stated",
     [{"spacing_m": 2, "surface": "surface", "year": 2020}, {"spacing_m": 5, "surface": "ground", "year": 2020}]),
    ("a 0.4 m surface model against the same 5 m bare-earth one",
     [{"spacing_m": 0.4, "surface": "surface", "year": 2020}, {"spacing_m": 5, "surface": "ground", "year": 2020}]),
    ("old fine against new coarse: 1 m bare earth of 2005 against 2 m bare earth of 2024",
     [{"spacing_m": 1, "surface": "ground", "year": 2005}, {"spacing_m": 2, "surface": "ground", "year": 2024}]),
    ("the same with accuracies stated: 1 m of 2005 at 0.30 m RMSE against 2 m of 2024 at 0.10 m",
     [{"spacing_m": 1, "surface": "ground", "year": 2005, "vertical_rmse_m": 0.30},
      {"spacing_m": 2, "surface": "ground", "year": 2024, "vertical_rmse_m": 0.10}]),
    ("1 m bare earth of 2005 against 5 m bare earth of 2024",
     [{"spacing_m": 1, "surface": "ground", "year": 2005}, {"spacing_m": 5, "surface": "ground", "year": 2024}]),
    ("a 30 m surface model of 2013 at 2 m RMSE against a 10 m bare-earth one, year and accuracy not stated",
     [{"spacing_m": 30, "surface": "surface", "year": 2013, "vertical_rmse_m": 2}, {"spacing_m": 10, "surface": "ground"}]),
]


def said(decl, piece):
    s, vals = score(decl, piece)
    parts = ", ".join(f"{k} {piece[k]} → {vals[k]:.3f}" if vals[k] is not None else f"{k} not stated" for k in decl["metric"])
    return f"  {s:.3f}  {parts}" if s is not None else f"  none   {parts}"


def main(argv):
    if len(argv) < 3:
        sys.exit(__doc__)
    with open(argv[1], "rb") as f:
        decl = tomllib.load(f)
    if argv[2] == "--examples":
        for title, pieces in EXAMPLES:
            print(title)
            for p in pieces:
                print(said(decl, p))
        return
    piece = {}
    for a in argv[2:] + ["--"]:
        if a == "--":
            if piece:
                print(said(decl, piece))
            piece = {}
            continue
        k, v = a.split("=", 1)
        piece[k] = v if "values" in decl["metric"][k] else float(v)


if __name__ == "__main__":
    main(sys.argv)
