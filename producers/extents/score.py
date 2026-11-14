#!/usr/bin/env python3
"""A theme's score (docs/inputs.md §6.11), as its score.toml declares it: each metric's 0-1 value,
the primary metric's deciding and the weighted mean of the others (one the piece doesn't state at
`unstated`) breaking near-ties, its guarantee checked, and how pieces rank (`best`: of those whose primary ties the
finest's, the best availability, then the best score, then the best-founded extent). The producers don't score pieces (the map
server does, from the same file); this prints what a score.toml does, for tuning its weights.

usage: score.py <score.toml> --examples
       score.py <score.toml> --json <pieces.json>
       score.py <score.toml> <metric>=<value> ... [availability=<id>] [extent=<class>] [-- <metric>=<value> ...]...

--json prints, as JSON, the examples' and the given pieces' (`pieces`, each its metrics) scores
and metric values in full, and the order of the given groups of pieces (`groups`, each piece its
metrics, `availability` and `extent`): the map server's scoring and ranking
(crate::inputs::extents::Score::of, Score::ranked) is tested against it
(crates/inputs/src/inputs/extents/score_expected.json, made by
`score.py producers/extents/terrain/score.toml --json crates/inputs/src/inputs/extents/score_pieces.json`).
Pieces given on the command line are scored, then listed best first.
"""
import json
import math
import sys
import tomllib

# What a weighted metric a piece doesn't state counts at where score.toml gives no `unstated`
# (crate::inputs::extents::UNSTATED).
UNSTATED = 0.5


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
    over its scale's span, must outweigh `share` × the rest's whole range, 0 to 1 (each value, and
    `unstated`, in it)."""
    m = decl["metric"][decl["primary"]]
    k = math.log(float(decl["within"])) / abs(math.log(float(m["worst"]) / float(m["best"])))
    if not 0 <= decl["share"] < k / (1 + k):
        sys.exit(f"share {decl['share']} breaks the guarantee: it must be under {k / (1 + k):.4f} for within = {decl['within']}")
    if not 0 <= unstated(decl) <= 1:
        sys.exit(f"unstated {unstated(decl)} isn't from 0 to 1")
    for name, other in decl["metric"].items():
        if name != decl["primary"] and other["weight"] < 0:
            sys.exit(f"{name}: a negative weight")


def unstated(decl):
    """What a weighted metric a piece doesn't state counts at in the rest."""
    return decl.get("unstated", UNSTATED)


def score(decl, piece):
    """(score or None, {metric: value or None}) of a piece's metrics: (1 - share) × the primary's
    value + share × the rest, the weighted mean of the others, one not stated at `unstated`
    (no weighted metric: `unstated`)."""
    vals = {k: value(m, piece.get(k)) for k, m in decl["metric"].items()}
    lead = vals[decl["primary"]]
    if lead is None:
        return None, vals
    others = [k for k in decl["metric"] if k != decl["primary"]]
    w = sum(decl["metric"][k]["weight"] for k in others)
    u = unstated(decl)
    r = sum(decl["metric"][k]["weight"] * (u if vals[k] is None else vals[k]) for k in others) / w if w else u
    return (1 - decl["share"]) * lead + decl["share"] * r, vals


EXAMPLES = [
    ("a 2 m surface model against a 5 m bare-earth one, both of 2020, accuracy not stated",
     [{"spacing_m": 2, "surface": "surface", "year": 2020}, {"spacing_m": 5, "surface": "ground", "year": 2020}]),
    ("a 1.2 m surface model against a 1.25 m bare-earth one, both of 2020: within the factor, the kind takes no part",
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
    ("a 0.5 m bare-earth grid of 2021 against a 0.5 m surface model stating no year (France's LiDAR HD and MNS Correl)",
     [{"spacing_m": 0.5, "surface": "ground", "year": 2021}, {"spacing_m": 0.5, "surface": "surface"}]),
    ("the same grid of 2008 against the one stating no year",
     [{"spacing_m": 0.5, "surface": "ground", "year": 2008}, {"spacing_m": 0.5, "surface": "surface"}]),
]


def tie(decl):
    """How far apart two pieces' primary values (0-1) may be and still tie: a factor of `within` on
    the primary's log scale."""
    m = decl["metric"][decl["primary"]]
    return math.log(float(decl["within"])) / abs(math.log(float(m["worst"]) / float(m["best"])))


def avail_rank(decl, a):
    """Where availability `a` ranks: its place in `availability`, one not listed (or no order) after."""
    order = decl.get("availability", [])
    return order.index(a) if a in order else len(order)


# The extent classes, best founded first (extents.py's CLASSES; crate::inputs::extents::CLASSES).
CLASSES = ("mask", "files", "index", "service", "claimed", "unknown")


def class_rank(c):
    """Where extent class `c` ranks among pieces otherwise equal: its place in CLASSES, else last."""
    return CLASSES.index(c) if c in CLASSES else len(CLASSES)


def best(decl, pieces):
    """The index of the best of `pieces` (each its metrics, `availability` and `extent`; those with
    a score), or None: of those whose primary ties the finest's (within `within`), the best
    availability; of them, the best score; of equal scores, the best-founded extent (`class_rank`);
    of equals, the first (the server's Score::best)."""
    scored = [(k, score(decl, p)) for k, p in enumerate(pieces)]
    scored = [(k, s, vals[decl["primary"]]) for k, (s, vals) in scored if s is not None]
    if not scored:
        return None
    finest = max(lead for _, _, lead in scored)
    out = None
    for k, s, lead in scored:
        if finest - lead > tie(decl) + 1e-12:
            continue
        r, c = avail_rank(decl, pieces[k].get("availability")), class_rank(pieces[k].get("extent"))
        if out is None or r < out[1] or (r == out[1] and (s > out[2] or (s == out[2] and c < out[3]))):
            out = (k, r, s, c)
    return out[0]


def ranked(decl, pieces):
    """The indices of `pieces` with a score, best first: the best, then the best of the rest."""
    left = list(range(len(pieces)))
    out = []
    while (k := best(decl, [pieces[i] for i in left])) is not None:
        out.append(left.pop(k))
    return out


def said(decl, piece):
    s, vals = score(decl, piece)
    parts = ", ".join(f"{k} {piece[k]} → {vals[k]:.3f}" if vals[k] is not None else f"{k} not stated" for k in decl["metric"])
    return f"  {s:.3f}  {parts}" if s is not None else f"  none   {parts}"


def expected(decl, pieces_path):
    """The JSON --json prints: each piece's metrics, score and values, the examples' then the
    file's (`scored`), and each of the file's groups in order (`ranked`)."""
    with open(pieces_path) as f:
        given = json.load(f)
    pieces = [p for _, ps in EXAMPLES for p in ps] + given["pieces"]
    scored = []
    for p in pieces:
        s, vals = score(decl, p)
        scored.append({"metrics": p, "score": s, "values": vals})
    groups = [{"pieces": g, "order": ranked(decl, g)} for g in given["groups"]]
    return json.dumps({"scored": scored, "ranked": groups}, indent=1, sort_keys=True) + "\n"


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
    piece, pieces = {}, []
    for a in argv[2:] + ["--"]:
        if a == "--":
            if piece:
                print(said(decl, piece))
                pieces.append(piece)
            piece = {}
            continue
        k, v = a.split("=", 1)
        piece[k] = v if k in ("availability", "extent") or "values" in decl["metric"][k] else float(v)
    if len(pieces) > 1:
        print("best first: " + ", ".join(str(k + 1) for k in ranked(decl, pieces)))


if __name__ == "__main__":
    main(sys.argv)
