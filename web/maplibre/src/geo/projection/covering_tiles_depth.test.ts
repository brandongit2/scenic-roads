// Scenic Roads: a tile's zoom by its depth along the camera's forward vector (covering_tiles.ts,
// FORK.md): tiles at the same depth get the same zoom, at the screen's sides as in the middle.
import {describe, expect, test} from 'vitest';
import {LngLat} from '../lng_lat.ts';
import {coveringTiles, tileDepthDistance2d} from './covering_tiles.ts';
import {MercatorTransform} from './mercator_transform.ts';

describe('tile zoom by depth', () => {
    test('tiles at equal depth, on the line of sight and off it, are given the same distance', () => {
        const cam = {x: 0.5, y: 0.5};
        const pitch = 60 * Math.PI / 180;
        // Bearing 0 (looking north, toward −y): a z14 tile's row ahead, straight on and far to the side.
        const z = 14, n = 2 ** z;
        const row = Math.floor(0.5 * n) - 40;
        const ahead = tileDepthDistance2d(cam, 0.0001, pitch, 0, {x: n / 2, y: row, z}, 0, 1);
        const side = tileDepthDistance2d(cam, 0.0001, pitch, 0, {x: n / 2 + 60, y: row, z}, 0, 1);
        expect(side).toBeCloseTo(ahead, 12);
        // Turned 90° (looking east): the same holds for a column.
        const a2 = tileDepthDistance2d(cam, 0.0001, pitch, Math.PI / 2, {x: n / 2 + 40, y: n / 2, z}, 0, 1);
        const s2 = tileDepthDistance2d(cam, 0.0001, pitch, Math.PI / 2, {x: n / 2 + 40, y: n / 2 - 60, z}, 0, 1);
        expect(s2).toBeCloseTo(a2, 12);
        // Never more than the distance it's given; straight down, the camera's own spot for all.
        expect(tileDepthDistance2d(cam, 0.0001, pitch, 0, {x: n / 2 + 60, y: row, z}, 0, 1e-9)).toBe(1e-9);
        expect(tileDepthDistance2d(cam, 0.0001, 0, 0, {x: n / 2 + 60, y: row, z}, 0, 1)).toBeCloseTo(0, 12);
    });

    test('a tilted view: the row through its center is one zoom from edge to edge', () => {
        const t = new MercatorTransform();
        t.resize(2400, 800);
        t.setCenter(new LngLat(139.76, 35.68));
        t.setZoom(13.5);
        t.setPitch(65);
        const tiles = coveringTiles(t, {tileSize: 512, minzoom: 0, maxzoom: 14});
        const c = t.center;
        const cy = (1 - Math.log(Math.tan(Math.PI / 4 + c.lat * Math.PI / 360)) / Math.PI) / 2;
        const inRow = tiles.filter((id) => {
            const n = 2 ** id.canonical.z;
            return id.canonical.y / n <= cy && cy < (id.canonical.y + 1) / n;
        });
        expect(inRow.length).toBeGreaterThan(2);
        const zs = new Set(inRow.map((id) => id.canonical.z));
        expect(zs.size).toBe(1);
    });
});
