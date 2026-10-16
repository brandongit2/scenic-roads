import {describe, expect, test} from 'vitest';
import {terrainUniformValues} from './terrain_program.ts';
import {Sky, fogDepth} from '../../style/sky.ts';
import {earthRadius} from '../../geo/lng_lat.ts';

// A camera 1000 m up, 2000 px from the map's centre, at 1 px a metre.
const tr = {cameraToCenterDistance: 2000, pixelsPerMeter: 1, getCameraAltitude: () => 1000};

describe('terrainUniformValues', () => {
    test('the distance fog: its range and opacity', () => {
        const sky = new Sky({'fog-color': '#102030'}, {});
        sky.fogRange = {start: 0.3, end: 0.9, opacity: 0.5};
        const uniformValues = terrainUniformValues(1.0, sky, tr);
        expect(uniformValues['u_ele_delta']).toBe(1.0);
        expect(uniformValues['u_fog']).toEqual([0.3, 0.9, 0.5]);
        expect(uniformValues['u_fog_color']).toEqual(sky.properties.get('fog-color'));
        expect(uniformValues['u_fog_depth']).toEqual(fogDepth(tr));
    });

    test('no fog without a sky, or with a transparent fog colour', () => {
        expect(terrainUniformValues(1.0, null, tr)['u_fog'][2]).toBe(0);
        const sky = new Sky({'fog-color': 'transparent'}, {});
        expect(terrainUniformValues(1.0, sky, tr)['u_fog'][2]).toBe(0);
    });

    test('an end before the start is kept after it', () => {
        const sky = new Sky({'fog-color': '#000'}, {});
        sky.fogRange = {start: 0.8, end: 0.2, opacity: 1};
        const [start, end] = terrainUniformValues(1.0, sky, tr)['u_fog'];
        expect(end).toBeGreaterThan(start);
    });
});

describe('fogDepth', () => {
    test('0 at the map\'s centre, 1 at the horizon', () => {
        const [a, b] = fogDepth(tr);
        const horizon = Math.sqrt(1000 * (2 * earthRadius + 1000));
        expect(2000 * a - b).toBeCloseTo(0, 9);
        expect(horizon * a - b).toBeCloseTo(1, 9);
    });
});
