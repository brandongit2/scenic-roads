"""Vertical datum surfaces for the depth producers: an offset between two water levels (mean sea
level and a chart's datum, a survey's datum and MSL), from tide stations, over the water.

The surface is harmonic between the stations (each water cell the mean of its water neighbours,
the stations fixed; land a barrier, so a narrow bay's tide doesn't leak across a peninsula: the
Atlantic coast of Nova Scotia, about 1 m, stays out of the Minas Basin, 7.5 m), solved on a
lon/lat grid over the water bodies that hold a station (a station counts for the water nearest it,
within 25 km), and the nearest solved value beyond. A water body with no station within 25 km of
a solved cell gets no depth (NaN: nodata, an inland lake or lagoon), logged in one line; more than
5% of the water left so, or one body over 2%, stops the run (a missing area of the table; with
OpenSea a body at least half on a Natural Earth lake is a lake, logged and left out of that count), as does no
station within 25 km of any water. A caller may opt in to open-sea extrapolation (OpenSea; only
NONNA-10's chunked run does): such a body that is open sea (under half its cells on a Natural Earth lake, and
reaching within 6 cells of the grid's edge or over 100 km²; for a sea-only chart, `every_body`,
NONNA's, any size or place) takes the nearest solved cell's value,
or with no station near any water the nearest station's, however far (the cells and the farthest
km logged), and the stop counts only the water still without a value. Extrapolated cells are not
flagged in the output (one band). A
station table is a CSV with `lon`, `lat` and the value's column.
"""
from __future__ import annotations

import csv
from pathlib import Path

import numpy as np

from grid import Window, log, warp_onto


def stations(path: Path, column: str, skip=frozenset(), id_column: str | None = None) -> list[tuple[float, float, float]]:
    """(lon, lat, value) of each row with a value (rows whose `id_column` is in `skip` left out)."""
    out = []
    with open(path, newline="") as f:
        for r in csv.DictReader(f):
            if id_column and r.get(id_column) in skip:
                continue
            v = (r.get(column) or "").strip()
            if v:
                out.append((float(r["lon"]), float(r["lat"]), float(v)))
    return out


# A station farther than this (km) from its water, or water farther than this from every solved
# cell, isn't used: datum offsets change by decimetres over tens of km.
REACH_KM = 25.0
KM_DEG = 111.32


def nearest_water(water: np.ndarray, lon: float, lat: float, west: float, north: float, res: float, reach_km: float = REACH_KM):
    """The (row, col) of the water cell nearest the point, within `reach_km`, or None."""
    H, W = water.shape
    fr, fc = (north - lat) / res - 0.5, (lon - west) / res - 0.5
    ky = KM_DEG * res
    kx = ky * max(np.cos(np.radians(lat)), 0.05)
    ry, rx = int(np.ceil(reach_km / ky)) + 1, int(np.ceil(reach_km / kx)) + 1
    r0, r1 = max(0, int(np.floor(fr)) - ry), min(H, int(np.ceil(fr)) + ry + 1)
    c0, c1 = max(0, int(np.floor(fc)) - rx), min(W, int(np.ceil(fc)) + rx + 1)
    if r1 <= r0 or c1 <= c0:
        return None
    rr, cc = np.nonzero(water[r0:r1, c0:c1])
    if not len(rr):
        return None
    d = np.hypot((rr + r0 - fr) * ky, (cc + c0 - fc) * kx)
    k = int(np.argmin(d))
    return (int(rr[k]) + r0, int(cc[k]) + c0) if d[k] <= reach_km else None


def nearest_station(water: np.ndarray, pts, west: float, north: float, res: float):
    """(km, (lon, lat)) of the station nearest any water cell (for the message of a table without one)."""
    from scipy.spatial import cKDTree

    rr, cc = np.nonzero(water)
    tree = cKDTree(xyz(west + (cc + 0.5) * res, north - (rr + 0.5) * res))
    d, _ = tree.query(xyz([p[0] for p in pts], [p[1] for p in pts]))
    k = int(np.argmin(d))
    return float(chord_km(d[k])), (pts[k][0], pts[k][1])


R_KM = 6371.0


