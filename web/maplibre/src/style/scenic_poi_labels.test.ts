// Scenic Roads: the app's stops & sights come from one source (web/src/basemap.ts POI_SOURCE), each
// kind's names still a layer of their own over it, filtered to the kind (poiLabelLayer).
import {describe, expect, test} from 'vitest';
import {featureFilter, type FilterSpecification} from '@maplibre/maplibre-gl-style-spec';
import {POI_SOURCE, POI_STYLE, poiLabelLayer, poiTilesUrl, poiKinds} from '../../../src/basemap.ts';

const globals = {zoom: 12};
const feature = (props: Record<string, unknown>) => ({type: 1 as const, properties: props, geometry: [], id: 1});
const keys = Object.keys(POI_STYLE);

describe('poiLabelLayer', () => {
    test('a layer a kind over the one source, its colour, its kind\'s named features only', () => {
        for (const k of keys) {
            const layer = poiLabelLayer(k) as any;
            expect(layer.id).toBe(`poi-${k}-label`);
            expect(layer.source).toBe(POI_SOURCE);
            expect(layer.paint['text-color']).toBe(POI_STYLE[k][1]);
            const f = featureFilter(layer.filter as FilterSpecification, 'layers[0].filter');
            for (const other of keys) for (const kind of poiKinds(other))
                expect(f.filter(globals as any, feature({kind, name: 'a'}) as any)).toBe(other === k);
            expect(f.filter(globals as any, feature({kind: poiKinds(k)[0], name: ''}) as any)).toBe(false);
        }
    });

    test('its tiles name the kinds they hold, in one order', () => {
        expect(poiTilesUrl(3, ['rest', 'peak'])).toBe('lmk://pois/{z}/{x}/{y}?v=3&k=peak,rest');
    });
});
