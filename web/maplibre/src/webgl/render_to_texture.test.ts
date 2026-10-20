import {beforeEach, describe, test, expect, vi} from 'vitest';
import {RenderToTexture, drapes} from './render_to_texture.ts';
import {RTTFingerprint} from './rtt_fingerprint.ts';
import {createRenderContext} from '../render/render_context.ts';
import {Context} from '../webgl/context.ts';
import {ColorMode} from '../webgl/color_mode.ts';
import {Terrain} from '../render/terrain.ts';
import {Tile} from '../tile/tile.ts';
import {OverscaledTileID} from '../tile/tile_id.ts';
import {DepthMode} from '../webgl/depth_mode.ts';
import {createNullGL} from '../util/test/null_gl.ts';

import type {Style} from '../style/style.ts';
import type {Map} from '../ui/map.ts';
import type {TileManager} from '../tile/tile_manager.ts';
import type {TerrainSpecification} from '@maplibre/maplibre-gl-style-spec';
import type {FillStyleLayer} from '../style/style_layer/fill_style_layer.ts';
import type {RasterStyleLayer} from '../style/style_layer/raster_style_layer.ts';
import type {HillshadeStyleLayer} from '../style/style_layer/hillshade_style_layer.ts';
import type {BackgroundStyleLayer} from '../style/style_layer/background_style_layer.ts';
import type {SymbolStyleLayer} from '../style/style_layer/symbol_style_layer.ts';
import type {LineStyleLayer} from '../style/style_layer/line_style_layer.ts';
import type {Painter, RTTObject} from '../render/painter.ts';

