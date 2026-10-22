import type {Tile} from './tile.ts';
import type {InViewTiles} from './tile_manager_in_view_tiles.ts';

/**
 * For raster terrain source, backfill DEM to eliminate visible tile boundaries.
 *
 * Scenic Roads: across zooms too. A source's tiles may sit beside tiles of another zoom
 * (`scenic:zoom-by-area`, or a coarser tile standing in while a finer one loads), and upstream
 * filled a tile's border only from its eight neighbours of its own zoom, leaving the rest clamped
 * to the tile's edge (a line in the hill-shading along every edge between zooms). Now the loaded
 * tile's border, and the border of every renderable tile it touches, is filled from all the
 * renderable tiles touching it, coarsest first so the finest data at a place is written last
 * (`DEMData.backfillFrom`).
 */
export function backfillDEM(tile: Tile, inViewTiles: InViewTiles): void {
    if (!tile.dem) return;
    const tiles: Tile[] = [];
    for (const id of inViewTiles.getRenderableIds()) {
        const t = inViewTiles.getTileById(id);
        if (t?.dem && t !== tile) tiles.push(t);
    }
    tiles.push(tile);
    tiles.sort((a, b) => a.tileID.canonical.z - b.tileID.canonical.z);
    fillBorder(tile, tiles);
    for (const t of tiles) {
        if (t !== tile && touches(t, tile)) fillBorder(t, tiles);
    }
}

/** A tile's extent in world tiles of zoom 0 ([x, y, size]; x counts the world copies). */
function extent(t: Tile): [number, number, number] {
    const c = t.tileID.canonical;
    const size = 1 / (1 << c.z);
    return [t.tileID.wrap + c.x * size, c.y * size, size];
}

/** Where `b` lies in units of `a` (`a` from 0 to 1): [x, y, size]. */
function relative(a: Tile, b: Tile): [number, number, number] {
    const [ax, ay, as] = extent(a);
    const [bx, by, bs] = extent(b);
    return [(bx - ax) / as, (by - ay) / as, bs / as];
}

/** Whether `b` meets `a`'s 2 px border and doesn't overlap `a` itself. */
function touches(a: Tile, b: Tile): boolean {
    const [x, y, s] = relative(a, b);
    const m = 2 / a.dem.dim;
    const meets = x < 1 + m && x + s > -m && y < 1 + m && y + s > -m;
    const overlaps = x < 1 && x + s > 0 && y < 1 && y + s > 0;
    return meets && !overlaps;
}

/** `tile`'s border filled from each of `tiles` touching it, in their order. */
function fillBorder(tile: Tile, tiles: Tile[]) {
    let filled = false;
    for (const t of tiles) {
        if (t === tile || !touches(tile, t)) continue;
        const [x, y, s] = relative(tile, t);
        tile.dem.backfillFrom(t.dem, x, y, s);
        filled = true;
    }
    if (filled) {
        tile.needsHillshadePrepare = true;
        tile.needsTerrainPrepare = true;
        tile.needsColorReliefPrepare = true;
    }
}
