#!/usr/bin/env python3
"""Building footprints for the roadside-buildings factor: Overture Maps buildings (OSM, Microsoft
and Google footprints merged; ODbL / CDLA Permissive 2.0), bounding boxes only.

Streams each region's buildings from Overture's public S3 release with DuckDB (only the bbox
column of the row groups inside the region is read) and writes data/buildings/<region>.f32:
little-endian float32 [xmin, ymin, xmax, ymax] per building, degrees. The `scenic` step keeps the
ones near roads. Heights are not used.

usage: buildings.py [region ...]
       buildings.py --world parts --zoom z --workers n --release r   release r whole, for the
                                             build agent (crates/pipeline/src/buildtiles.rs): every
                                             building's box, by the zoom-z tile holding its centre,
                                             into parts/<file>/<z>-<x>-<y>.f32, a folder per file of
                                             the release with `.done` once written (folders done
                                             are skipped), and the release's files in
                                             parts/files.json; `progress: d/t files` on stderr
"""
from __future__ import annotations

import json
import shutil
import sys
import time
from concurrent.futures import ProcessPoolExecutor, as_completed
from pathlib import Path

import duckdb
import numpy as np

RELEASE = "2026-09-23.1"
SRC = f"s3://overturemaps-us-west-2/release/{RELEASE}/theme=buildings/type=building/*.parquet"
OUT = Path(__file__).resolve().parent.parent / "data" / "buildings"

def regions() -> dict[str, list[tuple]]:
    """West, south, east, north, per building file (regions.json "buildings"), or a list of them
    (Japan, kept clear of Korea). Boxes overlap a little and take in some neighbours; the scenic
    step only uses buildings near our roads, and a building counted twice changes nothing. (Read
    only here: the app's dem/, where the agent runs the world mode, has no regions.json.)"""
    v = json.loads((Path(__file__).resolve().parent.parent / "regions.json").read_text())["buildings"]
    return {k: [tuple(x) for x in (b if isinstance(b[0], list) else [b])] for k, b in v.items()}


def connect(threads: int | None = None):
    config = {"custom_user_agent": "scenic-roads/0.1 (personal offline map)"}
    if threads:
        config["threads"] = threads
    con = duckdb.connect(config=config)
    con.execute("INSTALL httpfs; LOAD httpfs; SET s3_region='us-west-2'; SET s3_access_key_id=''; SET s3_secret_access_key=''; SET enable_progress_bar = false;")
    return con


def _tile_xy(lon: np.ndarray, lat: np.ndarray, z: int) -> tuple[np.ndarray, np.ndarray]:
    """The zoom-z tile holding each point (Web Mercator, as the agent's tiles)."""
    n = 1 << z
    x = np.clip(np.floor((lon + 180.0) / 360.0 * n), 0, n - 1)
    r = np.radians(np.clip(lat, -85.05112878, 85.05112878))
    y = np.clip(np.floor((1.0 - np.log(np.tan(r) + 1.0 / np.cos(r)) / np.pi) / 2.0 * n), 0, n - 1)
    return x.astype(np.int64), y.astype(np.int64)


def _scan(i: int, url: str, parts: str, z: int) -> int:
    """One file of the release into parts/<i>/: its buildings' boxes by tile."""
    out = Path(parts) / f"{i:04d}"
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True)
    con = connect(threads=4)
    cols = con.execute(f"SELECT bbox.xmin AS a, bbox.ymin AS b, bbox.xmax AS c, bbox.ymax AS d FROM read_parquet('{url}')").fetchnumpy()
    boxes = np.stack([np.asarray(cols[k], dtype=np.float32) for k in "abcd"], axis=1)
    x, y = _tile_xy(((boxes[:, 0] + boxes[:, 2]) * 0.5).astype(np.float64), ((boxes[:, 1] + boxes[:, 3]) * 0.5).astype(np.float64), z)
    key = x * (1 << z) + y
    order = np.argsort(key, kind="stable")
    key, boxes = key[order], boxes[order]
    cuts = np.flatnonzero(np.diff(key)) + 1
    for k, part in zip(np.split(key, cuts), np.split(boxes, cuts)):
        if len(part):
            tx, ty = divmod(int(k[0]), 1 << z)
            np.ascontiguousarray(part, dtype="<f4").tofile(out / f"{z}-{tx}-{ty}.f32")
    (out / ".done").touch()
    return len(boxes)


def world(parts: Path, z: int, workers: int, release: str) -> None:
    parts.mkdir(parents=True, exist_ok=True)
    src = f"s3://overturemaps-us-west-2/release/{release}/theme=buildings/type=building/*.parquet"
    files = [r[0] for r in connect().execute(f"SELECT file FROM glob('{src}') ORDER BY file").fetchall()]
    # (An empty listing is a release Overture no longer serves, or a failed listing: never an
    # empty world.)
    if not files:
        sys.exit(f"buildings: no files for release {release} ({src})")
    listed = parts / "files.json"
    if listed.exists() and json.loads(listed.read_text()) != files:
        sys.exit(f"buildings: release {release}'s files aren't those its parts were made from ({listed})")
    listed.write_text(json.dumps(files))
    todo = [(i, f) for i, f in enumerate(files) if not (parts / f"{i:04d}" / ".done").exists()]
    print(f"buildings: {len(files)} files in release {release}, {len(todo)} to scan", file=sys.stderr, flush=True)
    done, n, t0 = len(files) - len(todo), 0, time.time()
    with ProcessPoolExecutor(workers) as ex:
        for fut in as_completed([ex.submit(_scan, i, f, str(parts), z) for i, f in todo]):
            n += fut.result()
            done += 1
            print(f"buildings: {n:,} so far ({time.time() - t0:.0f} s)", file=sys.stderr)
            print(f"progress: {done}/{len(files)} files", file=sys.stderr, flush=True)


def main() -> None:
    if len(sys.argv) == 9 and sys.argv[1:8:2] == ["--world", "--zoom", "--workers", "--release"]:
        world(Path(sys.argv[2]), int(sys.argv[4]), int(sys.argv[6]), sys.argv[8])
        return
    REGIONS = regions()
    OUT.mkdir(parents=True, exist_ok=True)
    con = connect()
    todo = sys.argv[1:] or list(REGIONS)
    for name in todo:
        path = OUT / f"{name}.f32"
        if path.exists():
            print(f"{name}: exists")
            continue
        t0 = time.time()
        tmp = path.with_suffix(".tmp")
        count = 0
        with tmp.open("wb") as f:
            # One query per box (a plain range filter lets DuckDB skip the row groups outside it).
            for w, s, e, n in REGIONS[name]:
                q = f"""
                    SELECT bbox.xmin AS a, bbox.ymin AS b, bbox.xmax AS c, bbox.ymax AS d FROM read_parquet('{SRC}')
                    WHERE bbox.xmin BETWEEN {w} AND {e} AND bbox.ymin BETWEEN {s} AND {n}
                """
                reader = con.execute(q).fetch_record_batch(1_000_000)
                for batch in reader:
                    cols = [batch.column(i).to_numpy(zero_copy_only=False).astype(np.float32) for i in range(4)]
                    np.stack(cols, axis=1).tofile(f)
                    count += batch.num_rows
                    print(f"\r{name}: {count:,} buildings ({time.time() - t0:.0f} s)", end="", flush=True)
        tmp.rename(path)
        print(f"\r{name}: {count:,} buildings, {path.stat().st_size / 1e9:.2f} GB ({time.time() - t0:.0f} s)")


if __name__ == "__main__":
    main()
