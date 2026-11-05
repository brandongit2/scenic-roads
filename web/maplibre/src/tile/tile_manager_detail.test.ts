import {describe, test, expect} from 'vitest';
import {TileManager} from './tile_manager.ts';
import {OverscaledTileID} from './tile_id.ts';
import {MercatorTransform} from '../geo/projection/mercator_transform.ts';
import {LngLat} from '../geo/lng_lat.ts';

// Scenic Roads: a source with `detail` (TileManager._refineCover).
describe('TileManager._refineCover', () => {
    const T = new OverscaledTileID(8, 0, 8, 100, 100);
    const far = new OverscaledTileID(8, 0, 8, 120, 100);
    const kids = (id: OverscaledTileID) => id.children(22).flatMap(c => c.children(22));

    /** `stillIn`: the map's ms until the camera has been still for its refineDelay (Map._stillIn). */
    function setup(loaded: string[], {moving = false, stillIn = moving ? 300 : 0} = {}) {
        const tm = Object.create(TileManager.prototype) as TileManager;
        tm.detailBudget = 8; // (512² px × 8: room for all 32 finer tiles, 256² texels each)
        tm._source = {maxzoom: 10} as any;
        tm._detailTiles = 0;
        const woken = {n: 0};
        tm.map = {isMoving: () => moving, _stillIn: () => stillIn, _updateWhenStill: () => woken.n++, getPixelRatio: () => 1} as any;
        tm._inViewTiles = {getTileById: (key: string) => loaded.includes(key) ? {hasData: () => true, state: 'loaded'} : undefined} as any;
        const transform = new MercatorTransform();
        transform.resize(512, 512);
        transform.setCenter(new LngLat(-39.4, 39.4));
        transform.setZoom(8);
        return {tm, transform, woken};
    }
    const keys = (ids: OverscaledTileID[]) => ids.map(i => i.key).sort();

    test('a tile is refined only once it has data itself and the camera is still; pending till then', () => {
        const fine = [...kids(T), ...kids(far)];
        let {tm, transform, woken} = setup([]);
        expect(keys(tm._refineCover(transform, [T, far], () => fine))).toEqual(keys([T, far]));
        expect(tm._detailPending).toBe(true);
        ({tm, transform} = setup([T.key, far.key]));
        expect(keys(tm._refineCover(transform, [T, far], () => fine))).toEqual(keys(fine));
        expect(tm._detailPending).toBe(false);
        // the finer tiles' zooms beyond the screen's, for the drapes' sizes
        expect(tm._detailOffset.get(kids(T)[0].key)?.offset).toBe(2);
        expect(tm._detailTiles).toBe(32);
        // moving: the screen's own cover, the finer tiles in or not (no finer cover worked out);
        // pending while a tile of it has finer tiles, and the sources updated once still
        const loaded = [T.key, far.key, ...kids(T).map(k => k.key)];
        const noFineCover = () => { throw new Error('the finer cover while moving'); };
        ({tm, transform, woken} = setup(loaded, {moving: true}));
        expect(keys(tm._refineCover(transform, [T, far], noFineCover))).toEqual(keys([T, far]));
        expect(tm._detailPending).toBe(true);
        expect(tm._detailOffset.size).toBe(0);
        expect(woken.n).toBe(1);
        // stopped but not yet still for refineDelay: the same
        ({tm, transform, woken} = setup(loaded, {stillIn: 120}));
        expect(keys(tm._refineCover(transform, [T, far], noFineCover))).toEqual(keys([T, far]));
        expect(tm._detailPending).toBe(true);
        expect(woken.n).toBe(1);
        // (at the source's finest zoom: nothing to come)
        tm._source = {maxzoom: 8} as any;
        tm._refineCover(transform, [T, far], noFineCover);
        expect(tm._detailPending).toBe(false);
    });

    test('the out-of-view cache keeps the last rest\'s finer tiles', () => {
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
