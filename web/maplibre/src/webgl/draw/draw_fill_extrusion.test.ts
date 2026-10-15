import {describe, test, expect, vi} from 'vitest';
import {drawFillExtrusion} from './draw_fill_extrusion.ts';
import {FillExtrusionStyleLayer} from '../../style/style_layer/fill_extrusion_style_layer.ts';
import {OverscaledTileID} from '../../tile/tile_id.ts';

import type {Painter} from '../../render/painter.ts';
import type {TileManager} from '../../tile/tile_manager.ts';
import type {RenderContext} from '../../render/render_context.ts';
import type {EvaluationParameters} from '../../style/evaluation_parameters.ts';
import type {ZoomHistory} from '../../style/zoom_history.ts';

// Scenic Roads: `scenic:depth-bias` (a highlight drawn over the extrusions it copies wins over them).
function setup(metadata?: unknown) {
    const calls: string[] = [];
    const gl = {
        LEQUAL: 0x0203, POLYGON_OFFSET_FILL: 0x8037,
        enable: vi.fn((c: number) => calls.push(`enable ${c}`)),
        disable: vi.fn((c: number) => calls.push(`disable ${c}`)),
        polygonOffset: vi.fn((f: number, u: number) => calls.push(`offset ${f} ${u}`)),
    };
    const painter = {
        context: {gl},
        colorModeForRenderPass: vi.fn(),
        stencilModeFor3D: vi.fn(),
        transform: {},
    } as unknown as Painter;
    // (no bucket in the tile: nothing drawn, the state around the draw is what's tested)
    const tileManager = {getTile: vi.fn(() => ({getBucket: () => null}))} as unknown as TileManager;
    const layer = new FillExtrusionStyleLayer({
        id: 'highlight', type: 'fill-extrusion', source: 's', ...(metadata ? {metadata} : {}),
        paint: {'fill-extrusion-opacity': 0.85},
    } as any, {});
    layer.recalculate({zoom: 15, zoomHistory: {} as ZoomHistory} as EvaluationParameters, []);
    const draw = () => drawFillExtrusion(painter, tileManager, layer, [new OverscaledTileID(14, 0, 14, 1, 1)], {currentPass: 'translucent', depthRangeFor3D: [0, 1]} as unknown as RenderContext);
    return {calls, draw, tileManager};
}

describe('drawFillExtrusion depth bias', () => {
    test('a layer with scenic:depth-bias is drawn with a polygon offset toward the camera, reset after', () => {
        const {calls, draw, tileManager} = setup({'scenic:depth-bias': 4});
        draw();
        expect(tileManager.getTile).toHaveBeenCalledTimes(2);
        expect(calls).toEqual([`enable ${0x8037}`, 'offset -4 -4', 'offset 0 0', `disable ${0x8037}`]);
    });

    test('a layer without it touches no polygon offset', () => {
        const {calls, draw, tileManager} = setup();
        draw();
        expect(tileManager.getTile).toHaveBeenCalledTimes(2);
        expect(calls).toEqual([]);
    });

    test('a bias that is not a positive number is ignored', () => {
        const {calls, draw} = setup({'scenic:depth-bias': 'x'});
        draw();
        expect(calls).toEqual([]);
    });
});
