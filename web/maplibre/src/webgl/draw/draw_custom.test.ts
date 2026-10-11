import {describe, test, expect, vi} from 'vitest';
import {OverscaledTileID} from '../../tile/tile_id.ts';
import {TileManager} from '../../tile/tile_manager.ts';
import {Tile} from '../../tile/tile.ts';
import {Painter} from '../../render/painter.ts';
import {createRenderContext} from '../../render/render_context.ts';
import {drawCustom, drapeMatrix} from './draw_custom.ts';
import {CustomStyleLayer} from '../../style/style_layer/custom_style_layer.ts';
import {MercatorTransform} from '../../geo/projection/mercator_transform.ts';
import {MercatorProjection} from '../../geo/projection/mercator_projection.ts';
import {expectToBeCloseToArray} from '../../util/test/util.ts';

import type {CustomRenderMethodInput} from '../../style/style_layer/custom_style_layer.ts';
import type {Map} from '../../ui/map.ts';

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

describe('drawCustom', () => {
    test('should return custom render method inputs', () => {
        // same transform setup as in transform.test.ts 'creates a transform', so matrices of transform should be the same
        const transform = new MercatorTransform({minZoom: 0, maxZoom: 22, minPitch: 0, maxPitch: 60, renderWorldCopies: true});
        transform.resize(500, 500);
        transform.setMinPitch(10);
        transform.setMaxPitch(10);
        const mockPainter = new Painter(null, null);
        mockPainter.style = {
            projection: new MercatorProjection(),
        } as any;
        mockPainter.transform = transform;
        const renderContext = createRenderContext(transform, mockPainter.style.projection, null);
        renderContext.currentPass = 'translucent';
        mockPainter.renderContext = renderContext;
        mockPainter.context = {
            gl: {},
            setColorMode: () => {},
            setStencilMode: () => {},
            setDepthMode: () => {},
            setDirty: () => {},
            bindFramebuffer: {
                set: () => {}
            }
        } as any;

        const tileId = new OverscaledTileID(1, 0, 1, 0, 0);
        const tile = new Tile(tileId, 256);
        tile.tileID = tileId;
        tile.imageAtlasTexture = {
            bind: () => { }
        } as any;
        const tileManagerMock = new TileManager(null, null, null);
        (vi.mocked(tileManagerMock.getTile)).mockReturnValue(tile);
        tileManagerMock.map = {showCollisionBoxes: false} as any as Map;

        let result: {
            gl: WebGLRenderingContext | WebGL2RenderingContext;
            args: CustomRenderMethodInput;
        };
        const mockLayer = new CustomStyleLayer({
            id: 'custom-layer',
            type: 'custom',
            render(gl, args) {
                result = {
                    gl,
                    args
                };
            },
        }, {});
        drawCustom(mockPainter, tileManagerMock, mockLayer, renderContext);
        expect(result.gl).toBeDefined();
        expect(result.args.farZ).toBeCloseTo(804.8028169246645, 6);
        expect(result.args.farZ).toBe(mockPainter.transform.farZ);
        expect(result.args.nearZ).toBe(mockPainter.transform.nearZ);
        expect(result.args.fov).toBe(mockPainter.transform.fov * Math.PI / 180);
        expect(result.args.modelViewProjectionMatrix).toEqual(mockPainter.transform.modelViewProjectionMatrix);
        expect(result.args.projectionMatrix).toEqual(mockPainter.transform.projectionMatrix);
        expectToBeCloseToArray(result.args.defaultProjectionData.tileMercatorCoords, [0, 0, 1, 1]);
        expect(result.args.defaultProjectionData.mainMatrix).toBeInstanceOf(Float64Array);
        expect(result.args.defaultProjectionData.fallbackMatrix).toBeInstanceOf(Float64Array);
        expect(result.args.defaultProjectionData.mainMatrix[0]).toBe(1536);
        expect(result.args.defaultProjectionData.mainMatrix[5]).toBe(-1512.6647086267515);
        expect(result.args.defaultProjectionData.mainMatrix[15]).toEqual(794.4539334827342);
        expect(result.args.defaultProjectionData.projectionTransition).toBe(0);
        expect(result.args.defaultProjectionData.mainMatrix).toEqual(result.args.defaultProjectionData.fallbackMatrix);
        const tileProjectionData = result.args.getProjectionData({
            tileID: {
                wrap: 1,
                canonical: {
                    z: 1,
                    x: 1,
                    y: 0,
                }
            }
        });
        expectToBeCloseToArray(tileProjectionData.tileMercatorCoords, [0.5, 0, 0.00006103515625, 0.00006103515625]);
        expect(tileProjectionData.mainMatrix).toBeInstanceOf(Float32Array);
        expect(tileProjectionData.fallbackMatrix).toBeInstanceOf(Float32Array);
        expect(tileProjectionData.mainMatrix[0]).toBeCloseTo(0.09375, 6);
        expect(tileProjectionData.mainMatrix[5]).toBeCloseTo(-0.09232572466135025, 6);
        expect(tileProjectionData.mainMatrix[15]).toBeCloseTo(794.4539184570312, 6);
        expect(tileProjectionData.projectionTransition).toBe(0);
        expect(tileProjectionData.mainMatrix).toEqual(tileProjectionData.fallbackMatrix);
    });
});

