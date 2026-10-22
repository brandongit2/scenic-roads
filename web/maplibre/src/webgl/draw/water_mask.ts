// Scenic Roads: a raster-DEM layer drawn as another source's raster, its drawn alpha the layer's
// coverage (`"scenic:mask": "<raster source id>"` in the layer's metadata; the app's depth over its
// water tiles). Each part of the view is drawn where a DEM tile meets a tile the mask source draws,
// over the finer of the two; both textures are sampled by the part's place in their tiles. Where
// the mask source has nothing drawn (none of its tiles, or its layers hidden), nothing is drawn.
import {mat4} from 'gl-matrix';
import {EXTENT} from '../../data/extent.ts';
import {OverscaledTileID} from '../../tile/tile_id.ts';
import {StencilMode} from '../stencil_mode.ts';

import type {Painter} from '../../render/painter.ts';
import type {StyleLayer} from '../../style/style_layer.ts';
import type {TileManager} from '../../tile/tile_manager.ts';
import type {Texture} from '../texture.ts';

/** A part of the view: drawn with `id`'s mesh; `dem` and `mask` each at their tile's [x, y, scale]. */
export type MaskCell = {
    id: OverscaledTileID;
    dem: OverscaledTileID;
    mask: Texture;
    demCell: [number, number, number];
    maskCell: [number, number, number];
    stencil: Readonly<StencilMode>;
};

/** The mask source's tile manager of a layer that names one. */
export function maskTileManager(painter: Painter, layer: StyleLayer): TileManager | null {
    const id = (layer.metadata as {'scenic:mask'?: string} | undefined)?.['scenic:mask'];
    return id ? painter.style.tileManagers[id] ?? null : null;
}

/** Where `fine` (z ≥ coarse's) lies in `coarse`: its [x, y, scale] in coarse's units; null outside. */
function within(coarse: OverscaledTileID, fine: OverscaledTileID): [number, number, number] | null {
    const a = coarse.canonical, b = fine.canonical, d = b.z - a.z;
    if (d < 0 || coarse.wrap !== fine.wrap || b.x >> d !== a.x || b.y >> d !== a.y) return null;
    const s = 1 / 2 ** d;
    return [(b.x - (a.x << d)) * s, (b.y - (a.y << d)) * s, s];
}

/** The parts of the DEM tiles `coords` the mask source draws, each with its stencil (finer parts
 * first: where both a tile and an ancestor of it are drawn, the finer shows, as a raster's tiles do). */
export function maskCells(painter: Painter, mask: TileManager, coords: OverscaledTileID[]): MaskCell[] {
    // (The mask's layers hidden: nothing drawn, as nothing of the mask is.)
    if (!mask.used) return [];
    const masks = mask.getRenderableIds().map((k) => mask.getTileByID(k)).filter((t) => t?.texture);
    const cells: (Omit<MaskCell, 'stencil'> & {prio: number})[] = [];
    for (const dem of coords) {
        for (const m of masks) {
            const w = m.tileID;
            const inDem = within(dem, w);
            if (inDem) {
                const id = new OverscaledTileID(w.canonical.z, dem.wrap, w.canonical.z, w.canonical.x, w.canonical.y);
                // (Drawn into the terrain's textures: placed by the DEM tile's place there.)
                if (dem.terrainRttPosMatrix32f) {
                    const tm = mat4.translate(mat4.create(), dem.terrainRttPosMatrix32f as unknown as mat4, [inDem[0] * EXTENT, inDem[1] * EXTENT, 0]);
                    id.terrainRttPosMatrix32f = new Float32Array(mat4.scale(tm, tm, [inDem[2], inDem[2], 1]));
                }
                cells.push({id, dem, mask: m.texture, demCell: inDem, maskCell: [0, 0, 1], prio: w.canonical.z * 64 + dem.overscaledZ});
                continue;
            }
            const inMask = within(w, dem);
            if (inMask) cells.push({id: dem, dem, mask: m.texture, demCell: [0, 0, 1], maskCell: inMask, prio: dem.overscaledZ * 64 + w.canonical.z});
        }
    }
    if (!cells.length) return [];
    cells.sort((a, b) => b.prio - a.prio);
    const prios = [...new Set(cells.map((c) => c.prio))];
    const gl = painter.context.gl;
    if (prios.length === 1) return cells.map((c) => ({...c, stencil: StencilMode.disabled}));
    painter.currentStencilSource = undefined;
    if (painter.nextStencilID + prios.length > 256) painter.clearStencil();
    const base = painter.nextStencilID;
    painter.nextStencilID += prios.length;
    const modes = new Map(prios.map((p, i) => [p, new StencilMode({func: gl.GEQUAL, mask: 0xFF}, base + prios.length - 1 - i, 0xFF, gl.KEEP, gl.KEEP, gl.REPLACE)]));
    return cells.map((c) => ({...c, stencil: modes.get(c.prio)}));
}
