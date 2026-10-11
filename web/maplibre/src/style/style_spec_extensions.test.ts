import {describe, test, expect} from 'vitest';
import {createStyleLayer} from './create_style_layer.ts';
import {EvaluationParameters} from './evaluation_parameters.ts';
import {validateStyle} from './validate_style.ts';
import {drapes} from '../webgl/render_to_texture.ts';
import {styleSpec} from './style_spec_extensions.ts';

import type {LineStyleLayer} from './style_layer/line_style_layer.ts';
import type {FillStyleLayer} from './style_layer/fill_style_layer.ts';

// Scenic Roads: elevated lines and fills (style_spec_extensions.ts).
describe('elevated lines and fills', () => {
    const layer = (type: 'line' | 'fill', paint: Record<string, unknown>) => {
        const l = createStyleLayer({id: type, type, source: 's', paint} as any, {}) as LineStyleLayer | FillStyleLayer;
        l.recalculate(new EvaluationParameters(10), []);
        return l;
    };

    test('the spec knows the properties', () => {
        expect(styleSpec.paint_line['line-z-offset']).toBeDefined();
        expect(styleSpec.paint_fill['fill-elevation-reference']).toBeDefined();
        const errors = validateStyle({
            version: 8,
            sources: {s: {type: 'geojson', data: {type: 'FeatureCollection', features: []}}},
            layers: [
                {id: 'l', type: 'line', source: 's', paint: {'line-z-offset': ['get', 'z'], 'line-elevation-reference': 'sea'}},
                {id: 'f', type: 'fill', source: 's', paint: {'fill-z-offset': 447, 'fill-elevation-reference': 'sea'}},
            ],
        } as any);
        expect(errors).toEqual([]);
        expect(validateStyle({
            version: 8,
            sources: {s: {type: 'geojson', data: {type: 'FeatureCollection', features: []}}},
            layers: [{id: 'l', type: 'line', source: 's', paint: {'line-elevation-reference': 'moon'}}],
        } as any).length).toBe(1);
    });

    for (const type of ['line', 'fill'] as const) {
        test(`${type}: draped unless lifted`, () => {
            const plain = layer(type, {});
            expect(plain.elevationReference()).toBe(0);
            expect(plain.is3D()).toBe(false);
            expect(drapes(plain)).toBe(true);

            const zero = layer(type, {[`${type}-z-offset`]: 0});
            expect(zero.elevationReference()).toBe(0);

            const ground = layer(type, {[`${type}-z-offset`]: ['get', 'z']});
            expect(ground.elevationReference()).toBe(1);
            expect(ground.is3D()).toBe(true);
            expect(drapes(ground)).toBe(false);

            const sea = layer(type, {[`${type}-elevation-reference`]: 'sea'});
            expect(sea.elevationReference()).toBe(2);
            expect(drapes(sea)).toBe(false);
        });
    }
});
