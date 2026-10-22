import {Texture} from '../texture.ts';
import {DepthMode} from '../depth_mode.ts';
import {CullFaceMode} from '../cull_face_mode.ts';
import {
    colorReliefUniformValues
} from '../program/color_relief_program.ts';
import {getProjectionDataForTile, getTerrainDataForTile, type RenderContext} from '../../render/render_context.ts';

import type {ColorMode} from '../color_mode.ts';
import type {StencilMode} from '../stencil_mode.ts';
import type {Painter} from '../../render/painter.ts';
import type {TileManager} from '../../tile/tile_manager.ts';
import type {ColorReliefStyleLayer} from '../../style/style_layer/color_relief_style_layer.ts';
import type {OverscaledTileID} from '../../tile/tile_id.ts';
import {maskCells, maskTileManager, type MaskCell} from './water_mask.ts';

export function drawColorRelief(painter: Painter, tileManager: TileManager, layer: ColorReliefStyleLayer, tileIDs: OverscaledTileID[], renderContext: RenderContext): void {
    if (renderContext.currentPass !== 'translucent') return;
    if (!tileIDs.length) return;

    const projection = painter.style.projection;
    const useSubdivision = projection.useSubdivision;

    const depthMode = painter.getDepthModeForSublayer(0, DepthMode.ReadOnly);
    const colorMode = painter.colorModeForRenderPass();

    // Scenic Roads: drawn as the mask source's raster (water_mask.ts).
    const mask = maskTileManager(painter, layer);
    if (mask) {
        const cells = maskCells(painter, mask, tileIDs);
        renderColorRelief(painter, tileManager, layer, cells.map((c) => c.id), {}, depthMode, colorMode, false, renderContext, cells);
        return;
    }

    // Globe (or any projection with subdivision) needs two-pass rendering to avoid artifacts when rendering texture tiles.
    // See comments in draw_raster.ts for more details.
    if (useSubdivision) {
        // Two-pass rendering
        const [stencilBorderless, stencilBorders, coords] = painter.stencilConfigForOverlapTwoPass(tileIDs);
        renderColorRelief(painter, tileManager, layer, coords, stencilBorderless, depthMode, colorMode, false, renderContext); // draw without borders
        renderColorRelief(painter, tileManager, layer, coords, stencilBorders, depthMode, colorMode, true, renderContext); // draw with borders
    } else {
        // Simple rendering
        const [stencil, coords] = painter.getStencilConfigForOverlapAndUpdateStencilID(tileIDs);
        renderColorRelief(painter, tileManager, layer, coords, stencil, depthMode, colorMode, false, renderContext);
    }
}

let textureMaxSize = 0;
/**
 * Draws the color-relief tiles of one pass.
 *
 * A loaded raster-DEM tile can carry no DEM at all (e.g. an empty 204 response);
 * such tiles are skipped before the first-tile setup reads from them.
 */
function renderColorRelief(
    painter: Painter,
    tileManager: TileManager,
    layer: ColorReliefStyleLayer,
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
    const program = painter.useProgram('colorRelief');
    const align = !painter.options.moving;

    const textureFilter = layer.paint.get('resampling') === 'nearest' ?  gl.NEAREST : gl.LINEAR;

    let firstTile = true;
    let colorRampSize = 0;

    for (let i = 0; i < coords.length; i++) {
        const coord = coords[i], cell = cells?.[i];
        const tile = tileManager.getTile(cell ? cell.dem : coord);
        const dem = tile.dem;
        if (!dem?.data) {
            continue;
        }
        if(firstTile) {
            // we should avoid calling gl.getParameter at runtime (GPU stall risk)
            textureMaxSize ||= gl.getParameter(gl.MAX_TEXTURE_SIZE);
            const maxLength = textureMaxSize;
            const {elevationTexture, colorTexture} = layer.getColorRampTextures(context, maxLength, dem.getUnpackVector());
            context.activeTexture.set(gl.TEXTURE1);
            elevationTexture.bind(gl.NEAREST, gl.CLAMP_TO_EDGE);
            context.activeTexture.set(gl.TEXTURE4);
            colorTexture.bind(gl.LINEAR, gl.CLAMP_TO_EDGE);
            firstTile = false;
            colorRampSize = elevationTexture.size[0];
        }

        const textureStride = dem.stride;

        context.activeTexture.set(gl.TEXTURE0);

        if (!tile.demTexture || tile.needsColorReliefPrepare) {
            context.pixelStoreUnpackPremultiplyAlpha.set(false);
            tile.demTexture ||= painter.getTileTexture(textureStride) ??
                new Texture(context, {width: textureStride, height: textureStride, data: null}, gl.RGBA);
            tile.demTexture.update(dem.getPixels(), {premultiply: false});
            tile.needsColorReliefPrepare = false;
        }
        // (Scenic Roads: masked, the shader interpolates the depths itself, those with none left out.)
        tile.demTexture.bind(cell ? gl.NEAREST : textureFilter, gl.CLAMP_TO_EDGE);
        if (cell) {
            context.activeTexture.set(gl.TEXTURE2);
            cell.mask.bind(gl.LINEAR, gl.CLAMP_TO_EDGE, gl.LINEAR_MIPMAP_LINEAR);
        }

        const mesh = projection.getMeshFromTileID(context, coord.canonical, useBorder, true, 'raster');

        const terrainData = getTerrainDataForTile(renderContext, coord);

        const projectionData = getProjectionDataForTile(renderContext, coord, {aligned: align});

        program.draw(context, gl.TRIANGLES, depthMode, cell ? cell.stencil : stencilModes[coord.overscaledZ], colorMode, CullFaceMode.backCCW,
            colorReliefUniformValues(layer, tile.dem, colorRampSize, cell), terrainData, projectionData, layer.id, mesh.vertexBuffer, mesh.indexBuffer, mesh.segments);
    }
}
