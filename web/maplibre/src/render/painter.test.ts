import {describe, beforeEach, test, expect, vi, afterEach} from 'vitest';
import {Painter} from './painter.ts';
import {MercatorTransform} from '../geo/projection/mercator_transform.ts';
import {GlobeProjection} from '../geo/projection/globe_projection.ts';
import {Style} from '../style/style.ts';
import {CustomStyleLayer} from '../style/style_layer/custom_style_layer.ts';
import {StubMap} from '../util/test/util.ts';
import {Texture} from '../webgl/texture.ts';
import {createNullGL} from '../util/test/null_gl.ts';
import {restoreNow, setNow} from '../util/time_control.ts';

describe('render', () => {
    let painter: Painter;
    let map: any;
    let style: Style;
    const renderOptions = {
        fadeDuration: 0,
        moving: false,
        rotating: false,
        showOverdrawInspector: false,
        showPadding: false,
        showTileBoundaries: false,
        zooming: false,
        anisotropicFilterPitch: 20,
    };

    beforeEach(() => {
        const gl = createNullGL();
        const transform = new MercatorTransform({minZoom: 0, maxZoom: 22, minPitch: 0, maxPitch: 60, renderWorldCopies: true});
        transform.resize(512, 512);
        painter = new Painter(gl, transform);
        map = new StubMap() as any;
        style = new Style(map);
        style._setProjectionInternal('mercator');
        style._updatePlacement(transform, false, 0, false);
    });

    test('must not fail with incompletely loaded style', () => {
        painter.render(style, renderOptions);

        expect(painter.renderContext.currentPass).toBe('translucent');
    });

    test('calls terrainDepth', () => {
        const terrainDepth = vi.spyOn(painter.drawFunctions, 'terrainDepth').mockImplementation(() => {});
        map.terrain = {tileManager: {anyTilesAfterTime: () => false}};

        painter.render(style, renderOptions);

        expect(terrainDepth).toHaveBeenCalled();
    });

    test('redraws cached terrain depth once after deferred invalidations', ({onTestFinished}) => {
        const terrainDepth = vi.spyOn(painter.drawFunctions, 'terrainDepth').mockImplementation(() => {}).mockClear();
        onTestFinished(() => terrainDepth.mockRestore());
        map.terrain = {tileManager: {anyTilesAfterTime: () => false}};

        painter.render(style, renderOptions);
        expect(terrainDepth).toHaveBeenCalledTimes(1);
        painter.render(style, renderOptions);
        expect(terrainDepth).toHaveBeenCalledTimes(1);

        painter.markTerrainDepthDirty();
        painter.markTerrainDepthDirty();
        expect(terrainDepth).toHaveBeenCalledTimes(1);

        painter.render(style, renderOptions);
        expect(terrainDepth).toHaveBeenCalledTimes(2);
        painter.render(style, renderOptions);
        expect(terrainDepth).toHaveBeenCalledTimes(2);
    });

    test('builds render context from the transform, globe projection and terrain', () => {
        const terrain = {tileManager: {anyTilesAfterTime: () => false}};
        map.terrain = terrain;
        style.projection = new GlobeProjection({type: 'vertical-perspective'}, {});
        vi.spyOn(painter.drawFunctions, 'terrainDepth').mockImplementation(() => {});
        vi.spyOn(painter.drawFunctions, 'atmosphere').mockImplementation(() => {});

        painter.render(style, renderOptions);

        expect(painter.renderContext.transform).toBe(painter.transform);
        expect(painter.renderContext.terrain).toBe(terrain);
        expect(painter.renderContext.projectionTransition).toBe(1);
        expect(painter.renderContext.isRenderingGlobe).toBe(true);
    });

    test('uses render context for depth and blending when drawing a custom layer', () => {
        painter.render(style, renderOptions);
        const renderContext = painter.renderContext;
        renderContext.depthRangeFor3D = [0.1, 0.8];
        const render = vi.fn((gl: WebGL2RenderingContext) => {
            expect(painter.context.depthRange.get()).toEqual([0.1, 0.8]);
            expect(painter.context.blend.get()).toBe(true);
            expect(painter.context.blendFunc.get()).toEqual([gl.ONE, gl.ONE_MINUS_SRC_ALPHA]);
        });
        const layer = new CustomStyleLayer({id: 'custom', type: 'custom', renderingMode: '3d', render}, {});

        painter.renderLayer(painter, null, layer, [], renderContext);

        expect(render).toHaveBeenCalledTimes(1);
    });

    describe('terrain render time', () => {
        beforeEach(() => {
            vi.spyOn(painter.drawFunctions, 'terrainDepth').mockImplementation(() => {});
            map.terrain = {tileManager: {anyTilesAfterTime: () => false}};
        });

        afterEach(() => {
            restoreNow();
        });

        test('stores terrain render time using the controlled clock', () => {
            setNow(1234);
            painter.render(style, renderOptions);

            expect(painter.terrainFacilitator.renderTime).toBe(1234);
        });
    });
});

