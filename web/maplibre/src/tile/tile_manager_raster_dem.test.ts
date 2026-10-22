import {describe, test, expect, vi} from 'vitest';
import {backfillDEM} from './tile_manager_raster_dem.ts';
import {Tile} from './tile.ts';
import {OverscaledTileID} from './tile_id.ts';
import {InViewTiles} from './tile_manager_in_view_tiles.ts';

import type {DEMData} from '../data/dem_data.ts';

function demTile(z: number, x: number, y: number, wrap = 0, overscaledZ = z): Tile {
    const tile = new Tile(new OverscaledTileID(overscaledZ, wrap, z, x, y), 512);
    tile.state = 'loaded';
    tile.dem = {dim: 256, backfillFrom: vi.fn()} as any as DEMData;
    return tile;
}

function inView(...tiles: Tile[]): InViewTiles {
    const v = new InViewTiles();
    for (const t of tiles) v.setTile(t.tileID.key, t);
    return v;
}

describe('backfillDEM', () => {
    test('nothing without neighbours', () => {
        const tile = demTile(1, 0, 0);
        backfillDEM(tile, inView(tile));
        expect(tile.dem.backfillFrom).toHaveBeenCalledTimes(0);
    });

    test('a neighbour of its zoom both ways, where it lies', () => {
        const tile = demTile(1, 0, 0);
        const east = demTile(1, 1, 0);
        backfillDEM(tile, inView(tile, east));
        expect(tile.dem.backfillFrom).toHaveBeenCalledWith(east.dem, 1, 0, 1);
        expect(east.dem.backfillFrom).toHaveBeenCalledWith(tile.dem, -1, 0, 1);
        expect(tile.needsHillshadePrepare && east.needsHillshadePrepare).toBe(true);
    });

    test('across the antimeridian', () => {
        const tile = demTile(1, 1, 0);
        const east = demTile(1, 0, 0, 1);
        backfillDEM(tile, inView(tile, east));
        expect(tile.dem.backfillFrom).toHaveBeenCalledWith(east.dem, 1, 0, 1);
    });

    test('neighbours of other zooms, coarsest first; not a tile it overlaps nor one past its border', () => {
        const tile = demTile(14, 103, 101);
        const coarse = demTile(12, 26, 25, 0, 14); // z14 104–108 × 100–104: east, from a row up
        const south = demTile(15, 206, 204); // z14 103–103.5 × 102–102.5
        const parent = demTile(13, 51, 50); // over it
        const far = demTile(16, 410, 404); // z14 102.5–102.75: past its west border
        backfillDEM(tile, inView(far, south, tile, parent, coarse));
        expect(vi.mocked(tile.dem.backfillFrom).mock.calls).toEqual([
            [coarse.dem, 1, -1, 4],
            [south.dem, 0, 1, 0.5],
        ]);
        // each neighbour's border from the tiles touching it, the loaded one among them
        expect(coarse.dem.backfillFrom).toHaveBeenCalledWith(tile.dem, -0.25, 0.25, 0.25);
        expect(south.dem.backfillFrom).toHaveBeenCalledWith(tile.dem, 0, -2, 2);
        expect(far.dem.backfillFrom).toHaveBeenCalledTimes(0);
        expect(parent.dem.backfillFrom).toHaveBeenCalledTimes(0);
    });
});