describe('drapeMatrix (Scenic Roads)', () => {
    const apply = (m: Float32Array, x: number, y: number) => [m[0] * x + m[4] * y + m[12], m[1] * x + m[5] * y + m[13]];
    const target = new OverscaledTileID(10, 0, 10, 100, 200);

    test('the terrain tile itself spans the texture', () => {
        const m = drapeMatrix(target, {z: 10, x: 100, y: 200});
        expectToBeCloseToArray(apply(m, 0, 0), [-1, 1]);
        expectToBeCloseToArray(apply(m, 8192, 8192), [1, -1]);
    });

    test('a child tile covers its quarter', () => {
        const m = drapeMatrix(target, {z: 11, x: 201, y: 400});
        expectToBeCloseToArray(apply(m, 0, 0), [0, 1]);
        expectToBeCloseToArray(apply(m, 8192, 8192), [1, 0]);
    });

    test('a parent tile reaches past the texture', () => {
        const m = drapeMatrix(target, {z: 9, x: 50, y: 100});
        expectToBeCloseToArray(apply(m, 0, 0), [-1, 1]);
        expectToBeCloseToArray(apply(m, 4096, 4096), [1, -1]);
        expectToBeCloseToArray(apply(m, 8192, 8192), [3, -3]);
    });

    test('a tile of another world copy', () => {
        expect(drapeMatrix(target, {z: 10, x: 100, y: 200, wrap: 1})).toBeNull();
        const wrapped = new OverscaledTileID(10, 1, 10, 100, 200);
        const m = drapeMatrix(wrapped, {z: 10, x: 100, y: 200, wrap: 1});
        expectToBeCloseToArray(apply(m, 0, 0), [-1, 1]);
    });

    test('a tile elsewhere: none', () => {
        expect(drapeMatrix(target, {z: 10, x: 101, y: 200})).toBeNull();
        expect(drapeMatrix(target, {z: 12, x: 0, y: 0})).toBeNull();
    });
});

describe('drawCustom reading the frame\'s depth (Scenic Roads)', () => {
    const setup = (readsSceneDepth: boolean, sceneTarget: any) => {
        const transform = new MercatorTransform({minZoom: 0, maxZoom: 22, minPitch: 0, maxPitch: 60, renderWorldCopies: true});
        transform.resize(500, 500);
        const painter = new Painter(null, null);
        painter.style = {projection: new MercatorProjection()} as any;
        painter.transform = transform;
        painter.sceneTarget = sceneTarget;
        vi.mocked(painter.mainFramebuffer).mockReturnValue(sceneTarget?.framebuffer ?? null);
        const renderContext = createRenderContext(transform, painter.style.projection, null);
        renderContext.currentPass = 'translucent';
        const bound: unknown[] = [];
        painter.context = {
            gl: {},
            setColorMode: () => {},
            setStencilMode: () => {},
            setDepthMode: () => {},
            setDirty: () => {},
            bindFramebuffer: {set: (fb: unknown) => bound.push(fb), get: () => null},
        } as any;
        let args: CustomRenderMethodInput;
        const layer = new CustomStyleLayer({id: 'c', type: 'custom', readsSceneDepth, render(_gl, a) { args = a; }}, {});
        drawCustom(painter, null, layer, renderContext);
        return {args, bound};
    };
    const target = {framebuffer: 'frame', colorOnly: 'colour', depthStencil: 'depth', width: 10, height: 20};

    test('a layer that reads it draws into the colour alone, the depth given', () => {
        const {args, bound} = setup(true, target);
        expect(args.sceneDepth).toEqual({texture: 'depth', width: 10, height: 20});
        expect(args.framebuffer).toBe('colour');
        expect(bound).toEqual(['colour', 'frame']);
    });

    test('a layer that doesn\'t: the frame\'s framebuffer, no depth', () => {
        const {args} = setup(false, target);
        expect(args.sceneDepth).toBeNull();
        expect(args.framebuffer).toBe('frame');
        expect(args.terrainDepth).toBeNull();
    });

    test('terrainDepth gives the depth texture\'s own size', () => {
        const transform = new MercatorTransform({minZoom: 0, maxZoom: 22, minPitch: 0, maxPitch: 60, renderWorldCopies: true});
        transform.resize(500, 500);
        const painter = new Painter(null, null);
        painter.style = {projection: new MercatorProjection(), map: {terrain: {getDepthTexture: () => ({texture: 'depth', width: 250, height: 125})}}} as any;
        painter.transform = transform;
        painter.terrainFacilitator = {renderTime: 7} as any;
        painter.width = 1000;
        painter.pixelRatio = 3;
        const renderContext = createRenderContext(transform, painter.style.projection, null);
        renderContext.currentPass = 'translucent';
        painter.context = {gl: {}, setColorMode: () => {}, setStencilMode: () => {}, setDepthMode: () => {}, setDirty: () => {}, bindFramebuffer: {set: () => {}, get: () => null}} as any;
        let args: CustomRenderMethodInput;
        drawCustom(painter, null, new CustomStyleLayer({id: 'c', type: 'custom', render(_gl, a) { args = a; }}, {}), renderContext);
        expect(args.terrainDepth).toMatchObject({texture: 'depth', width: 250, height: 125, revision: 7});
    });
});