describe('render to texture', () => {
    const gl = createNullGL();
    const backgroundLayer = {
        id: 'maine-background',
        type: 'background',
        source: 'maine',
        isHidden: () => false,
        isStateDependent: () => true
    } as any as BackgroundStyleLayer;
    const fillLayer = {
        id: 'maine-fill',
        type: 'fill',
        source: 'maine',
        isHidden: () => false,
        isStateDependent: () => true
    } as any as FillStyleLayer;
    const rasterLayer = {
        id: 'maine-raster',
        type: 'raster',
        source: 'maine',
        isHidden: () => false,
        isStateDependent: () => true
    } as any as RasterStyleLayer;
    const hillshadeLayer = {
        id: 'maine-hillshade',
        type: 'line',
        source: 'maine',
        isHidden: () => false,
        isStateDependent: () => true
    } as any as HillshadeStyleLayer;
    const lineLayer = {
        id: 'maine-line',
        type: 'line',
        source: 'maine',
        isHidden: () => false,
        isStateDependent: () => true
    } as any as LineStyleLayer;
    const symbolLayer = {
        id: 'maine-symbol',
        type: 'symbol',
        source: 'maine',
        layout: {
            'text-field': 'maine',
            'symbol-placement': 'line'
        },
        isHidden: () => false
    } as any as SymbolStyleLayer;

    let layersDrawn = 0;
    function createMockRTTObject(size: number) {
        return {
            texture: {texture: gl.createTexture(), bind: vi.fn(), generateMipmap: vi.fn()},
            size
        };
    }
    const painter = {
        layersDrawn: 0,
        context: new Context(gl),
        transform: {zoom: 10, calculatePosMatrix: () => {}, getProjectionData(_a) { return {}; }, cameraToCenterDistance: 1000, pixelsPerMeter: 1, getCameraAltitude: () => 1000},
        options: {moving: false},
        colorModeForRenderPass: () => ColorMode.alphaBlended,
        getDepthModeFor3D: () => DepthMode.disabled,
        useProgram: () => ({draw: () => { layersDrawn++; }}),
        renderTileClippingMasks: vi.fn(),
        renderLayer: vi.fn(),
        acquireRTT: (size: number) => createMockRTTObject(size),
        bindRTT: vi.fn(),
        mainFramebuffer: () => null,
        releaseRTT: vi.fn(),
        drawFunctions: {
            terrainDepth: vi.fn(),
        }
    } as any as Painter;
    const map = {painter} as Map;

    const tile = new Tile(new OverscaledTileID(3, 0, 2, 1, 2), 512);
    const tileManager = {
        _source: {minzoom: 0, maxzoom: 2},
        getTileByID: (_id) => tile,
        getVisibleCoordinates: () => [tile.tileID]
    } as TileManager;

    const style = {
        tileManagers: {
            'maine': {
                getVisibleCoordinates: () => [tile.tileID],
                getSource: () => ({}),
                getState: vi.fn().mockReturnValue({revision: 0})
            }
        },
        _order: ['maine-fill', 'maine-symbol'],
        _layers: {
            'maine-background': backgroundLayer,
            'maine-fill': fillLayer,
            'maine-raster': rasterLayer,
            'maine-hillshade': hillshadeLayer,
            'maine-line': lineLayer,
            'maine-symbol': symbolLayer
        },
        projection: {
            transitionState: 0,
        }
    } as any as Style;
    painter.style = style;
    map.style = style;
    style.map = map;

    const terrain = new Terrain(painter, tileManager, {} as any as TerrainSpecification);
    vi.spyOn(terrain.tileManager, 'getRenderableTiles').mockReturnValue([tile]);
    vi.spyOn(terrain.tileManager, 'getTerrainCoords').mockReturnValue({[tile.tileID.key]: tile.tileID});
    map.terrain = terrain;

    const rtt = new RenderToTexture(painter, terrain);
    rtt.prepareForRender(style, 0);
    painter.renderToTexture = rtt;

    beforeEach(() => {
        tile.rttObjects.length = 0;
        tile.rttFingerprint = {};
        style._order = ['maine-fill', 'maine-symbol'];
        vi.spyOn(terrain.tileManager, 'getRenderableTiles').mockReturnValue([tile]);
        vi.spyOn(terrain.tileManager, 'getTerrainCoords').mockReturnValue({[tile.tileID.key]: tile.tileID});
        painter.options.moving = false;
    });

    function visibleLayerIds() {
        return style._order.map((id) => drapes(style._layers[id]) ? `${id}^` : id).join();
    }

    function markRendered(renderedTile: Tile, obj: RTTObject, fingerprint: RTTFingerprint) {
        renderedTile.rttFingerprint = {maine: fingerprint};
        renderedTile.rttObjects[0] = obj;
    }

    test('should call painter with overlay tiles for terrain tile', () => {
        const renderLayerSpy = vi.spyOn(painter, 'renderLayer');
        rtt.prepareForRender(style, 0);

        const renderContext = createRenderContext(painter.transform, undefined, terrain);
        for (const layerId of style._order) {
            const layer = style._layers[layerId];
            rtt.renderLayer(layer, renderContext);
        }

        expect(renderLayerSpy).toHaveBeenCalledWith(
            expect.anything(),
            expect.anything(),
            expect.objectContaining({id: 'maine-fill'}),
            [tile.tileID],
            expect.anything()
        );
    });

    test('should clear tile cache when overlaid tiles change and return rtt object to painter pool', () => {
        rtt.prepareForRender(style, 0);

        const obj = {texture: {}, size: 512} as unknown as RTTObject;
        tile.rttFingerprint = {maine: new RTTFingerprint([tile.tileID], 0, 0, visibleLayerIds())};
        tile.rttObjects[0] = obj;

        const otherTileID = new OverscaledTileID(3, 0, 2, 2, 2);
        (vi.mocked(terrain.tileManager.getTerrainCoords)).mockReturnValueOnce({[tile.tileID.key]: otherTileID});
        (vi.mocked(painter.releaseRTT)).mockClear();

        rtt.prepareForRender(style, 0);

        expect(tile.getRTT(0)).toBeUndefined();
        expect(painter.releaseRTT).toHaveBeenCalledWith(obj);
    });

    test('pacedDrapes: a source tile\'s drop is done at the next frame, and none past movingRedraws while moving', () => {
        rtt.prepareForRender(style, 0);
        const obj = {texture: {}, size: 512} as unknown as RTTObject;
        markRendered(tile, obj, new RTTFingerprint([tile.tileID], 0, 0, visibleLayerIds()));
        terrain.tileManager._tiles = {[tile.tileID.key]: tile};
        terrain.tileManager.tileManager.map = {painter, triggerRepaint: vi.fn()} as any as Map;

        terrain.tileManager.releaseRTT(tile.tileID);
        expect(tile.getRTT(0)).toBe(obj);
        expect(terrain.tileManager._dirtyRTT.has(tile.tileID.key)).toBe(true);

        painter.options.moving = true;
        terrain.tileManager.movingRedraws = 0;
        rtt.prepareForRender(style, 0);
        expect(tile.getRTT(0)).toBe(obj);
        expect(rtt.needsFollowUpFrame).toBe(true);

        painter.options.moving = false;
        terrain.tileManager.movingRedraws = 2;
        rtt.prepareForRender(style, 0);
        expect(tile.getRTT(0)).toBeUndefined();
        expect(terrain.tileManager._dirtyRTT.size).toBe(0);
        terrain.tileManager._tiles = {};
    });

    test('should not clear tile cache if state remains same', () => {
        rtt.prepareForRender(style, 0);
        tile.rttFingerprint = {maine: new RTTFingerprint([tile.tileID], 0, 0, visibleLayerIds())};
        tile.rttObjects[0] = {texture: {}, size: 512} as unknown as RTTObject;

        rtt.prepareForRender(style, 0);

        expect(tile.getRTT(0)).toBeTruthy();
    });

    test('should render text after a line by not adding the text to the stack', () => {
        style._order = ['maine-fill', 'maine-symbol'];
        rtt.prepareForRender(style, 0);
        layersDrawn = 0;
        const renderContext = createRenderContext(painter.transform, undefined, terrain);
        expect(rtt._renderableLayerIds).toStrictEqual(['maine-fill', 'maine-symbol']);
        expect(rtt.renderLayer(fillLayer, renderContext)).toBeTruthy();
        expect(rtt.renderLayer(symbolLayer, renderContext)).toBeFalsy();
        expect(layersDrawn).toBe(1);
    });

    test('a texture missing a draw whose program was still linking is drawn again next frame', () => {
        style._order = ['maine-fill', 'maine-symbol'];
        rtt.prepareForRender(style, 0);
        const renderContext = createRenderContext(painter.transform, undefined, terrain);
        const spy = vi.spyOn(painter, 'renderLayer').mockImplementationOnce(() => { painter.context.pendingPrograms++; });
        painter.context.pendingPrograms = 0;
        terrain.tileManager._tiles = {[tile.tileID.key]: tile};
        rtt.renderLayer(fillLayer, renderContext);
        rtt.renderLayer(symbolLayer, renderContext);
        expect(terrain.tileManager._dirtyRTT.has(tile.tileID.key)).toBe(true);
        expect(rtt.needsFollowUpFrame).toBe(true);

        // (the next frame: dropped, drawn again with the program linked, kept)
        rtt.prepareForRender(style, 0);
        expect(tile.getRTT(0)).toBeUndefined();
        rtt.renderLayer(fillLayer, renderContext);
        rtt.renderLayer(symbolLayer, renderContext);
        expect(terrain.tileManager._dirtyRTT.size).toBe(0);
        expect(rtt.needsFollowUpFrame).toBe(false);
        expect(tile.getRTT(0)).toBeDefined();
        spy.mockRestore();
        painter.context.pendingPrograms = 0;
        terrain.tileManager._tiles = {};
    });

    test('render symbol between rtt layers', () => {
        style._order = ['maine-background', 'maine-fill', 'maine-raster', 'maine-hillshade', 'maine-symbol', 'maine-line', 'maine-symbol'];
        rtt.prepareForRender(style, 0);
        layersDrawn = 0;
        const renderContext = createRenderContext(painter.transform, undefined, terrain);
        expect(rtt._renderableLayerIds).toStrictEqual(['maine-background', 'maine-fill', 'maine-raster', 'maine-hillshade', 'maine-symbol', 'maine-line', 'maine-symbol']);
        expect(rtt.renderLayer(backgroundLayer, renderContext)).toBeTruthy();
        expect(rtt.renderLayer(fillLayer, renderContext)).toBeTruthy();
        expect(rtt.renderLayer(rasterLayer, renderContext)).toBeTruthy();
        expect(rtt.renderLayer(hillshadeLayer, renderContext)).toBeTruthy();
        expect(rtt.renderLayer(symbolLayer, renderContext)).toBeFalsy();
        expect(rtt.renderLayer(lineLayer, renderContext)).toBeTruthy();
        expect(rtt.renderLayer(symbolLayer, renderContext)).toBeFalsy();
        expect(layersDrawn).toBe(2);
    });

    test('render more symbols between rtt layers', () => {
        style._order = ['maine-background', 'maine-symbol', 'maine-hillshade', 'maine-symbol', 'maine-line', 'maine-symbol'];
        rtt.prepareForRender(style, 0);
        layersDrawn = 0;
        const renderContext = createRenderContext(painter.transform, undefined, terrain);
        expect(rtt._renderableLayerIds).toStrictEqual(['maine-background', 'maine-symbol', 'maine-hillshade', 'maine-symbol', 'maine-line', 'maine-symbol']);
        expect(rtt.renderLayer(backgroundLayer, renderContext)).toBeTruthy();
        expect(rtt.renderLayer(symbolLayer, renderContext)).toBeFalsy();
        expect(rtt.renderLayer(hillshadeLayer, renderContext)).toBeTruthy();
        expect(rtt.renderLayer(symbolLayer, renderContext)).toBeFalsy();
        expect(rtt.renderLayer(lineLayer, renderContext)).toBeTruthy();
        expect(rtt.renderLayer(symbolLayer, renderContext)).toBeFalsy();
        expect(layersDrawn).toBe(3);
    });

    test('should clear tile cache on source state update', () => {
        const state = {revision: 0};
        (vi.mocked(style.tileManagers['maine'].getState)).mockReturnValue(state as any);

        tile.rttObjects[0] = {texture: {}, size: 512} as unknown as RTTObject;
        tile.rttFingerprint = {maine: new RTTFingerprint([tile.tileID], 0, 0, visibleLayerIds())};

        rtt.prepareForRender(style, 0);
        expect(tile.getRTT(0)).toBeTruthy();

        state.revision = 1;
        rtt.prepareForRender(style, 0);
        expect(tile.getRTT(0)).toBeUndefined();
    });

    test('keeps the tile cache on a source state update when no draped layer of the source reads feature state (Scenic Roads)', () => {
        const state = {revision: 0};
        (vi.mocked(style.tileManagers['maine'].getState)).mockReturnValue(state as any);
        const reads = [backgroundLayer, fillLayer, rasterLayer, hillshadeLayer, lineLayer].map(l => [l, l.isStateDependent] as const);
        for (const [l] of reads) l.isStateDependent = () => false;
        try {
            tile.rttObjects[0] = {texture: {}, size: 512} as unknown as RTTObject;
            tile.rttFingerprint = {maine: new RTTFingerprint([tile.tileID], 0, 0, visibleLayerIds())};
            rtt.prepareForRender(style, 0);
            state.revision = 1;
            rtt.prepareForRender(style, 0);
            expect(tile.getRTT(0)).toBeTruthy();
        } finally {
            for (const [l, f] of reads) l.isStateDependent = f;
        }
    });

    test('cache miss acquires an rtt object from the painter and stores it on the tile', () => {
        style._order = ['maine-fill', 'maine-symbol'];
        rtt.prepareForRender(style, 0);

        const acquireSpy = vi.spyOn(painter, 'acquireRTT');
        acquireSpy.mockClear();

        const renderContext = createRenderContext(painter.transform, undefined, terrain);
        rtt.renderLayer(fillLayer, renderContext);
        rtt.renderLayer(symbolLayer, renderContext);

        expect(acquireSpy).toHaveBeenCalledWith(rtt.rttSize);
        expect(tile.getRTT(0)).toBeTruthy();
        expect(tile.getRTT(0).size).toBe(rtt.rttSize);
    });

    test('rebuilds the texture mip chain once the stack has been rendered into it', () => {
        style._order = ['maine-fill', 'maine-symbol'];
        rtt.prepareForRender(style, 0);

        const renderContext = createRenderContext(painter.transform, undefined, terrain);
        rtt.renderLayer(fillLayer, renderContext);
        rtt.renderLayer(symbolLayer, renderContext);

        expect(tile.getRTT(0).texture.generateMipmap).toHaveBeenCalledTimes(1);
    });

    test('terrain samples the texture with trilinear filtering', () => {
        style._order = ['maine-fill', 'maine-symbol'];
        rtt.prepareForRender(style, 0);

        const renderContext = createRenderContext(painter.transform, undefined, terrain);
        rtt.renderLayer(fillLayer, renderContext);
        rtt.renderLayer(symbolLayer, renderContext);

        expect(tile.getRTT(0).texture.bind).toHaveBeenCalledWith(gl.LINEAR, gl.CLAMP_TO_EDGE, gl.LINEAR_MIPMAP_LINEAR);
    });

    // Scenic Roads (third review B2): the drape pass's stencil IDs are its own, so an overflow there
    // (clearStencil resets the counter) doesn't drop a later lifted layer's GEQUAL references below
    // values an earlier lifted layer wrote in the main framebuffer.
    test('the drape pass starts each tile\'s stencil IDs at 1 and restores the main framebuffer\'s counter', () => {
        style._order = ['maine-fill', 'maine-symbol'];
        rtt.prepareForRender(style, 0);
        tile.rttObjects = [];
        painter.nextStencilID = 250;
        const seen: number[] = [];
        vi.mocked(painter.renderLayer).mockImplementation(() => {
            seen.push(painter.nextStencilID);
            painter.nextStencilID = 7; // the drape masks overflowed and clearStencil reset the counter
        });

        const renderContext = createRenderContext(painter.transform, undefined, terrain);
        rtt.renderLayer(fillLayer, renderContext);
        rtt.renderLayer(symbolLayer, renderContext);

        expect(seen).toEqual([1]);
        expect(painter.nextStencilID).toBe(250);
        vi.mocked(painter.renderLayer).mockReset();
    });

    test('cache hit reuses cached RTT and skips acquireRTT', () => {
        style._order = ['maine-fill', 'maine-symbol'];
        rtt.prepareForRender(style, 0);

        const cached = createMockRTTObject(rtt.rttSize) as unknown as RTTObject;
        tile.rttObjects[0] = cached;

        const acquireSpy = vi.spyOn(painter, 'acquireRTT');
        acquireSpy.mockClear();

        const renderContext = createRenderContext(painter.transform, undefined, terrain);
        rtt.renderLayer(fillLayer, renderContext);
        rtt.renderLayer(symbolLayer, renderContext);

        expect(acquireSpy).not.toHaveBeenCalled();
        expect(tile.getRTT(0)).toBe(cached);
    });

    test('renderLayer records the fingerprint including the render zoom', () => {
        (vi.mocked(style.tileManagers['maine'].getState)).mockReturnValue({revision: 0} as any);
        style._order = ['maine-fill', 'maine-symbol'];
        rtt.prepareForRender(style, 0);

        const renderContext = createRenderContext(painter.transform, undefined, terrain);
        rtt.renderLayer(fillLayer, renderContext);
        rtt.renderLayer(symbolLayer, renderContext);

        expect(new RTTFingerprint([tile.tileID], 0, 0, visibleLayerIds()).difference(tile.rttFingerprint['maine'])).toBe('none');
    });

    test('re-renders a texture rendered at another zoom once the zoom settles', () => {
        (vi.mocked(style.tileManagers['maine'].getState)).mockReturnValue({revision: 0} as any);
        const obj = {texture: {}, size: 512} as unknown as RTTObject;
        tile.rttFingerprint = {maine: new RTTFingerprint([tile.tileID], 0, 10, visibleLayerIds())};
        tile.rttObjects[0] = obj;

        rtt.prepareForRender(style, 11);
        // the zoom is still changing: keep the texture, but ask for the settling frame
        expect(tile.getRTT(0)).toBe(obj);
        expect(rtt.needsFollowUpFrame).toBe(true);

        rtt.prepareForRender(style, 11);
        // the zoom has settled: release, so this frame re-renders at the on-screen zoom
        expect(tile.getRTT(0)).toBeUndefined();
        expect(rtt.needsFollowUpFrame).toBe(false);
    });

    test('keeps a texture rendered at the current zoom', () => {
        (vi.mocked(style.tileManagers['maine'].getState)).mockReturnValue({revision: 0} as any);
        const obj = {texture: {}, size: 512} as unknown as RTTObject;
        tile.rttFingerprint = {maine: new RTTFingerprint([tile.tileID], 0, 10, visibleLayerIds())};
        tile.rttObjects[0] = obj;

        rtt.prepareForRender(style, 10);
        rtt.prepareForRender(style, 10);

        expect(tile.getRTT(0)).toBe(obj);
        expect(rtt.needsFollowUpFrame).toBe(false);
    });

    test('keeps a texture rendered at another zoom while the map is moving, and re-renders it once the map has stopped', () => {
        (vi.mocked(style.tileManagers['maine'].getState)).mockReturnValue({revision: 0} as any);
        const obj = {texture: {}, size: 512} as unknown as RTTObject;
        tile.rttFingerprint = {maine: new RTTFingerprint([tile.tileID], 0, 10, visibleLayerIds())};
        tile.rttObjects[0] = obj;
        painter.options.moving = true;

        rtt.prepareForRender(style, 10.01);
        rtt.prepareForRender(style, 10.01);
        expect(tile.getRTT(0)).toBe(obj);
        expect(rtt.needsFollowUpFrame).toBe(true);

        painter.options.moving = false;
        rtt.prepareForRender(style, 10.01);
        expect(tile.getRTT(0)).toBeUndefined();
    });

    test('re-renders at most one tile with new source tiles per frame, nearest to the camera first, and keeps the others until their turn', () => {
        (vi.mocked(style.tileManagers['maine'].getState)).mockReturnValue({revision: 0} as any);
        const farTile = new Tile(new OverscaledTileID(3, 0, 2, 2, 2), 512);
        const farthestTile = new Tile(new OverscaledTileID(3, 0, 2, 3, 2), 512);
        const tiles = [tile, farTile, farthestTile];
        vi.spyOn(terrain.tileManager, 'getRenderableTiles').mockReturnValue(tiles);
        (vi.mocked(terrain.tileManager.getTerrainCoords)).mockReturnValue(Object.fromEntries(tiles.map(t => [t.tileID.key, t.tileID])));
        const renderedBeforeSourceTilesArrived = new RTTFingerprint([], 0, 0, visibleLayerIds());
        const objects = tiles.map(t => {
            const obj = {texture: {}, size: 512} as unknown as RTTObject;
            markRendered(t, obj, renderedBeforeSourceTilesArrived);
            return obj;
        });
        (vi.mocked(painter.releaseRTT)).mockClear();

        rtt.prepareForRender(style, 0);
        expect(tile.getRTT(0)).toBeUndefined();
        expect(farTile.getRTT(0)).toBe(objects[1]);
        expect(farthestTile.getRTT(0)).toBe(objects[2]);
        expect(painter.releaseRTT).toHaveBeenCalledTimes(1);
        expect(rtt.needsFollowUpFrame).toBe(true);

        markRendered(tile, objects[0], new RTTFingerprint([tile.tileID], 0, 0, visibleLayerIds()));
        rtt.prepareForRender(style, 0);
        expect(tile.getRTT(0)).toBe(objects[0]);
        expect(farTile.getRTT(0)).toBeUndefined();
        expect(farthestTile.getRTT(0)).toBe(objects[2]);
        expect(rtt.needsFollowUpFrame).toBe(true);

        markRendered(farTile, objects[1], new RTTFingerprint([farTile.tileID], 0, 0, visibleLayerIds()));
        rtt.prepareForRender(style, 0);
        expect(farthestTile.getRTT(0)).toBeUndefined();
        expect(rtt.needsFollowUpFrame).toBe(false);

    });

    test('a tile without a texture is rendered regardless of how many stale tiles were re-rendered this frame', () => {
        (vi.mocked(style.tileManagers['maine'].getState)).mockReturnValue({revision: 0} as any);
        const newTile = new Tile(new OverscaledTileID(3, 0, 2, 2, 2), 512);
        vi.spyOn(terrain.tileManager, 'getRenderableTiles').mockReturnValue([tile, newTile]);
        (vi.mocked(terrain.tileManager.getTerrainCoords)).mockReturnValue({[tile.tileID.key]: tile.tileID, [newTile.tileID.key]: newTile.tileID});
        tile.rttFingerprint = {maine: new RTTFingerprint([], 0, 0, visibleLayerIds())};
        tile.rttObjects[0] = {texture: {}, size: 512} as unknown as RTTObject;

        rtt.prepareForRender(style, 0);
        const acquireSpy = vi.spyOn(painter, 'acquireRTT');
        acquireSpy.mockClear();
        const renderContext = createRenderContext(painter.transform, undefined, terrain);
        rtt.renderLayer(fillLayer, renderContext);
        rtt.renderLayer(symbolLayer, renderContext);

        expect(acquireSpy).toHaveBeenCalledTimes(2);
        expect(tile.getRTT(0)).toBeTruthy();
        expect(newTile.getRTT(0)).toBeTruthy();

    });

    test('a layer leaving its zoom range re-renders every tile in the same frame once the zoom has settled, even while the map is moving', () => {
        (vi.mocked(style.tileManagers['maine'].getState)).mockReturnValue({revision: 0} as any);
        const trackLayer = {...lineLayer, id: 'maine-track', isHidden: (zoom: number) => zoom < 12} as any as LineStyleLayer;
        style._layers['maine-track'] = trackLayer;
        style._order = ['maine-fill', 'maine-track', 'maine-symbol'];
        const farTile = new Tile(new OverscaledTileID(3, 0, 2, 2, 2), 512);
        vi.spyOn(terrain.tileManager, 'getRenderableTiles').mockReturnValue([tile, farTile]);
        (vi.mocked(terrain.tileManager.getTerrainCoords)).mockReturnValue({[tile.tileID.key]: tile.tileID, [farTile.tileID.key]: farTile.tileID});
        const objects = [tile, farTile].map(t => {
            const obj = {texture: {}, size: 512} as unknown as RTTObject;
            markRendered(t, obj, new RTTFingerprint([t.tileID], 0, 12, visibleLayerIds()));
            return obj;
        });
        painter.options.moving = true;

        rtt.prepareForRender(style, 11.98);
        expect(tile.getRTT(0)).toBe(objects[0]);
        expect(farTile.getRTT(0)).toBe(objects[1]);
        expect(rtt.needsFollowUpFrame).toBe(true);

        rtt.prepareForRender(style, 11.98);
        expect(tile.getRTT(0)).toBeUndefined();
        expect(farTile.getRTT(0)).toBeUndefined();
        expect(rtt.needsFollowUpFrame).toBe(false);
        delete style._layers['maine-track'];
    });

    test('a source data change releases immediately even while the zoom is changing', () => {
        (vi.mocked(style.tileManagers['maine'].getState)).mockReturnValue({revision: 1} as any);
        const obj = {texture: {}, size: 512} as unknown as RTTObject;
        tile.rttFingerprint = {maine: new RTTFingerprint([tile.tileID], 0, 4, visibleLayerIds())};
        tile.rttObjects[0] = obj;

        rtt.prepareForRender(style, 5);

        expect(tile.getRTT(0)).toBeUndefined();
        expect(rtt.needsFollowUpFrame).toBe(false);
    });

    test('prepare only queries sources rendered to texture', () => {
        const tileManager = () => ({
            getVisibleCoordinates: vi.fn().mockReturnValue([tile.tileID]),
            getSource: vi.fn().mockReturnValue({}),
            getState: vi.fn().mockReturnValue({revision: 0})
        });
        const maineTileManager = tileManager();
        const terrainTileManager = tileManager();
        const symbolTileManager = tileManager();
        const testStyle = {
            ...style,
            terrain: {source: 'terrainSource'},
            tileManagers: {
                maine: maineTileManager,
                terrainSource: terrainTileManager,
                symbols: symbolTileManager
            },
            _order: ['maine-fill', 'symbols'],
            _layers: {
                'maine-fill': fillLayer,
                symbols: {...symbolLayer, id: 'symbols', source: 'symbols'}
            }
        } as any as Style;

        (vi.mocked(terrain.tileManager.getTerrainCoords)).mockClear();
        rtt.prepareForRender(testStyle, 0);

        expect(maineTileManager.getVisibleCoordinates).toHaveBeenCalledTimes(1);
        expect(terrainTileManager.getVisibleCoordinates).not.toHaveBeenCalled();
        expect(symbolTileManager.getVisibleCoordinates).not.toHaveBeenCalled();
        expect(terrain.tileManager.getTerrainCoords).toHaveBeenCalledTimes(1);
        expect(Object.keys(rtt._coordsAscending)).toStrictEqual(['maine']);
    });

    describe('lifted layers and the textures (Scenic Roads)', () => {
        const lifted = {id: 'lifted-line', type: 'line', source: 'lifted', isHidden: () => false, elevationReference: () => 1} as any;
        const lowered = {id: 'lowered-line', type: 'line', source: 'maine', isHidden: () => false, isStateDependent: () => true, level: 0, elevationReference() { return this.level; }} as any;

        beforeEach(() => {
            style._layers['lifted-line'] = lifted;
            style._layers['lowered-line'] = lowered;
            (style.tileManagers as any).lifted = {getVisibleCoordinates: () => [tile.tileID], getSource: () => ({}), getState: () => ({revision: 0})};
        });

        test('a source drawn only by lifted layers doesn\'t redraw the textures every frame', () => {
            style._order = ['maine-fill', 'lifted-line'];
            rtt.prepareForRender(style, 0);
            const renderContext = createRenderContext(painter.transform, undefined, terrain);
            for (const id of style._order) rtt.renderLayer(style._layers[id], renderContext);
            expect(tile.getRTT(0)).toBeTruthy();
            rtt.prepareForRender(style, 0);
            expect(tile.getRTT(0)).toBeTruthy();
            expect(Object.keys(rtt._rttFingerprints)).not.toContain('lifted');
        });

        test('a layer lifted off the terrain draws the textures again', () => {
            style._order = ['maine-fill', 'lowered-line'];
            lowered.level = 0;
            rtt.prepareForRender(style, 0);
            const renderContext = createRenderContext(painter.transform, undefined, terrain);
            for (const id of style._order) rtt.renderLayer(style._layers[id], renderContext);
            rtt.prepareForRender(style, 0);
            expect(tile.getRTT(0)).toBeTruthy();
            lowered.level = 1;
            rtt.prepareForRender(style, 0);
            expect(tile.getRTT(0)).toBeUndefined();
        });
    });

    describe('custom layers draped (Scenic Roads)', () => {
        const drapeLayer = {
            id: 'custom-drape',
            type: 'custom',
            implementation: {id: 'custom-drape', type: 'custom', render: vi.fn(), drape: vi.fn(), drapeRevision: 0},
            isHidden: () => false
        } as any;

        beforeEach(() => {
            style._layers['custom-drape'] = drapeLayer;
            style._order = ['maine-fill', 'custom-drape', 'maine-symbol'];
        });

        test('a custom layer with drape joins the stack, and draws in the main pass after its terrain', () => {
            const calls: {id: string; rtt: boolean; coords: number}[] = [];
            const renderLayerSpy = vi.spyOn(painter, 'renderLayer').mockImplementation((_p, _tm, layer, coords, ctx) => {
                calls.push({id: layer.id, rtt: ctx.isRenderingToTexture, coords: coords.length});
            });
            rtt.prepareForRender(style, 0);
            const renderContext = createRenderContext(painter.transform, undefined, terrain);
            const skipped = style._order.map((id) => rtt.renderLayer(style._layers[id], renderContext));
            // The fill and the custom layer stacked, the symbol drawn by the painter.
            expect(skipped).toEqual([true, true, false]);
            expect(calls).toEqual([
                {id: 'maine-fill', rtt: true, coords: 1},
                {id: 'custom-drape', rtt: true, coords: 1},
                {id: 'custom-drape', rtt: false, coords: 0},
            ]);
            renderLayerSpy.mockReset();
        });

        test('a new drape revision draws the terrain textures again', () => {
            rtt.prepareForRender(style, 0);
            const renderContext = createRenderContext(painter.transform, undefined, terrain);
            for (const id of style._order) rtt.renderLayer(style._layers[id], renderContext);
            expect(tile.getRTT(0)).toBeTruthy();
            rtt.prepareForRender(style, 0);
            expect(tile.getRTT(0)).toBeTruthy();
            drapeLayer.implementation.drapeRevision = 1;
            rtt.prepareForRender(style, 0);
            expect(tile.getRTT(0)).toBeUndefined();
        });
    });

    describe('a cover changing level while moving (Scenic Roads)', () => {
        const parent = new Tile(new OverscaledTileID(1, 0, 1, 0, 1), 512);
        const children = [[0, 2], [1, 2], [0, 3], [1, 3]].map(([x, y]) => new Tile(new OverscaledTileID(2, 0, 2, x, y), 512));
        const draw = () => {
            const renderContext = createRenderContext(painter.transform, undefined, terrain);
            rtt.renderLayer(fillLayer, renderContext);
            rtt.renderLayer(symbolLayer, renderContext);
        };
        const drawn = () => children.filter(c => c.getRTT(0)).length;
        const setUp = (moving: boolean) => {
            for (const t of [parent, ...children]) t.rttObjects.length = 0;
            parent.rttObjects[0] = {texture: {bind: vi.fn()}, size: 512} as unknown as RTTObject;
            terrain.tileManager._retired = {[parent.tileID.key]: parent};
            terrain.tileManager.tileManager.map = {painter, triggerRepaint: vi.fn()} as any as Map;
            vi.spyOn(terrain.tileManager, 'getRenderableTiles').mockReturnValue(children);
            (vi.mocked(terrain.tileManager.getTerrainCoords)).mockReturnValue(Object.fromEntries(children.map(c => [c.tileID.key, c.tileID])));
            painter.options.moving = moving;
            rtt._waitingSince = new globalThis.Map();
        };

        test('the new tiles\' textures are drawn over standInFrames + 1 frames, the parent\'s standing in', () => {
            setUp(true);
            rtt.prepareForRender(style, 0);
            // 4 waiting, 3 frames: 2 now, the others with the parent's quarter
            expect(Object.keys(rtt._standIns)).toHaveLength(2);
            const [key, standIn] = Object.entries(rtt._standIns)[0];
            const c = children.find(t => t.tileID.key === key);
            expect(standIn.tile).toBe(parent);
            expect(standIn.rect).toEqual([c.tileID.canonical.x / 2, (c.tileID.canonical.y - 2) / 2, 0.5, 0.5]);
            expect(rtt.needsFollowUpFrame).toBe(true);
            draw();
            expect(drawn()).toBe(2);
            expect(rtt.getTextureRect(c)).toEqual(standIn.rect);
            expect(rtt.getTexture(c)).toBe(parent.getRTT(0).texture);
            expect(rtt.getTextureRect(children.find(t => t.getRTT(0)))).toEqual([0, 0, 1, 1]);
            rtt.prepareForRender(style, 0);
            draw();
            expect(drawn()).toBe(3);
            expect(terrain.tileManager._retired[parent.tileID.key]).toBe(parent);
            rtt.prepareForRender(style, 0);
            draw();
            expect(drawn()).toBe(4);
            // none waits: the parent's textures released
            rtt.prepareForRender(style, 0);
            expect(Object.keys(rtt._standIns)).toHaveLength(0);
            expect(terrain.tileManager._retired).toEqual({});
            expect(parent.rttObjects).toHaveLength(0);
        });

        test('at rest every new tile is drawn at once and the stand-ins released', () => {
            setUp(false);
            rtt.prepareForRender(style, 0);
            expect(Object.keys(rtt._standIns)).toHaveLength(0);
            expect(terrain.tileManager._retired).toEqual({});
            draw();
            expect(drawn()).toBe(4);
        });

        test('standInFrames 0: drawn at once while moving too', () => {
            setUp(true);
            terrain.tileManager.standInFrames = 0;
            rtt.prepareForRender(style, 0);
            draw();
            expect(drawn()).toBe(4);
            terrain.tileManager.standInFrames = 2;
        });
    });
});