def xyz(lon, lat) -> np.ndarray:
    """Points on the unit sphere (for great-circle nearest neighbours by chord)."""
    lon, lat = np.radians(np.asarray(lon, float)), np.radians(np.asarray(lat, float))
    return np.column_stack([np.cos(lat) * np.cos(lon), np.cos(lat) * np.sin(lon), np.sin(lat)])


def chord_km(d) -> np.ndarray:
    return 2 * R_KM * np.arcsin(np.minimum(np.asarray(d) / 2, 1.0))


def grid_km(r1, c1, r2, c2, north: float, res: float) -> np.ndarray:
    """km between grid cells, each pair's east-west scale at its own mean latitude."""
    lat = north - ((np.asarray(r1) + np.asarray(r2)) / 2 + 0.5) * res
    return np.hypot((np.asarray(r1) - r2) * KM_DEG * res, (np.asarray(c1) - c2) * KM_DEG * res * np.cos(np.radians(lat)))


# Open sea (opt-in: OpenSea, only NONNA-10's chunked run): water beyond the reach that is sea, not a
# lagoon or lake, takes the nearest solved value however far. A body is open sea when it reaches
# within EDGE_CELLS of the grid's edge (the sea going on past the chunk; water_of pads the grid 4
# cells beyond the rasters' bounds, so 6 is 2 cells inside the data's footprint) or is over OPEN_KM2
# (NONNA covers the sea only, so a body that big cut off from the solved water is sea split by a
# coverage gap: ice, an unsurveyed strait; the sea's offsets change by decimetres over hundreds of
# km, an enclosed lagoon's can differ), and under LAKE_SHARE of its cells lie on an inland lake
# (Natural Earth's 10 m lakes, cell centres: Great Slave, Great Bear, the Great Lakes, Winnipeg are
# never extrapolated; a sea body that only touches a lake's edge stays sea).
OPEN_KM2 = 100.0
EDGE_CELLS = 6


class OpenSea:
    """The inland lakes open-sea extrapolation keeps out of (shapely geometries, lon/lat)."""

    def __init__(self, lakes, every_body: bool = False):
        """`every_body`: every body beyond the reach off the lakes is sea, whatever its size or
        place (a sea-only chart such as NONNA: its tiny disconnected bodies are sparse survey
        tracks, not lakes); otherwise only those at the edge or over OPEN_KM2."""
        self.lakes, self.every_body = list(lakes), every_body

    @classmethod
    def natural_earth(cls, sqlite: Path, every_body: bool = False) -> "OpenSea":
        """Natural Earth's ne_10m_lakes (every row: lakes, reservoirs, alkaline lakes) from its
        SQLite package (sources/basemap/natural_earth_vector.sqlite.zip, unzipped)."""
        import sqlite3

        from shapely import wkb

        if not Path(sqlite).is_file():
            raise SystemExit(f"{sqlite}: no Natural Earth SQLite (unzip sources/basemap/natural_earth_vector.sqlite.zip)")
        with sqlite3.connect(f"file:{sqlite}?mode=ro", uri=True) as db:
            rows = db.execute("select GEOMETRY from ne_10m_lakes where GEOMETRY is not null").fetchall()
        if not rows:
            raise SystemExit(f"{sqlite}: ne_10m_lakes is empty")
        return cls((wkb.loads(bytes(r[0])) for r in rows), every_body)

    def mask(self, shape, west: float, north: float, res: float) -> np.ndarray:
        """Cells whose centre is on a lake (not all_touched: a sea body grazing a lake's edge
        must not pick up its cells)."""
        from rasterio.features import rasterize
        from rasterio.transform import from_origin
        from shapely.geometry import box

        H, W = shape
        b = box(west, north - H * res, west + W * res, north)
        geoms = [g for g in self.lakes if g.intersects(b)]
        if not geoms:
            return np.zeros(shape, bool)
        return rasterize(geoms, out_shape=shape, transform=from_origin(west, north, res, res), all_touched=False, dtype="uint8").astype(bool)


def at_edge(lab: np.ndarray, bodies: np.ndarray) -> np.ndarray:
    """Which of `bodies` reach within EDGE_CELLS of the grid's edge."""
    H, W = lab.shape
    e = min(EDGE_CELLS, H, W)
    if e <= 0:
        return np.zeros(len(bodies), bool)
    edge = np.unique(np.concatenate([lab[:e].ravel(), lab[H - e:].ravel(), lab[:, :e].ravel(), lab[:, W - e:].ravel()]))
    return np.isin(bodies, edge)


