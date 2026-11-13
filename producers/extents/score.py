#!/usr/bin/env python3
"""A theme's score (docs/inputs.md §6.11), as its score.toml declares it: each metric's 0-1 value,
the primary metric's deciding and the weighted mean of the others the piece states breaking
near-ties, its guarantee checked. The producers don't score pieces (the build
step is to, from the same file); this prints what a score.toml does, for tuning its weights.

usage: score.py <score.toml> --examples
       score.py <score.toml> --json <pieces.json>
       score.py <score.toml> <metric>=<value> ... [-- <metric>=<value> ...]...

--json prints, as JSON, the examples' and the given pieces' (a list of their metrics) scores and
metric values in full: the map server's scoring (crate::inputs::extents::Score::of) is tested
against it (crates/inputs/src/inputs/extents/score_expected.json, made by
`score.py producers/extents/terrain/score.toml --json crates/inputs/src/inputs/extents/score_pieces.json`).
"""
import json
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


def check(decl):
    """The guarantee a score.toml states holds for its numbers: a piece finer by more than `within`
    scores higher whatever the other metrics say. (1 - share) × the primary's least step, ln(within)
    over its scale's span, must outweigh `share` × the rest's whole range."""
    m = decl["metric"][decl["primary"]]
    k = math.log(float(decl["within"])) / abs(math.log(float(m["worst"]) / float(m["best"])))
    if not 0 <= decl["share"] < k / (1 + k):
        sys.exit(f"share {decl['share']} breaks the guarantee: it must be under {k / (1 + k):.4f} for within = {decl['within']}")
    for name, other in decl["metric"].items():
        if name != decl["primary"] and other["weight"] < 0:
            sys.exit(f"{name}: a negative weight")


def score(decl, piece):
    """(score or None, {metric: value or None}) of a piece's metrics."""
    vals = {k: value(m, piece.get(k)) for k, m in decl["metric"].items()}
    lead = vals[decl["primary"]]
    if lead is None:
        return None, vals
    rest = {k: v for k, v in vals.items() if k != decl["primary"] and v is not None}
    w = sum(decl["metric"][k]["weight"] for k in rest)
    r = sum(decl["metric"][k]["weight"] * v for k, v in rest.items()) / w if w else lead
    return (1 - decl["share"]) * lead + decl["share"] * r, vals


EXAMPLES = [
    ("a 2 m surface model against a 5 m bare-earth one, both of 2020, accuracy not stated",
     [{"spacing_m": 2, "surface": "surface", "year": 2020}, {"spacing_m": 5, "surface": "ground", "year": 2020}]),
    ("a 1.2 m surface model against a 1.25 m bare-earth one: within the factor, the kind decides",
     [{"spacing_m": 1.2, "surface": "surface", "year": 2020}, {"spacing_m": 1.25, "surface": "ground", "year": 2020}]),
    ("a 3 ft (0.9144 m) surface model of 2010 against a 1 m bare-earth one of 2024",
     [{"spacing_m": 0.9144, "surface": "surface", "year": 2010}, {"spacing_m": 1, "surface": "ground", "year": 2024}]),
    ("old fine against new coarse: 1 m bare earth of 2005 against 2 m bare earth of 2024",
     [{"spacing_m": 1, "surface": "ground", "year": 2005}, {"spacing_m": 2, "surface": "ground", "year": 2024}]),
    ("the same with accuracies stated: 1 m of 2005 at 0.30 m RMSE against 2 m of 2024 at 0.10 m",
     [{"spacing_m": 1, "surface": "ground", "year": 2005, "vertical_rmse_m": 0.30},
      {"spacing_m": 2, "surface": "ground", "year": 2024, "vertical_rmse_m": 0.10}]),
    ("1 m bare earth of 2005 against 5 m bare earth of 2024",
     [{"spacing_m": 1, "surface": "ground", "year": 2005}, {"spacing_m": 5, "surface": "ground", "year": 2024}]),
    ("a 30 m surface model of 2013 at 2 m RMSE against a 10 m bare-earth one, year and accuracy not stated",
     [{"spacing_m": 30, "surface": "surface", "year": 2013, "vertical_rmse_m": 2}, {"spacing_m": 10, "surface": "ground"}]),
    ("what an unknown metric does: the same 1 m bare earth of 2024, stating a 0.5 m RMSE, and stating none",
     [{"spacing_m": 1, "surface": "ground", "year": 2024, "vertical_rmse_m": 0.5}, {"spacing_m": 1, "surface": "ground", "year": 2024}]),
]


def said(decl, piece):
    s, vals = score(decl, piece)
    parts = ", ".join(f"{k} {piece[k]} → {vals[k]:.3f}" if vals[k] is not None else f"{k} not stated" for k in decl["metric"])
    return f"  {s:.3f}  {parts}" if s is not None else f"  none   {parts}"


def expected(decl, pieces_path):
    """The JSON --json prints: each piece's metrics, score and values, the examples' then the
    file's."""
    with open(pieces_path) as f:
        given = json.load(f)
    pieces = [p for _, ps in EXAMPLES for p in ps] + given
    out = []
    for p in pieces:
        s, vals = score(decl, p)
        out.append({"metrics": p, "score": s, "values": vals})
    return json.dumps(out, indent=1, sort_keys=True) + "\n"


def main(argv):
    if len(argv) < 3:
        sys.exit(__doc__)
    with open(argv[1], "rb") as f:
        decl = tomllib.load(f)
    check(decl)
    if argv[2] == "--json":
        sys.stdout.write(expected(decl, argv[3]))
        return
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
