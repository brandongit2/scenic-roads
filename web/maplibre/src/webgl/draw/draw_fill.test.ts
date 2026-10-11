import {describe, test, expect, vi} from 'vitest';
import {mat4} from 'gl-matrix';
import {OverscaledTileID} from '../../tile/tile_id.ts';
import {TileManager} from '../../tile/tile_manager.ts';
import {Tile} from '../../tile/tile.ts';
import {Painter} from '../../render/painter.ts';
import {createRenderContext} from '../../render/render_context.ts';
import {Program} from '../program.ts';
import {FillStyleLayer} from '../../style/style_layer/fill_style_layer.ts';
import {drawFill} from './draw_fill.ts';
import {prepareDrawLayerOpacity, drawLayerOpacity} from './draw_layer_opacity.ts';
import {FillBucket} from '../../data/bucket/fill_bucket.ts';
import {createIdentityMat4f32} from '../../util/util.ts';

import type {ProgramConfiguration, ProgramConfigurationSet} from '../../data/program_configuration.ts';
import type {Style} from '../../style/style.ts';
import type {ProjectionData} from '../../geo/projection/projection_data.ts';
import type {FillLayerSpecification, AllPaintProperties} from '@maplibre/maplibre-gl-style-spec';
import type {IReadonlyTransform} from '../../geo/transform_interface.ts';
import type {EvaluationParameters} from '../../style/evaluation_parameters.ts';
import type {Map} from '../../ui/map.ts';
import type {ZoomHistory} from '../../style/zoom_history.ts';

vi.mock(import('../../render/painter'));
vi.mock(import('../program'));
vi.mock(import('../../tile/tile_manager'));
vi.mock(import('../../tile/tile'));

vi.mock(import('../../data/bucket/symbol_bucket'), () => {
    return {
        SymbolBucket: vi.fn()
    };
});
vi.mock(import('../../symbol/projection'));
vi.mock(import('./draw_layer_opacity'), () => ({
    prepareDrawLayerOpacity: vi.fn(() => ({}) as any),
    drawLayerOpacity: vi.fn(),
}));