def over_area(lab: np.ndarray, bodies: np.ndarray, north: float, res: float) -> np.ndarray:
    """Which of `bodies` are over OPEN_KM2 (each row's cells at its own latitude)."""
    from scipy.ndimage import sum as nsum

    H, W = lab.shape
    lat = north - (np.arange(H) + 0.5) * res
    area = np.broadcast_to(((KM_DEG * res) ** 2 * np.cos(np.radians(lat)))[:, None], (H, W))
    return np.atleast_1d(nsum(area, lab, bodies)) > OPEN_KM2


def open_sea(lab: np.ndarray, bodies: np.ndarray, north: float, res: float, lake: np.ndarray, every_body: bool = False) -> np.ndarray:
    """Which of `bodies` are open sea: not a lake (`inland`: under half on `lake`), and (unless `every_body`) at the
    edge or over OPEN_KM2."""
    on_lake = inland(lab, bodies, lake)
    if every_body:
        return ~on_lake
    return (at_edge(lab, bodies) | over_area(lab, bodies, north, res)) & ~on_lake


LAKE_SHARE = 0.5


def inland(lab: np.ndarray, bodies: np.ndarray, lake: np.ndarray) -> np.ndarray:
    """Which of `bodies` are lakes: at least LAKE_SHARE of their cells on an inland lake (`lake`),
    so a sea body touching a lake's edge (Bras d'Or, Lake Melville, a river mouth) stays sea."""
    from scipy.ndimage import sum as nsum

    if not len(bodies):
        return np.zeros(0, bool)
    on = np.atleast_1d(nsum(lake, lab, bodies))
    size = np.atleast_1d(nsum(np.ones(lab.shape), lab, bodies))
    return on >= LAKE_SHARE * size


def check_far(lab: np.ndarray, far: np.ndarray, dist, total: int, lake_far: np.ndarray | None = None) -> None:
    """Log the water left without a datum (`far`) and stop when it is over 5% of the water or one
    body over 2%: a missing area of the table. Bodies at least half on a Natural Earth lake (`lake_far`, with
    OpenSea) are lakes rightly left without depth: logged, not counted toward the stop."""
    if not far.any():
        return
    ids, size = np.unique(lab[far], return_counts=True)
    how = f", the farthest {dist[far].max():.0f} km from a solved cell" if dist is not None else ""
    log(f"datum surface: {len(ids)} water bodies, {int(size.sum())} cells, with no station within {REACH_KM:g} km (the largest {int(size.max())} cells{how}) get no depth")
    if lake_far is not None and lake_far.any():
        lk, lsize = np.unique(lab[lake_far], return_counts=True)
        log(f"datum surface: {len(lk)} of those bodies, {int(lsize.sum())} cells (the largest {int(lsize.max())}), are Natural Earth lakes: not a gap in the table")
        far = far & ~lake_far
        if not far.any():
            return
        ids, size = np.unique(lab[far], return_counts=True)
    if size.sum() > 0.05 * total or size.max() > 0.02 * total:
        raise SystemExit(f"{int(size.sum())} of {total} water cells (the largest body {int(size.max())}) have no station within {REACH_KM:g} km: "
                         "that is a missing area of the table, not a lake; the table needs rows for it")


