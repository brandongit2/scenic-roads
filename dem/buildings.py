#!/usr/bin/env python3
"""Building footprints for the roadside-buildings factor: Overture Maps buildings (OSM, Microsoft
and Google footprints merged; ODbL / CDLA Permissive 2.0), bounding boxes only.

Streams each region's buildings from Overture's public S3 release with DuckDB (only the bbox
column of the row groups inside the region is read) and writes data/buildings/<region>.f32:
little-endian float32 [xmin, ymin, xmax, ymax] per building, degrees. The `scenic` step keeps the
ones near roads. Heights are not used.

usage: buildings.py [region ...]
"""
from __future__ import annotations

import sys
import time
import json
from pathlib import Path

import duckdb
import numpy as np

RELEASE = "2026-09-23.1"
SRC = f"s3://overturemaps-us-west-2/release/{RELEASE}/theme=buildings/type=building/*.parquet"
OUT = Path(__file__).resolve().parent.parent / "data" / "buildings"

# west, south, east, north, per building file (regions.json "buildings"), or a list of them (Japan,
# kept clear of Korea). Boxes overlap a little and take in some neighbours; the scenic step only
# uses buildings near our roads, and a building counted twice changes nothing.
REGIONS = {k: [tuple(b) for b in (v if isinstance(v[0], list) else [v])]
           for k, v in json.loads((Path(__file__).resolve().parent.parent / "regions.json").read_text())["buildings"].items()}


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    con = duckdb.connect()
    con.execute("INSTALL httpfs; LOAD httpfs; SET s3_region='us-west-2'; SET s3_access_key_id=''; SET s3_secret_access_key='';")
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