describe('drawFill', () => {
    test('should call programConfiguration.setConstantPatternPositions for transitioning fill-pattern', () => {

        const painterMock: Painter = constructMockPainter();
        const layer: FillStyleLayer = constructMockLayer();

        const programMock = new Program(null, null, null, null, null, null, null, null);
        (vi.mocked(painterMock.useProgram)).mockReturnValue(programMock);

        const mockTile = constructMockTile(layer);

        const tileManagerMock = new TileManager(null, null, null);
        (vi.mocked(tileManagerMock.getTile)).mockReturnValue(mockTile);
        tileManagerMock.map = {showCollisionBoxes: false} as any as Map;

        drawFill(painterMock, tileManagerMock, layer, [mockTile.tileID], painterMock.renderContext);

        // twice: first for fill, second for stroke
        expect(programMock.draw).toHaveBeenCalledTimes(2);

        const bucket: FillBucket = (mockTile.getBucket(layer) as any);
        const programConfiguration = bucket.programConfigurations.get(layer.id);

        expect(programConfiguration.setConstantPatternPositions).toHaveBeenCalled();
    });

    // Scenic Roads (second review 1): a lifted fill isn't clipped by its tiles' masks, so where a
    // retained parent and a loaded child overlap it takes the overlap stencil rasters use.
    test('a lifted fill draws a retained parent and a loaded child z-descending with the overlap stencil', async () => {
        const {Painter: ActualPainter} = await vi.importActual<typeof import('../../render/painter.ts')>('../../render/painter.ts');
        const painterMock: Painter = constructMockPainter();
        painterMock.context.gl = {GEQUAL: 0x0206, KEEP: 0x1E00, REPLACE: 0x1E01} as any;
        painterMock.nextStencilID = 1;
        painterMock.getStencilConfigForOverlapAndUpdateStencilID = ActualPainter.prototype.getStencilConfigForOverlapAndUpdateStencilID;

        const layer = new FillStyleLayer({
            id: 'lifted', source: 'empty-source', type: 'fill', layout: {},
            paint: {'fill-color': '#00f', 'fill-opacity': 0.5, 'fill-z-offset': 1500, 'fill-elevation-reference': 'sea'}
        } as any, {});
        layer.recalculate({zoom: 0, zoomHistory: {} as ZoomHistory} as EvaluationParameters, []);
        expect(layer.elevationReference()).toBe(2);

        const programMock = new Program(null, null, null, null, null, null, null, null);
        (vi.mocked(painterMock.useProgram)).mockReturnValue(programMock);
        const parent = constructMockTile(layer);
        const child = constructMockTile(layer);
        child.tileID = new OverscaledTileID(2, 0, 2, 0, 0);
        child.tileID.terrainRttPosMatrix32f = createIdentityMat4f32();

        const tileManagerMock = new TileManager(null, null, null);
        (vi.mocked(tileManagerMock.getTile)).mockImplementation((id) => id.key === child.tileID.key ? child : parent);
        tileManagerMock.map = {showCollisionBoxes: false} as any as Map;

        const coords = [parent.tileID, child.tileID];
        drawFill(painterMock, tileManagerMock, layer, coords, painterMock.renderContext);

        expect(coords).toEqual([parent.tileID, child.tileID]); // the caller's order kept
        expect(vi.mocked(painterMock.stencilModeForClipping)).not.toHaveBeenCalled();
        // fill then outline, each child (z2) first, with a higher GEQUAL ref that REPLACEs
        const calls = vi.mocked(programMock.draw).mock.calls;
        expect(calls).toHaveLength(4);
        const [childFill, parentFill, childOutline, parentOutline] = calls.map(c => c[3] as any);
        for (const s of [childFill, parentFill]) {
            expect(s.test.func).toBe(0x0206);
            expect(s.pass).toBe(0x1E01);
        }
        expect(childFill.ref).toBeGreaterThan(parentFill.ref);
        // one config for both passes: the parent's outline is kept off the child's fill too
        expect(childOutline).toBe(childFill);
        expect(parentOutline).toBe(parentFill);
        expect(calls[0][9]).toBe('lifted');
    });

    // Scenic Roads (third review B1): with fill-layer-opacity below 1 a lifted fill still takes the
    // overlap stencil, not the clipping masks' EQUAL tests (it draws no masks).
    test('a lifted fill with layer opacity takes the overlap stencil inside the layer-opacity pass', async () => {
        const {Painter: ActualPainter} = await vi.importActual<typeof import('../../render/painter.ts')>('../../render/painter.ts');
        const painterMock: Painter = constructMockPainter();
        painterMock.context.gl = {GEQUAL: 0x0206, KEEP: 0x1E00, REPLACE: 0x1E01} as any;
        painterMock.nextStencilID = 1;
        painterMock.getStencilConfigForOverlapAndUpdateStencilID = ActualPainter.prototype.getStencilConfigForOverlapAndUpdateStencilID;

        const layer = new FillStyleLayer({
            id: 'lifted', source: 'empty-source', type: 'fill', layout: {},
            paint: {'fill-color': '#00f', 'fill-z-offset': 1500, 'fill-elevation-reference': 'sea', 'fill-layer-opacity': 0.5}
        } as any, {});
        layer.recalculate({zoom: 0, zoomHistory: {} as ZoomHistory} as EvaluationParameters, []);

        const programMock = new Program(null, null, null, null, null, null, null, null);
        (vi.mocked(painterMock.useProgram)).mockReturnValue(programMock);
        const parent = constructMockTile(layer);
        const child = constructMockTile(layer);
        child.tileID = new OverscaledTileID(2, 0, 2, 0, 0);
        child.tileID.terrainRttPosMatrix32f = createIdentityMat4f32();
        const tileManagerMock = new TileManager(null, null, null);
        (vi.mocked(tileManagerMock.getTile)).mockImplementation((id) => id.key === child.tileID.key ? child : parent);
        tileManagerMock.map = {showCollisionBoxes: false} as any as Map;

        drawFill(painterMock, tileManagerMock, layer, [parent.tileID, child.tileID], painterMock.renderContext);

        expect(vi.mocked(prepareDrawLayerOpacity)).toHaveBeenCalledTimes(1);
        expect(vi.mocked(drawLayerOpacity)).toHaveBeenCalledTimes(1);
        expect(vi.mocked(painterMock.stencilModeForClipping)).not.toHaveBeenCalled();
        const stencils = vi.mocked(programMock.draw).mock.calls.map(c => c[3] as any);
        expect(stencils).toHaveLength(4);
        for (const s of stencils) expect(s.test.func).toBe(0x0206);
        expect(stencils[0].ref).toBeGreaterThan(stencils[1].ref);
    });

    function constructMockLayer(): FillStyleLayer {
        const layerSpec = {
            id: 'mock-layer',
            source: 'empty-source',
            type: 'fill',
            layout: {},
            'paint': {
                'fill-pattern': 'pattern0'
            }
        } as FillLayerSpecification;
        const layer = new FillStyleLayer(layerSpec, {});
        layer.getCrossfadeParameters = () => ({} as any);
        layer.recalculate({zoom: 0, zoomHistory: {} as ZoomHistory} as EvaluationParameters, []);

        // Important: this setup is on purpose -- to NOT match layerspec
        // 'fill-pattern': 'pattern0'
        // so tile.imageAtlas.patternPositions['pattern0'] would return nothing
        // mimicking the transitioning fill-pattern value
        // eslint-disable-next-line @typescript-eslint/no-unnecessary-type-assertion -- otherwise typescript fails with error TS2590: Expression produces a union type that is too complex to represent
        layer.getPaintProperty = (() => {
            return 'pattern1';
        }) as <K extends keyof AllPaintProperties>(name: K) => AllPaintProperties[K];

        return layer;
    }

    function constructMockPainter(): Painter {
        const painterMock = new Painter(null, null);
        painterMock.context = {
            gl: {},
            activeTexture: {
                set: () => {}
            }
        } as any;
        painterMock.transform = {
            pitch: 0,
            labelPlaneMatrix: mat4.create(),
            zoom: 0,
            angle: 0,
            getProjectionData(_canonical, fallback): ProjectionData {
                return {
                    mainMatrix: fallback,
                    tileMercatorCoords: [0, 0, 1, 1],
                    clippingPlane: [0, 0, 0, 0],
                    projectionTransition: 0.0,
                    fallbackMatrix: fallback,
                    clipAntimeridian: false,
                };
            },
        } as any as IReadonlyTransform;
        painterMock.renderContext = createRenderContext(painterMock.transform, undefined, null);
        painterMock.renderContext.currentPass = 'translucent';
        painterMock.options = {} as any;
        painterMock.style = {
            map: {
                projection: {}
            }
        } as any as Style;

        return painterMock;
    }

    function constructMockTile(layer: FillStyleLayer): Tile {
        const tileId = new OverscaledTileID(1, 0, 1, 0, 0);
        tileId.terrainRttPosMatrix32f = createIdentityMat4f32();

        const tile = new Tile(tileId, 256);
        tile.tileID = tileId;

        // Important: this setup is on purpose -- to NOT match layerspec
        // 'fill-pattern': 'pattern0'
        // so tile.imageAtlas.patternPositions['pattern0'] would return nothing
        // mimicking the transitioning fill-pattern value
        tile.imageAtlas = {
            patternPositions: {
                'pattern1': {}
            }
        } as any;
        tile.imageAtlasTexture = {
            bind: () => {}
        } as any;

        const bucketMock = constructMockBucket(layer);

        (vi.mocked(tile.getBucket)).mockReturnValue(bucketMock);
        (vi.mocked(tile.patternsLoaded)).mockReturnValue(true);
        return tile;
    }

    function constructMockBucket(layer: FillStyleLayer) {
        const bucketMock = new FillBucket({
            layers: [layer]
        } as any);

        const mockProgramConfigurations: ProgramConfigurationSet<FillStyleLayer> = {} as any;
        const mockProgramConfiguration: ProgramConfiguration = {} as any;
        mockProgramConfiguration.updatePaintBuffers = () => {};
        mockProgramConfiguration.setConstantPatternPositions = vi.fn();

        mockProgramConfigurations.get = () => mockProgramConfiguration;

        bucketMock.programConfigurations = mockProgramConfigurations;

        return bucketMock;
    }
});
