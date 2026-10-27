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

    function setup(loaded: string[], moving = false) {
        const tm = Object.create(TileManager.prototype) as TileManager;
        tm.detailBudget = 8; // (512² px × 8: room for all 32 finer tiles, 256² texels each)
        tm.refineDelay = 0;
        tm._stillSince = null;
        tm._refineTimer = null;
        tm._detailKids = null;
        tm.map = {isMoving: () => moving, getPixelRatio: () => 1, triggerRepaint() {}, _update() {}} as any;
        tm._inViewTiles = {getTileById: (key: string) => loaded.includes(key) ? {hasData: () => true, state: 'loaded'} : undefined} as any;
        const transform = new MercatorTransform();
        transform.resize(512, 512);
        transform.setCenter(new LngLat(-39.4, 39.4));
        transform.setZoom(8);
        return {tm, transform};
    }
    const keys = (ids: OverscaledTileID[]) => ids.map(i => i.key).sort();

    test('a tile is refined only once it has data itself and the camera is still', () => {
        const fine = [...kids(T), ...kids(far)];
        let {tm, transform} = setup([]);
        expect(keys(tm._refineCover(transform, [T, far], () => fine))).toEqual(keys([T, far]));
        expect(tm.detailState.todayLoaded).toBe(false);
        ({tm, transform} = setup([T.key, far.key]));
        expect(keys(tm._refineCover(transform, [T, far], () => fine))).toEqual(keys(fine));
        expect(tm.detailState).toMatchObject({todayLoaded: true});
        // the finer tiles' zooms beyond the screen's, for the drapes' sizes
        expect(tm._detailOffset.get(kids(T)[0].key)?.offset).toBe(2);
        // moving: no new finer tiles, those loaded kept
        ({tm, transform} = setup([T.key, far.key, ...kids(T).map(k => k.key)], true));
        expect(keys(tm._refineCover(transform, [T, far], () => fine))).toEqual(keys([...kids(T), far]));
        if (tm._refineTimer) clearTimeout(tm._refineTimer);
    });

    test('within the budget, nearest the camera first', () => {
        const fine = [...kids(T), ...kids(far)];
        const {tm, transform} = setup([T.key, far.key]);
        // 512² px × budget: room for one tile's 16 finer tiles beyond the two (256² texels each)
        tm.detailBudget = (2 + 15) * 256 * 256 / (512 * 512);
        // (the camera is over T: x 100 at z8)
        expect(keys(tm._refineCover(transform, [T, far], () => fine))).toEqual(keys([...kids(T), far]));
        expect(tm.detailState.texels).toBe(17 * 256 * 256);
    });
});