describe('tile texture pool', () => {
    function createPainterWithPool() {
        const gl = createNullGL();
        const transform = new MercatorTransform({minZoom: 0, maxZoom: 22, minPitch: 0, maxPitch: 60, renderWorldCopies: true});
        return new Painter(gl, transform);
    }

    function createTexture(painter: Painter, size: number): Texture {
        const gl = painter.context.gl;
        const image = {width: size, height: size, data: new Uint8Array(size * size * 4)} as any;
        return new Texture(painter.context, image, gl.RGBA);
    }

    test('saveTileTexture caps pool size and destroys excess', () => {
        const painter = createPainterWithPool();
        const cap = Painter.MAX_TEXTURE_POOL_SIZE_PER_BUCKET;

        const textures: Texture[] = [];
        for (let i = 0; i < cap + 100; i++) {
            const tex = createTexture(painter, 256);
            textures.push(tex);
            painter.saveTileTexture(tex);
        }

        let reused = 0;
        while (painter.getTileTexture(256)) reused++;
        expect(reused).toBe(cap);

        const destroyed = textures.filter(t => t.texture === null).length;
        expect(destroyed).toBe(100);

        painter.destroy();
    });
});

describe('RTT pool', () => {
    let painter: Painter;

    beforeEach(() => {
        const gl = createNullGL();
        const transform = new MercatorTransform({minZoom: 0, maxZoom: 22, minPitch: 0, maxPitch: 60, renderWorldCopies: true});
        painter = new Painter(gl, transform);
    });

    afterEach(() => {
        painter.destroy();
    });

    test('acquireRTT creates on miss, recycles on hit', () => {
        const a = painter.acquireRTT(256);
        expect(a.size).toBe(256);
        expect(a.texture).toBeTruthy();

        painter.releaseRTT(a);
        expect(painter.acquireRTT(256)).toBe(a);
    });

    test('acquireRTT resizes pooled textures when sizes differ', () => {
        const a = painter.acquireRTT(256);
        const texture = a.texture;
        painter.releaseRTT(a);

        const b = painter.acquireRTT(512);
        expect(b).toBe(a);
        expect(b.size).toBe(512);
        expect(b.texture).toBe(texture);
        expect(b.texture.size).toEqual([512, 512]);
    });

    test('bindRTT lazily creates an FBO for the size and binds texture', () => {
        expect(painter._rttFbos.size).toBe(0);
        const obj = painter.acquireRTT(256);
        painter.bindRTT(obj);
        expect([...painter._rttFbos.keys()]).toEqual([256]);
        expect(painter._rttFbos.get(256).fbo.width).toBe(256);
    });

    test('bindRTT keeps one FBO and depth-stencil a size, reused, never resized', () => {
        const gl = painter.context.gl;
        const a = painter.acquireRTT(256);
        const b = painter.acquireRTT(512);
        for (let i = 0; i < 3; i++) {
            painter.bindRTT(a);
            painter.bindRTT(b);
        }
        expect([...painter._rttFbos.keys()].sort()).toEqual([256, 512]);
        expect(gl.createFramebuffer).toHaveBeenCalledTimes(2);
        expect(painter._rttFbos.get(512).fbo.width).toBe(512);
        expect(painter._rttFbos.get(256).fbo.width).toBe(256);
    });

    test('acquireRTT hands out a pooled texture of the size asked for first', () => {
        const small = painter.acquireRTT(256);
        const big = painter.acquireRTT(1024);
        painter.releaseRTT(small);
        painter.releaseRTT(big);
        expect(painter.acquireRTT(256)).toBe(small);
        expect(small.size).toBe(256);
    });

    test('clearRTTPool destroys the pooled textures and leaves the ones tiles hold', () => {
        const pooled = painter.acquireRTT(256);
        const held = painter.acquireRTT(256);
        vi.spyOn(pooled.texture, 'destroy');
        vi.spyOn(held.texture, 'destroy');
        painter.releaseRTT(pooled);

        painter.clearRTTPool();

        expect(pooled.texture.destroy).toHaveBeenCalledTimes(1);
        expect(held.texture.destroy).not.toHaveBeenCalled();
        expect(painter.acquireRTT(256)).not.toBe(pooled);
    });

    test('destroyRTTResources frees the pool and the shared FBO, and both come back on the next acquire', () => {
        const gl = painter.context.gl;
        const obj = painter.acquireRTT(256);
        vi.spyOn(obj.texture, 'destroy');
        painter.bindRTT(obj);
        painter.releaseRTT(obj);

        painter.destroyRTTResources();

        expect(painter._rttFbos.size).toBe(0);
        expect(obj.texture.destroy).toHaveBeenCalledTimes(1);
        expect(gl.deleteFramebuffer).toHaveBeenCalledTimes(1);
        expect(gl.deleteRenderbuffer).toHaveBeenCalledTimes(1);

        painter.bindRTT(painter.acquireRTT(256));
        expect(gl.createFramebuffer).toHaveBeenCalledTimes(2);
    });

    test('painter.destroy cleans up pooled RTT textures and shared FBO', () => {
        const objs = [];
        for (let i = 0; i < 10; i++) {
            const obj = painter.acquireRTT(128);
            vi.spyOn(obj.texture, 'destroy');
            objs.push(obj);
        }
        // Bind one to force shared FBO creation
        painter.bindRTT(objs[0]);
        for (const obj of objs) painter.releaseRTT(obj);

        painter.destroy();
        const destroyed = objs.filter(o => o.texture.destroy.mock.calls.length > 0).length;
        expect(destroyed).toBe(10);
        expect(painter._rttFbos.size).toBe(0);
    });

    // Scenic Roads (second review 5): the scene target is refused on a multisampled canvas, known by
    // the context's antialias attribute (read once), not gl.SAMPLES of whatever framebuffer is bound.
    describe('the scene target and the canvas\'s antialias', () => {
        const withReader = (antialias: boolean, boundSamples: number) => {
            const gl = createNullGL() as any;
            gl.getContextAttributes.mockReturnValue({antialias});
            const getParameter = gl.getParameter.getMockImplementation();
            gl.getParameter.mockImplementation((p: number) => p === gl.SAMPLES ? boundSamples : getParameter(p));
            const transform = new MercatorTransform({minZoom: 0, maxZoom: 22, minPitch: 0, maxPitch: 60, renderWorldCopies: true});
            transform.resize(64, 64);
            const p = new Painter(gl, transform);
            p.resize(64, 64, 1);
            const layer = new CustomStyleLayer({id: 'reader', type: 'custom', readsSceneDepth: true, render() {}} as any, {});
            const s = {_order: ['reader'], _layers: {reader: layer}} as any as Style;
            return {gl, p, s};
        };

        test('an antialiased canvas: no scene target, whatever framebuffer is bound; the attribute read once', () => {
            const {gl, p, s} = withReader(true, 0);
            p._prepareSceneTarget(s);
            p._prepareSceneTarget(s);
            expect(p.sceneTarget).toBeFalsy();
            expect(gl.getContextAttributes).toHaveBeenCalledTimes(1);
        });

        test('a canvas without antialias: a scene target, even with a multisampled framebuffer bound', () => {
            const {p, s} = withReader(false, 4);
            p._prepareSceneTarget(s);
            expect(p.sceneTarget).toBeTruthy();
            p.sceneTarget.destroy();
        });
    });
});
