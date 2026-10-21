// Scenic Roads: a source's finest zoom by area (covering_tiles.ts `ZoomByArea`, FORK.md): outside
// the listed areas the cover is upstream's; inside, tiles go on to the area's zoom; no gaps between.
import {describe, expect, test} from 'vitest';
import {LngLat} from '../lng_lat.ts';
import {areaMaxZoom, coveringTiles, zoomByArea} from './covering_tiles.ts';
import {MercatorTransform} from './mercator_transform.ts';
import type {OverscaledTileID} from '../../tile/tile_id.ts';

/** Z10 tile 296/367's west edge, as longitude. */
const WEST = 296 / 1024 * 360 - 180;
const LAT = 45.3;

function view(pitch: number): MercatorTransform {
    const t = new MercatorTransform();
    t.resize(1200, 800);
    t.setZoom(15.5);
    t.setCenter(new LngLat(WEST, LAT));
    t.setPitch(pitch);
    return t;
}

/** Whether tile `id` lies in z10 tile 296/367. */
const inArea = (id: OverscaledTileID) => id.canonical.z >= 10 &&
    id.canonical.x >> (id.canonical.z - 10) === 296 && id.canonical.y >> (id.canonical.z - 10) === 367;

const keys = (ids: OverscaledTileID[]) => ids.map((id) => id.key).sort();

describe('a source\'s finest zoom by area', () => {
    test('its style form, and a tile\'s finest zoom', () => {
        const a = zoomByArea({zoom: 10, tiles: {'296/367': 16, 'bad': 3}}, 12)!;
        expect(a.max.size).toBe(1);
        expect(areaMaxZoom(a, 12, 14, 296 * 16 + 5, 367 * 16 + 9)).toBe(16);
        expect(areaMaxZoom(a, 12, 10, 296, 367)).toBe(16);
        expect(areaMaxZoom(a, 12, 14, 295 * 16, 367 * 16)).toBe(12);
        expect(areaMaxZoom(a, 12, 9, 148, 183)).toBe(12);
        expect(areaMaxZoom(undefined, 12, 14, 0, 0)).toBe(12);
        // A zoom past the source's maxzoom, or no tiles: none.
        expect(zoomByArea({zoom: 13, tiles: {'1/1': 16}}, 12)).toBeUndefined();
        expect(zoomByArea({zoom: 10, tiles: {}}, 12)).toBeUndefined();
        expect(zoomByArea(undefined, 12)).toBeUndefined();
        expect(zoomByArea({zoom: 10, tiles: {'1/2': 16}}, 12)!.key).not.toBe(a.key);
    });

    for (const pitch of [0, 60]) {
        test(`at pitch ${pitch}: today's tiles outside the area, finer ones inside, no gaps`, () => {
            const t = view(pitch);
            const opts = {tileSize: 256, minzoom: 0, maxzoom: 12};
            const before = coveringTiles(t, opts);
            const after = coveringTiles(t, {...opts, zoomByArea: zoomByArea({zoom: 10, tiles: {'296/367': 16}}, 12)});
            expect(before.every((id) => id.canonical.z <= 12)).toBe(true);
            // Outside: exactly as before.
            expect(keys(after.filter((id) => !inArea(id)))).toEqual(keys(before.filter((id) => !inArea(id))));
            // Inside: finer, to the area's zoom at most.
            const inside = after.filter(inArea);
            expect(inside.some((id) => id.canonical.z > 12)).toBe(true);
            expect(inside.every((id) => id.canonical.z <= 16)).toBe(true);
            // No gaps: each tile of before's inside is covered by its descendants in after, whole
            // (or the tile itself), as far as they're in view.
            for (const id of before.filter(inArea)) {
                const under = inside.filter((c) => c.canonical.z >= id.canonical.z &&
                    c.canonical.x >> (c.canonical.z - id.canonical.z) === id.canonical.x &&
                    c.canonical.y >> (c.canonical.z - id.canonical.z) === id.canonical.y);
                expect(under.length).toBeGreaterThan(0);
                const share = under.reduce((s, c) => s + 4 ** (id.canonical.z - c.canonical.z), 0);
                expect(share).toBeLessThanOrEqual(1 + 1e-9);
            }
            // And nothing in after overlaps: no tile is another's ancestor.
            for (const a of after) {
                for (const b of after) {
                    if (a === b || b.canonical.z <= a.canonical.z) continue;
                    const d = b.canonical.z - a.canonical.z;
                    expect(b.canonical.x >> d === a.canonical.x && b.canonical.y >> d === a.canonical.y && a.wrap === b.wrap).toBe(false);
                }
            }
        });
    }
});
