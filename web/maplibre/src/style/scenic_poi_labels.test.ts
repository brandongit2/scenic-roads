// Scenic Roads: the app's stops & sights names, every kind in one symbol layer (web/src/basemap.ts
// poiLabelLayer), give each feature what its kind's own layer gave it: text, colour, filter and
// placement order (the topmost kind's layer was placed first; the best-known first within a kind).
import {describe, expect, test} from 'vitest';
import {createExpression, featureFilter, type ExpressionSpecification, type FilterSpecification} from '@maplibre/maplibre-gl-style-spec';
import {POI_STYLE, poiLabelFilter, poiLabelLayer, poiTilesUrl, poiKinds} from '../../../src/basemap.ts';

const layer = poiLabelLayer() as any;
const globals = {zoom: 12};
const feature = (props: Record<string, unknown>) => ({type: 1 as const, properties: props, geometry: [], id: 1});
const value = (spec: unknown, props: Record<string, unknown>, type: any) => {
    const e = createExpression(spec as ExpressionSpecification, {type, 'property-type': 'data-driven', expression: {parameters: ['zoom', 'feature'], interpolated: false}} as any);
    if (e.result !== 'success') throw new Error(JSON.stringify(e.value));
    return e.value.evaluate(globals, feature(props) as any);
};
const keys = Object.keys(POI_STYLE);

describe('poiLabelLayer', () => {
    test('each kind its own colour', () => {
        for (const k of keys) for (const kind of poiKinds(k))
            expect(value(layer.paint['text-color'], {kind, name: 'x'}, 'color').toString()).toBe(value(POI_STYLE[k][1], {}, 'color').toString());
    });

    test('peaks with their elevation, the others main and sub', () => {
        const peak = value(layer.layout['text-field'], {kind: 'peak', name: 'mont blanc', ele: 4805.6}, 'formatted');
        expect(peak.toString()).toBe('Mont blanc\n4806 m');
        const fall = value(layer.layout['text-field'], {kind: 'waterfall', name: 'x', main: 'Ribbon', sub: 'Fall'}, 'formatted');
        expect(fall.toString()).toBe('Ribbon\nFall');
    });

    test('placed as the layers were: the last kind first, then by fame', () => {
        const key = (kind: string, fa: number) => value(layer.layout['symbol-sort-key'], {kind, fa, name: 'x'}, 'number');
        // every name of a later kind (its layer above) before every name of an earlier kind
        for (let i = 1; i < keys.length; i++)
            expect(key(poiKinds(keys[i])[0], 0)).toBeLessThan(key(poiKinds(keys[i - 1])[0], 9));
        expect(key('peak', 5)).toBeLessThan(key('peak', 2));
        expect(key('picnic_site', 1)).toBe(key('rest_area', 1));
    });

    test('filtered to the kinds on, each with its own filter, named ones only', () => {
        const f = featureFilter(['all', layer.filter, poiLabelFilter(['peak', 'rest'], (k) => (k === 'peak' ? ['>=', ['get', 'ele'], 3000] : null))] as FilterSpecification, 'layers[0].filter');
        const pass = (p: Record<string, unknown>) => f.filter(globals as any, feature(p) as any);
        expect(pass({kind: 'peak', name: 'a', ele: 3500})).toBe(true);
        expect(pass({kind: 'peak', name: 'a', ele: 2500})).toBe(false);
        expect(pass({kind: 'picnic_site', name: 'a'})).toBe(true);
        expect(pass({kind: 'waterfall', name: 'a'})).toBe(false);
        expect(pass({kind: 'peak', name: '', ele: 3500})).toBe(false);
    });

    test('its tiles name the kinds they hold, in one order', () => {
        expect(poiTilesUrl(3, ['rest', 'peak'])).toBe('lmk://pois/{z}/{x}/{y}?v=3&k=peak,rest');
    });
});