class Surface:
    """An offset (m) on a lon/lat grid (`a`, its west, north and cell size)."""

    def __init__(self, a: np.ndarray, west: float, north: float, res: float):
        self.a, self.west, self.north, self.res = a.astype("float32"), west, north, res

    @classmethod
    def constant(cls, v: float) -> "Surface":
        return cls(np.full((2, 2), v, "float32"), -180.0, 90.0, 180.0)

    @classmethod
    def harmonic(cls, pts, water: np.ndarray, west: float, north: float, res: float, sea: OpenSea | None = None, snap_km: float = REACH_KM) -> "Surface":
        """Solved over `water` (a bool lon/lat grid from `west`, `north`, cells of `res` degrees);
        with `sea`, open sea beyond the reach is extrapolated (see OpenSea). A station further than
        `snap_km` from the water is left out."""
        from scipy.ndimage import distance_transform_edt, label
        from scipy.sparse import coo_matrix
        from scipy.sparse.linalg import spsolve

        H, W = water.shape
        if not water.any() or not pts:
            raise SystemExit("a datum surface needs water and stations")
        fixed: dict = {}
        for lon, lat, v in pts:
            hit = nearest_water(water, lon, lat, west, north, res, snap_km)
            if hit:
                fixed.setdefault(hit, []).append(v)
        lab, _ = label(water)
        total = int(water.sum())
        lake = sea.mask(water.shape, west, north, res) if sea else None
        bodies = np.unique(lab[water])
        if not fixed and sea is not None and open_sea(lab, bodies, north, res, lake, sea.every_body).any():
            # no station within reach of any water: open sea takes the nearest station's value
            from scipy.spatial import cKDTree

            ext = np.isin(lab, bodies[open_sea(lab, bodies, north, res, lake, sea.every_body)])
            far = water & ~ext
            check_far(lab, far, None, total, far & np.isin(lab, bodies[inland(lab, bodies, lake)]))
            rr, cc = np.nonzero(ext)
            d, k = cKDTree(xyz([p[0] for p in pts], [p[1] for p in pts])).query(xyz(west + (cc + 0.5) * res, north - (rr + 0.5) * res))
            out = np.full((H, W), np.nan, "float32")
            out[rr, cc] = np.array([p[2] for p in pts])[k]
            log(f"datum surface: no station within {REACH_KM:g} km; {len(rr)} open-sea cells extrapolated from the nearest station (the farthest {chord_km(d).max():.0f} km)")
            return cls(out, west, north, res)
        if not fixed:
            d, at = nearest_station(water, pts, west, north, res)
            raise SystemExit(
                f"a datum surface needs a station within {REACH_KM:g} km of the water: none is, over "
                f"{west:.2f}..{west + W * res:.2f} E, {north - H * res:.2f}..{north:.2f} N (the nearest is {d:.0f} km from its nearest water cell, "
                f"at {at[0]:.2f}, {at[1]:.2f}); a far station's offset would be a wrong depth, so the table needs rows for this area")
        lat0 = north - H * res / 2
        solve = water & np.isin(lab, list({lab[k] for k in fixed}))
        # a water body without a station takes the nearest solved value, if one is within reach
        _, (iy2, ix2) = distance_transform_edt(~solve, return_indices=True, sampling=(KM_DEG * res, KM_DEG * res * np.cos(np.radians(lat0))))
        dist = grid_km(np.arange(H)[:, None], np.arange(W)[None, :], iy2, ix2, north, res)
        bodies = np.unique(lab[water & ~solve])
        far = np.zeros((H, W), bool)
        if len(bodies):
            from scipy.ndimage import minimum

            dmin = np.atleast_1d(minimum(dist, lab, bodies))
            gone = bodies[dmin > REACH_KM]
            if len(gone):
                ext = np.zeros((H, W), bool)
                if sea is not None:
                    opn = gone[open_sea(lab, gone, north, res, lake, sea.every_body)]
                    ext = np.isin(lab, opn)
                    if ext.any():
                        log(f"datum surface: {len(opn)} open-sea water bodies, {int(ext.sum())} cells, beyond {REACH_KM:g} km extrapolated from the nearest solved cell "
                            f"(the farthest {dist[ext].max():.0f} km)")
                far = np.isin(lab, gone) & ~ext
                check_far(lab, far, dist, total, np.isin(lab, gone[inland(lab, gone, lake)]) if sea is not None else None)
        idx = -np.ones((H, W), np.int64)
        cells = np.argwhere(solve)
        idx[solve] = np.arange(len(cells))
        rows, cols, vals = [], [], []
        b = np.zeros(len(cells))
        for k, (r, c) in enumerate(cells):
            if (r, c) in fixed:
                rows.append(k), cols.append(k), vals.append(1.0)
                b[k] = float(np.mean(fixed[(r, c)]))
                continue
            nb = [(r + dr, c + dc) for dr, dc in ((1, 0), (-1, 0), (0, 1), (0, -1)) if 0 <= r + dr < H and 0 <= c + dc < W and solve[r + dr, c + dc]]
            rows.append(k), cols.append(k), vals.append(float(len(nb) or 1))
            for rr, cc in nb:
                rows.append(k), cols.append(int(idx[rr, cc])), vals.append(-1.0)
        A = coo_matrix((vals, (rows, cols)), shape=(len(cells), len(cells))).tocsr()
        x = spsolve(A, b)
        out = np.full((H, W), np.nan, "float32")
        out[solve] = x
        log(f"datum surface: {len(pts)} stations on {len(fixed)} cells, {len(cells)} water cells, {x.min():.2f}–{x.max():.2f} m")
        out = out[iy2, ix2]
        out[far] = np.nan
        return cls(out, west, north, res)

    def on(self, win: Window) -> np.ndarray:
        """The surface on a depth grid's window (bilinear)."""
        from rasterio.transform import from_origin

        return warp_onto((self.a, from_origin(self.west, self.north, self.res, self.res), "EPSG:4326"), 1, win, "bilinear", src_nodata=np.nan)


