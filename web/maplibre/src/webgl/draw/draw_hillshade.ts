import {Texture} from '../texture.ts';
import {DepthMode} from '../depth_mode.ts';
import {CullFaceMode} from '../cull_face_mode.ts';
import {hillshadeUniformValues} from '../program/hillshade_program.ts';
import {getProjectionDataForTile, getTerrainDataForTile, type RenderContext} from '../../render/render_context.ts';

import type {ColorMode} from '../color_mode.ts';
import type {StencilMode} from '../stencil_mode.ts';
import type {Painter} from '../../render/painter.ts';
import type {TileManager} from '../../tile/tile_manager.ts';
import type {HillshadeStyleLayer} from '../../style/style_layer/hillshade_style_layer.ts';
import type {OverscaledTileID} from '../../tile/tile_id.ts';
import {maskCells, maskTileManager, type MaskCell} from './water_mask.ts';

// Scenic Roads: a hillshade layer draws precomputed shading tiles (the app's slope and depth-shade
// sources: each pixel's steepness and mean direction, hillshade.fragment.glsl), sampled as stored:
// no prepare pass deriving slopes from a DEM at render time (FORK.md).
export function drawHillshade(painter: Painter, tileManager: TileManager, layer: HillshadeStyleLayer, tileIDs: OverscaledTileID[], renderContext: RenderContext): void {
    if (renderContext.currentPass !== 'translucent') return;

    const depthMode = painter.getDepthModeForSublayer(0, DepthMode.ReadOnly);
    const colorMode = painter.colorModeForRenderPass();

    // Scenic Roads: drawn as the mask source's raster (water_mask.ts).
    const mask = maskTileManager(painter, layer);
    if (mask) {
        const cells = maskCells(painter, mask, tileIDs);
        renderHillshade(painter, tileManager, layer, cells.map((c) => c.id), {}, depthMode, colorMode, false, renderContext, cells);
        return;
    }
    // Globe (or any projection with subdivision) needs two-pass rendering to avoid artifacts when rendering texture tiles.
    // See comments in draw_raster.ts for more details.
    if (painter.style.projection.useSubdivision) {
        // Two-pass rendering
        const [stencilBorderless, stencilBorders, coords] = painter.stencilConfigForOverlapTwoPass(tileIDs);
        renderHillshade(painter, tileManager, layer, coords, stencilBorderless, depthMode, colorMode, false, renderContext); // draw without borders
        renderHillshade(painter, tileManager, layer, coords, stencilBorders, depthMode, colorMode, true, renderContext); // draw with borders
    } else {
        // Simple rendering
        const [stencil, coords] = painter.getStencilConfigForOverlapAndUpdateStencilID(tileIDs);
        renderHillshade(painter, tileManager, layer, coords, stencil, depthMode, colorMode, false, renderContext);
    }
}

function renderHillshade(
    painter: Painter,
    tileManager: TileManager,
    layer: HillshadeStyleLayer,
    coords: OverscaledTileID[],
    stencilModes: {[_: number]: Readonly<StencilMode>},
    depthMode: Readonly<DepthMode>,
    colorMode: Readonly<ColorMode>,
    useBorder: boolean,
    renderContext: RenderContext,
    cells?: MaskCell[]
) {
    const projection = painter.style.projection;
    const context = painter.context;
    const gl = context.gl;

    const defines = [`#define NUM_ILLUMINATION_SOURCES ${layer.paint.get('hillshade-highlight-color').values.length}`];
    const program = painter.useProgram('hillshade', null, false, defines);
    const align = !painter.options.moving;
    const textureFilter = layer.paint.get('resampling') === 'nearest' ? gl.NEAREST : gl.LINEAR;

    for (let i = 0; i < coords.length; i++) {
        const coord = coords[i], cell = cells?.[i];
        const tile = tileManager.getTile(cell ? cell.dem : coord);
        const dem = tile.dem;
        if (!dem?.data) {
            continue;
        }

        // The tile's pixels (with the one-pixel border its neighbours backfill) as a texture, as the
        // colour relief keeps it (draw_color_relief.ts: the same pixels, one texture).
        context.activeTexture.set(gl.TEXTURE0);
        if (!tile.demTexture || tile.needsHillshadePrepare) {
            context.pixelStoreUnpackPremultiplyAlpha.set(false);
            tile.demTexture ||= painter.getTileTexture(dem.stride) ??
                new Texture(context, {width: dem.stride, height: dem.stride, data: null}, gl.RGBA);
            tile.demTexture.update(dem.getPixels(), {premultiply: false});
            tile.needsHillshadePrepare = false;
        }
        tile.demTexture.bind(textureFilter, gl.CLAMP_TO_EDGE);
        if (cell) {
            context.activeTexture.set(gl.TEXTURE2);
            cell.mask.bind(gl.LINEAR, gl.CLAMP_TO_EDGE, gl.LINEAR_MIPMAP_LINEAR);
        }

        const mesh = projection.getMeshFromTileID(context, coord.canonical, useBorder, true, 'raster');
        const terrainData = getTerrainDataForTile(renderContext, coord);
        const projectionData = getProjectionDataForTile(renderContext, coord, {aligned: align});

        program.draw(context, gl.TRIANGLES, depthMode, cell ? cell.stencil : stencilModes[coord.overscaledZ], colorMode, CullFaceMode.backCCW,
            hillshadeUniformValues(painter, layer, cell), terrainData, projectionData, layer.id, mesh.vertexBuffer, mesh.indexBuffer, mesh.segments);
    }
}
