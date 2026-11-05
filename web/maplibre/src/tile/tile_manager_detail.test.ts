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

    function setup(loaded: string[]) {
        const tm = Object.create(TileManager.prototype) as TileManager;
        tm.detailBudget = 8; // (512² px × 8: room for all 32 finer tiles, 256² texels each)
        tm.maxTilesADrape = 240;
        tm._source = {maxzoom: 10} as any;
        tm._detailTiles = 0;
        tm._detailOffset = new Map();
        tm._refineMemo = null;
        tm._tileVersion = 0;
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

    test('worked out again only when what it reads changes', () => {
        const fine = [...kids(T), ...kids(far)];
        const loaded = [T.key];
        const {tm, transform} = setup(loaded);
        let asked = 0;
        const fineCover = () => { asked++; return fine; };
        expect(keys(tm._refineCover(transform, [T, far], fineCover))).toEqual(keys([...kids(T), far]));
        const offsets = tm._detailOffset;
        // the same frame again (the camera moved, nothing else): the last answer, no finer cover asked
        transform.setCenter(new LngLat(-39.3, 39.4));
        expect(keys(tm._refineCover(transform, [T, far], fineCover))).toEqual(keys([...kids(T), far]));
        expect(asked).toBe(1);
        expect(tm._detailOffset).toBe(offsets);
        // a tile comes in (_tileVersion): chosen again over the finer cover already known
        loaded.push(far.key);
        tm._tileVersion++;
        expect(keys(tm._refineCover(transform, [T, far], fineCover))).toEqual(keys(fine));
        expect(asked).toBe(1);
        expect(tm._detailOffset).not.toBe(offsets);
        // the screen's own cover changes: the finer cover asked again
        expect(keys(tm._refineCover(transform, [T], fineCover))).toEqual(keys(kids(T)));
        expect(asked).toBe(2);
        // (the same finer tiles chosen again: the same offsets)
        const now = tm._detailOffset;
        tm._tileVersion++;
        tm._refineCover(transform, [T], fineCover);
        expect(tm._detailOffset).toBe(now);
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
