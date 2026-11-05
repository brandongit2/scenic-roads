import {describe, test, expect} from 'vitest';
import {TileManager} from './tile_manager.ts';
import {OverscaledTileID} from './tile_id.ts';
import {MercatorTransform} from '../geo/projection/mercator_transform.ts';
import {LngLat} from '../geo/lng_lat.ts';
import {coveringTiles} from '../geo/projection/covering_tiles.ts';

// Scenic Roads: a source with `detail` (TileManager._refineCover).
describe('TileManager._refineCover', () => {
    const T = new OverscaledTileID(8, 0, 8, 100, 100);
    const far = new OverscaledTileID(8, 0, 8, 120, 100);
    const kids = (id: OverscaledTileID) => id.children(22).flatMap(c => c.children(22));

    function setup(loaded: string[]) {
        const tm = Object.create(TileManager.prototype) as TileManager;
        tm.detailBudget = 8; // (512² px × 8: room for all 32 finer tiles, 256² texels each)
        tm.maxTilesADrape = 240;
        tm._source = {maxzoom: 10} as any;
        tm._detailTiles = 0;
        tm._detailOffset = new Map();
        tm.map = {getPixelRatio: () => 1} as any;
        tm._inViewTiles = {getTileById: (key: string) => loaded.includes(key) ? {hasData: () => true, state: 'loaded'} : undefined} as any;
        const transform = new MercatorTransform();
        transform.resize(512, 512);
        transform.setCenter(new LngLat(-39.4, 39.4));
        transform.setZoom(8);
        return {tm, transform};
    }
    const keys = (ids: OverscaledTileID[]) => ids.map(i => i.key).sort();

    test('a tile is refined only once it has data itself, moving or not; pending till then', () => {
        const fine = [...kids(T), ...kids(far)];
        let {tm, transform} = setup([]);
        expect(keys(tm._refineCover(transform, [T, far], () => fine))).toEqual(keys([T, far]));
        expect(tm._detailPending).toBe(true);
        ({tm, transform} = setup([T.key, far.key]));
        expect(keys(tm._refineCover(transform, [T, far], () => fine))).toEqual(keys(fine));
        expect(tm._detailPending).toBe(false);
        // the finer tiles' zooms beyond the screen's, for the drapes' sizes
        expect(tm._detailOffset.get(kids(T)[0].key)?.offset).toBe(2);
        expect(tm._detailTiles).toBe(32);
    });

    test('follows the finer cover as the camera pans, the screen\'s own cover the same or not', () => {
        // (a finer cover kept while the screen's own stayed the same left the part of a tile newly
        // in view uncovered: 14 frames in 120 of a slow pan at saguenay, 5 Nov)
        const {tm} = setup([]);
        tm._inViewTiles = {getTileById: () => ({hasData: () => true, state: 'loaded'})} as any;
        tm.detailBudget = 1000;
        tm._source = {maxzoom: 16} as any;
        const transform = new MercatorTransform();
        transform.resize(800, 600);
        transform.setZoom(11.5);
        transform.setPitch(50);
        const cover = (tileSize: number) => coveringTiles(transform, {tileSize, minzoom: 0, maxzoom: 16});
        let sameToday = 0;
        let lastToday = '';
        for (let i = 0; i < 60; i++) {
            transform.setCenter(new LngLat(-71 + i * 0.0007, 48.4));
            const today = cover(512), fine = cover(128);
            if (keys(today).join() === lastToday) sameToday++;
            lastToday = keys(today).join();
            const got = tm._refineCover(transform, today, () => fine);
            const uncovered = fine.filter(f => !got.some(g => g.canonical.equals(f.canonical) || f.canonical.isChildOf(g.canonical)));
            expect(uncovered.map(f => f.key)).toEqual([]);
        }
        expect(sameToday).toBeGreaterThan(0);
    });

    test('the finer tiles\' offsets stay the same Map while they are the same', () => {
        const fine = [...kids(T), ...kids(far)];
        const {tm, transform} = setup([T.key, far.key]);
        tm._refineCover(transform, [T, far], () => fine);
        const offsets = tm._detailOffset;
        tm._refineCover(transform, [T, far], () => fine);
        expect(tm._detailOffset).toBe(offsets);
        tm._refineCover(transform, [T], () => kids(T));
        expect(tm._detailOffset).not.toBe(offsets);
    });

    test('the out-of-view cache keeps as many more as the cover\'s finer tiles', () => {
        const {tm, transform} = setup([T.key, far.key]);
        let max = 0;
        tm._outOfViewCache = {setMaxSize: (n: number) => { max = n; }} as any;
        tm._source = {maxzoom: 10, tileSize: 512} as any;
        tm._maxTileCacheZoomLevels = null;
        tm.updateCacheSize(transform);
        const without = max;
        tm._refineCover(transform, [T, far], () => [...kids(T), ...kids(far)]);
        tm.updateCacheSize(transform);
        expect(max).toBe(without + 32);
    });

    test('within the budget, nearest the camera first', () => {
        const fine = [...kids(T), ...kids(far)];
        const {tm, transform} = setup([T.key, far.key]);
        // 512² px × budget: room for one tile's 16 finer tiles beyond the two (256² texels each)
        tm.detailBudget = (2 + 15) * 256 * 256 / (512 * 512);
        // (the camera is over T: x 100 at z8)
        expect(keys(tm._refineCover(transform, [T, far], () => fine))).toEqual(keys([...kids(T), far]));
        // (far's finer tiles over the budget: not pending)
        expect(tm._detailPending).toBe(false);
    });
});
