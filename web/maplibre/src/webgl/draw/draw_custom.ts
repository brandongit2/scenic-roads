import {mat4} from 'gl-matrix';
import {DepthMode} from '../depth_mode.ts';
import {StencilMode} from '../stencil_mode.ts';
import {OverscaledTileID} from '../../tile/tile_id.ts';
import {EXTENT} from '../../data/extent.ts';

import type {Painter} from '../../render/painter.ts';
import type {RenderContext} from '../../render/render_context.ts';
import type {TileManager} from '../../tile/tile_manager.ts';
import type {CustomDrapeMethodInput, CustomLayerProjectionDataParams, CustomRenderMethodInput, CustomStyleLayer, CustomTerrainDepth} from '../../style/style_layer/custom_style_layer.ts';

/** GLSL reading the terrain's packed depth (draw_terrain's drawDepth, terrain_depth.fragment.glsl). */
const UNPACK_TERRAIN_DEPTH = `highp float unpackTerrainDepth(highp sampler2D depth, vec2 uv) {
    return dot(texture(depth, uv), vec4(1.0 / (256.0 * 256.0 * 256.0), 1.0 / (256.0 * 256.0), 1.0 / 256.0, 1.0));
}`;

function terrainDepthOf(painter: Painter): CustomTerrainDepth | null {
    const texture = painter.style.map?.terrain?.getDepthTexture() ?? null;
    if (!texture) return null;
    return {
        texture,
        width: Math.round(painter.width / painter.pixelRatio),
        height: Math.round(painter.height / painter.pixelRatio),
        revision: painter.terrainFacilitator.renderTime,
        unpack: UNPACK_TERRAIN_DEPTH,
    };
}

/**
 * Scenic Roads: the matrix taking `tile`'s coordinates to clip space in the texture of terrain tile
 * `target` (see CustomDrapeMethodInput.getDrapeMatrix), or null where they don't meet.
 */
export function drapeMatrix(target: OverscaledTileID, tile: {z: number; x: number; y: number}): Float32Array | null {
    const t = target.canonical;
    // Both in the finer zoom's tile units.
    const z = Math.max(t.z, tile.z);
    const ts = 1 << (z - t.z), ss = 1 << (z - tile.z);
    const tx = (t.x + target.wrap * (1 << t.z)) * ts, ty = t.y * ts;
    const sx = tile.x * ss, sy = tile.y * ss;
    if (sx + ss <= tx || sx >= tx + ts || sy + ss <= ty || sy >= ty + ts) return null;
    // tile units → the target's tile units (0..EXTENT) → clip.
    const m = mat4.ortho(new Float64Array(16) as unknown as mat4, 0, EXTENT, EXTENT, 0, 0, 1);
    mat4.translate(m, m, [(sx - tx) / ts * EXTENT, (sy - ty) / ts * EXTENT, 0]);
    mat4.scale(m, m, [ss / ts, ss / ts, 1]);
    return new Float32Array(m);
}

export function drawCustom(painter: Painter, tileManager: TileManager, layer: CustomStyleLayer, renderContext: RenderContext, coords?: OverscaledTileID[]): void {

    const {isRenderingGlobe} = renderContext;
    const context = painter.context;
    const implementation = layer.implementation;
    const projection = painter.style.projection;
    const transform = painter.transform;
    const draping = renderContext.isRenderingToTexture;
    // (Draped into a terrain tile's texture, or into the frame.)
    if (draping && !implementation.drape) return;

    const projectionData = transform.getProjectionDataForCustomLayer(isRenderingGlobe);
    const readsDepth = !draping && renderContext.currentPass === 'translucent' && !!implementation.readsSceneDepth && !!painter.sceneTarget;
    const framebuffer = draping ? context.bindFramebuffer.get() : readsDepth ? painter.sceneTarget.colorOnly : painter.mainFramebuffer();

    const customLayerArgs: CustomRenderMethodInput = {
        farZ: transform.farZ,
        nearZ: transform.nearZ,
        fov: transform.fov * Math.PI / 180, // fov converted to radians
        modelViewProjectionMatrix: transform.modelViewProjectionMatrix,
        projectionMatrix: transform.projectionMatrix,
        shaderData: {
            variantName: projection.shaderVariantName,
            vertexShaderPrelude: `const float PI = 3.141592653589793;\nuniform mat4 u_projection_matrix;\n${projection.shaderPreludeCode.vertexSource}`,
            define: projection.shaderDefine,
        },
        defaultProjectionData: projectionData,
        getProjectionData: (params: CustomLayerProjectionDataParams) => {
            return transform.getProjectionData({
                overscaledTileID: new OverscaledTileID(
                    params.tileID.canonical.z,
                    params.tileID.wrap ?? 0,
                    params.tileID.canonical.z,
                    params.tileID.canonical.x,
                    params.tileID.canonical.y,
                ),
                aligned: params.aligned,
                applyGlobeMatrix: params.applyGlobeMatrix,
                applyTerrainMatrix: params.applyTerrainMatrix,
            });
        },
        framebuffer,
        depthRangeFor3D: [renderContext.depthRangeFor3D[0], renderContext.depthRangeFor3D[1]],
        terrainDepth: terrainDepthOf(painter),
        sceneDepth: readsDepth ? {
            texture: painter.sceneTarget.depthStencil,
            width: painter.sceneTarget.width,
            height: painter.sceneTarget.height,
        } : null,
    };

    const renderingMode = implementation.renderingMode ? implementation.renderingMode : '2d';

    if (draping) {
        // Scenic Roads: into the terrain tile's texture (render_to_texture.ts binds it and sets the
        // viewport), once for each tile drawn.
        const target = coords[0];
        const drapeArgs: CustomDrapeMethodInput = {
            ...customLayerArgs,
            tileID: {z: target.canonical.z, x: target.canonical.x, y: target.canonical.y, wrap: target.wrap},
            size: context.viewport.get()[2],
            getDrapeMatrix: (tile) => drapeMatrix(target, tile),
        };
        painter.setCustomLayerDefaults();
        context.setColorMode(painter.colorModeForRenderPass());
        context.setStencilMode(StencilMode.disabled);
        context.setDepthMode(DepthMode.disabled);

        implementation.drape.call(implementation, context.gl, drapeArgs);

        context.setDirty();
        painter.setBaseState();
        context.bindFramebuffer.set(framebuffer);
    } else if (renderContext.currentPass === 'offscreen') {
        const prerender = implementation.prerender;
        if (prerender) {
            painter.setCustomLayerDefaults();
            context.setColorMode(painter.colorModeForRenderPass());

            prerender.call(implementation, context.gl, customLayerArgs);

            context.setDirty();
            painter.setBaseState();
        }
    } else if (renderContext.currentPass === 'translucent') {

        painter.setCustomLayerDefaults();

        context.setColorMode(painter.colorModeForRenderPass());
        context.setStencilMode(StencilMode.disabled);

        const depthMode = renderingMode === '3d' ?
            painter.getDepthModeFor3D() :
            painter.getDepthModeForSublayer(0, DepthMode.ReadOnly);

        context.setDepthMode(depthMode);
        // (A layer reading the frame's depth draws into its colour alone: the depth attached
        // would be a feedback loop.)
        if (readsDepth) context.bindFramebuffer.set(framebuffer);

        implementation.render(context.gl, customLayerArgs);

        context.setDirty();
        painter.setBaseState();
        context.bindFramebuffer.set(painter.mainFramebuffer());
    }
}
