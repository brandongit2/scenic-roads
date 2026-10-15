import {describe, test, expect} from 'vitest';
import {tileNearestDistance, tileFootprintPx} from './covering_tiles.ts';
import {rttSizeForFootprint} from '../../tile/terrain_tile_manager.ts';

// Scenic Roads (task #193): the render-to-texture tiles' footprints and texture sizes.
describe('drape footprints', () => {
    test('the nearest point of a tile box', () => {
        const tile = {x: 1, y: 1, z: 2}; // [0.25, 0.5]²
        expect(tileNearestDistance({x: 0.3, y: 0.3, z: 0.1}, tile, 0, 0, 0.05)).toBeCloseTo(0.05);
        expect(tileNearestDistance({x: 0.3, y: 0.3, z: 0.02}, tile, 0, 0, 0.05)).toBe(0);
        expect(tileNearestDistance({x: 0.1, y: 0.3, z: 0}, tile, 0, 0, 0)).toBeCloseTo(0.15);
        expect(tileNearestDistance({x: 1.3, y: 0.3, z: 0}, tile, 1, 0, 0)).toBeCloseTo(0);
    });
    test('the footprint falls with distance and zoom', () => {
        const cam = {x: 0.3, y: 0.3, z: 0.1};
        const a = tileFootprintPx(cam, {x: 1, y: 1, z: 2}, 0, 0, 0, 1000);
        expect(a).toBeCloseTo(1000 / 4 / 0.1);
        expect(tileFootprintPx(cam, {x: 2, y: 2, z: 3}, 0, 0, 0, 1000)).toBeCloseTo(a / 2);
        expect(tileFootprintPx(cam, {x: 1, y: 1, z: 2}, 0, 0.1, 0.1, 1000)).toBe(Infinity);
    });
    test('the texture size: the power of two at or above, clamped', () => {
        expect(rttSizeForFootprint(0, 128, 1024)).toBe(128);
        expect(rttSizeForFootprint(129, 128, 1024)).toBe(256);
        expect(rttSizeForFootprint(512, 128, 1024)).toBe(512);
        expect(rttSizeForFootprint(513, 128, 1024)).toBe(1024);
        expect(rttSizeForFootprint(5000, 128, 1024)).toBe(1024);
        expect(rttSizeForFootprint(Infinity, 128, 1024)).toBe(1024);
    });
});
