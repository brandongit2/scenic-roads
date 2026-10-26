import {describe, test, expect} from 'vitest';
import {createStyleLayer} from '../create_style_layer.ts';
import {symbolHeightScale} from '../style_spec_extensions.ts';
import {validateStyle} from '../validate_style.ts';

import type {LayerSpecification} from '@maplibre/maplibre-gl-style-spec';

// Scenic Roads: the elevation reference and height anchor `surface` (style_spec_extensions.ts).
describe('the elevation reference surface', () => {
    const layer = (type: 'fill' | 'line' | 'raster', reference?: string, offset?: number) => {
        const paint: Record<string, unknown> = {};
        if (reference) paint[`${type}-elevation-reference`] = reference;
        if (offset !== undefined) paint[`${type}-z-offset`] = offset;
        const l = createStyleLayer({id: type, type, source: 's', ...(type === 'raster' ? {} : {'source-layer': 'l'}), paint} as LayerSpecification, {}) as any;
        l.recalculate({zoom: 0, zoomHistory: {}} as any, undefined);
        return l.elevationReference();
    };

    test.each(['fill', 'line', 'raster'] as const)('%s: surface is 3, with or without an offset; ground and sea unchanged', (type) => {
        expect(layer(type, 'surface')).toBe(3);
        expect(layer(type, 'surface', 1)).toBe(3);
        expect(layer(type, 'sea')).toBe(2);
        expect(layer(type, undefined, 5)).toBe(1);
        expect(layer(type)).toBe(0);
    });

    test('the spec takes surface for the references and the anchor', () => {
        const errors = validateStyle({
            version: 8,
            sources: {s: {type: 'geojson', data: {type: 'FeatureCollection', features: []}}},
            layers: [
                {id: 'f', type: 'fill', source: 's', paint: {'fill-elevation-reference': 'surface'}},
                {id: 'l', type: 'line', source: 's', paint: {'line-elevation-reference': 'surface'}},
                {id: 't', type: 'symbol', source: 's', layout: {'symbol-height-anchor': 'surface'}},
            ],
        } as any);
        expect(errors).toEqual([]);
    });

    test('the anchor surface doesn\'t scale the offset (only sea does)', () => {
        expect(symbolHeightScale('surface', 3)).toBe(1);
        expect(symbolHeightScale('sea', 3)).toBe(3);
    });
});
