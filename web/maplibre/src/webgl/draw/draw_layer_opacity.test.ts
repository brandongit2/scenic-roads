// Scenic Roads (third review B2): the layer-opacity scratch FBO's stencil IDs are its own, so an
// overflow there doesn't reset the composite target's counter below values already written there.
import {describe, test, expect, vi} from 'vitest';
import {Context} from '../context.ts';
import {ColorMode} from '../color_mode.ts';
import {createNullGL} from '../../util/test/null_gl.ts';
import {prepareDrawLayerOpacity, drawLayerOpacity} from './draw_layer_opacity.ts';

import type {Painter} from '../../render/painter.ts';

describe('layer opacity and the stencil IDs', () => {
    test('the scratch FBO starts its IDs at 1 and the composite target\'s counter is restored', () => {
        const context = new Context(createNullGL());
        context.viewport.set([0, 0, 64, 32]);
        let atMasks = 0;
        const painter = {
            context,
            nextStencilID: 250,
            renderTileClippingMasks: vi.fn(() => {
                atMasks = painter.nextStencilID;
                painter.nextStencilID = 9; // the masks overflowed and clearStencil reset the counter
            }),
            useProgram: () => ({draw: vi.fn()}),
            colorModeForRenderPass: () => ColorMode.alphaBlended,
            transform: {zoom: 0},
        } as any as Painter;
        const layer = {id: 'l', paint: {}} as any;

        const results = prepareDrawLayerOpacity(painter, layer, []);
        expect(atMasks).toBe(1);
        drawLayerOpacity(painter, 0.5, results, layer);
        expect(painter.nextStencilID).toBe(250);
        expect(painter.currentStencilSource).toBeUndefined();
    });
});