def from_table(table, rasters, res: float = 0.005, sea: OpenSea | None = None, below: float | None = None,
               snap_km: float = REACH_KM) -> Surface | None:
    """A station table's surface (`table`: path, column, ids left out, the id column) over the
    water the `rasters` (a source's files, any CRS) have data on (with `below`, their values under
    it: a grid of land and sea alike; a station further than `snap_km` from it left out): a constant when every station says the same; None without a table."""
    if table is None:
        return None
    path, column, skip, id_column = table
    pts = stations(Path(path), column, skip, id_column)
    if not pts:
        raise SystemExit(f"{path}: no station has {column}")
    if len({round(p[2], 3) for p in pts}) == 1:
        return Surface.constant(pts[0][2])
    water, w, n, r = water_of(rasters, res, below)
    return Surface.harmonic(pts, water, w, n, r, sea, snap_km)


def water_of(rasters, res: float, below: float | None = None) -> tuple[np.ndarray, float, float, float]:
    """A lon/lat grid (cells of `res`°) of where any of `rasters` has data, over their bounds (with
    `below`: where any of a cell's values is under it)."""
    import rasterio
    from rasterio.enums import Resampling
    from rasterio.transform import from_origin
    from rasterio.warp import reproject, transform_bounds

    boxes = []
    for p in rasters:
        with rasterio.open(p) as d:
            boxes.append(transform_bounds(d.crs, "EPSG:4326", *d.bounds))
    w, s = min(b[0] for b in boxes) - 4 * res, min(b[1] for b in boxes) - 4 * res
    e, n = max(b[2] for b in boxes) + 4 * res, max(b[3] for b in boxes) + 4 * res
    W, H = int(np.ceil((e - w) / res)), int(np.ceil((n - s) / res))
    out = np.zeros((H, W), bool)
    for p in rasters:
        with rasterio.open(p) as d:
            a = np.full((H, W), np.nan, "float32")
            reproject(rasterio.band(d, 1), a, dst_transform=from_origin(w, n, res, res), dst_crs="EPSG:4326", dst_nodata=np.nan,
                      resampling=Resampling.max if below is None else Resampling.min)
            out |= np.isfinite(a) if below is None else np.isfinite(a) & (a < below)
    return out, w, n, res


def water_from(win: Window, *depths: np.ndarray, res: float = 0.01) -> tuple[np.ndarray, float, float, float]:
    """A lon/lat water grid (cells of `res`°) over the window: where any of `depths` (on the window)
    has data."""
    from rasterio.transform import from_origin
    from rasterio.warp import Resampling, reproject

    w, s, e, n = win.bounds_lonlat()
    W, H = max(2, int(np.ceil((e - w) / res))), max(2, int(np.ceil((n - s) / res)))
    out = np.zeros((H, W), bool)
    for d in depths:
        a = np.full((H, W), np.nan, "float32")
        reproject(np.where(np.isfinite(d), 1.0, np.nan).astype("float32"), a, src_transform=win.transform(), src_crs="EPSG:3857", src_nodata=np.nan,
                  dst_transform=from_origin(w, n, res, res), dst_crs="EPSG:4326", dst_nodata=np.nan, resampling=Resampling.max)
        out |= np.isfinite(a)
    return out, w, n, res
