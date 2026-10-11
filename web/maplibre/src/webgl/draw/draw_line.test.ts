// Scenic Roads (second review 1): a lifted line isn't clipped by its tiles' masks, so where a retained
// parent and a loaded child overlap it takes the overlap stencil rasters use.
import {describe, test, expect, vi} from 'vitest';
import {mat4} from 'gl-matrix';
import {OverscaledTileID} from '../../tile/tile_id.ts';
import {TileManager} from '../../tile/tile_manager.ts';
import {Tile} from '../../tile/tile.ts';
import {Painter} from '../../render/painter.ts';
import {createRenderContext} from '../../render/render_context.ts';
import {Program} from '../program.ts';
import {LineStyleLayer} from '../../style/style_layer/line_style_layer.ts';
import {drawLine} from './draw_line.ts';
import {createIdentityMat4f32} from '../../util/util.ts';
import {LIFTED_DEFINES} from '../../render/lifted.ts';

import type {Style} from '../../style/style.ts';
import type {ProjectionData} from '../../geo/projection/projection_data.ts';
import type {IReadonlyTransform} from '../../geo/transform_interface.ts';
import type {EvaluationParameters} from '../../style/evaluation_parameters.ts';
import type {ZoomHistory} from '../../style/zoom_history.ts';
import type {Map} from '../../ui/map.ts';

vi.mock(import('../../render/painter'));
vi.mock(import('../program'));
vi.mock(import('../../tile/tile_manager'));
vi.mock(import('../../tile/tile'));
vi.mock(import('../../data/bucket/symbol_bucket'), () => ({SymbolBucket: vi.fn()}));
vi.mock(import('../../symbol/projection'));

function mockPainter(): Painter {
    const painter = new Painter(null, null);
    painter.context = {gl: {GEQUAL: 0x0206, KEEP: 0x1E00, REPLACE: 0x1E01, LEQUAL: 0x0203, TRIANGLES: 4}, program: {get: () => null}} as any;
    painter.transform = {
        pitch: 0, zoom: 1, angle: 0, labelPlaneMatrix: mat4.create(),
        getPixelScale: () => 1,
        getProjectionData(_canonical, fallback): ProjectionData {
            return {mainMatrix: fallback, tileMercatorCoords: [0, 0, 1, 1], clippingPlane: [0, 0, 0, 0],
                projectionTransition: 0, fallbackMatrix: fallback, clipAntimeridian: false};
        },
    } as any as IReadonlyTransform;
    painter.renderContext = createRenderContext(painter.transform, undefined, null);
    painter.renderContext.currentPass = 'translucent';
    painter.options = {} as any;
    painter.style = {map: {projection: {}}} as any as Style;
    painter.nextStencilID = 1;
    return painter;
}

function mockTile(layer: LineStyleLayer, id: OverscaledTileID): Tile {
    id.terrainRttPosMatrix32f = createIdentityMat4f32();
    const tile = new Tile(id, 512);
    tile.tileID = id;
    tile.tileSize = 512;
    const programConfiguration = {updatePaintBuffers: () => {}} as any;
    (vi.mocked(tile.getBucket)).mockReturnValue({programConfigurations: {get: () => programConfiguration}} as any);
    return tile;
}

describe('drawLine, lifted', () => {
    test('draws a retained parent and a loaded child z-descending with the overlap stencil, in the LIFTED program', async () => {
        const {Painter: ActualPainter} = await vi.importActual<typeof import('../../render/painter.ts')>('../../render/painter.ts');
        const painter = mockPainter();
        painter.getStencilConfigForOverlapAndUpdateStencilID = ActualPainter.prototype.getStencilConfigForOverlapAndUpdateStencilID;
        const program = new Program(null, null, null, null, null, null, null, null);
        (vi.mocked(painter.useProgram)).mockReturnValue(program);

        const layer = new LineStyleLayer({
            id: 'lifted', source: 's', type: 'line', layout: {},
            paint: {'line-color': '#f00', 'line-width': 4, 'line-z-offset': 250}
        } as any, {});
        layer.recalculate({zoom: 1, zoomHistory: {} as ZoomHistory} as EvaluationParameters, []);
        expect(layer.elevationReference()).toBe(1);

        const parent = mockTile(layer, new OverscaledTileID(1, 0, 1, 0, 0));
        const child = mockTile(layer, new OverscaledTileID(2, 0, 2, 0, 0));
        const tileManager = new TileManager(null, null, null);
        (vi.mocked(tileManager.getTile)).mockImplementation((id) => id.key === child.tileID.key ? child : parent);
        tileManager.map = {showCollisionBoxes: false} as any as Map;

        const coords = [parent.tileID, child.tileID];
        drawLine(painter, tileManager, layer, coords, painter.renderContext);

        expect(coords).toEqual([parent.tileID, child.tileID]); // the caller's order kept
        expect(vi.mocked(painter.stencilModeForClipping)).not.toHaveBeenCalled();
        expect(vi.mocked(painter.useProgram).mock.calls[0][3]).toBe(LIFTED_DEFINES);
        const calls = vi.mocked(program.draw).mock.calls;
        expect(calls).toHaveLength(2);
        const [childStencil, parentStencil] = calls.map(c => c[3] as any);
        expect(vi.mocked(tileManager.getTile).mock.calls.map(c => c[0].overscaledZ)).toEqual([2, 1]);
        for (const s of [childStencil, parentStencil]) {
            expect(s.test.func).toBe(0x0206);
            expect(s.pass).toBe(0x1E01);
        }
        expect(childStencil.ref).toBeGreaterThan(parentStencil.ref);
    });

    test('a flat line keeps upstream\'s clipping masks and program', () => {
        const painter = mockPainter();
        const program = new Program(null, null, null, null, null, null, null, null);
        (vi.mocked(painter.useProgram)).mockReturnValue(program);
        const layer = new LineStyleLayer({id: 'flat', source: 's', type: 'line', layout: {}, paint: {'line-width': 4}} as any, {});
        layer.recalculate({zoom: 1, zoomHistory: {} as ZoomHistory} as EvaluationParameters, []);
        const tile = mockTile(layer, new OverscaledTileID(1, 0, 1, 0, 0));
        const tileManager = new TileManager(null, null, null);
        (vi.mocked(tileManager.getTile)).mockReturnValue(tile);
        tileManager.map = {showCollisionBoxes: false} as any as Map;

        drawLine(painter, tileManager, layer, [tile.tileID], painter.renderContext);

        expect(vi.mocked(painter.stencilModeForClipping)).toHaveBeenCalledTimes(1);
        expect(vi.mocked(painter.useProgram).mock.calls[0][3]).toEqual([]);
    });
});
