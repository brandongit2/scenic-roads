import {Color} from '@maplibre/maplibre-gl-style-spec';
import {drawTerrain} from './draw/draw_terrain.ts';
import {ImageSource} from '../source/image_source.ts';
import {RTT_DIFFERENCES, RTTFingerprint, type RTTDifference} from './rtt_fingerprint.ts';

import {isCustomStyleLayer} from '../style/style_layer/custom_style_layer.ts';

import type {Tile} from '../tile/tile.ts';
import type {OverscaledTileID} from '../tile/tile_id.ts';
import type {Style} from '../style/style.ts';
import type {Terrain} from '../render/terrain.ts';
import type {Texture} from './texture.ts';
import type {StyleLayer} from '../style/style_layer.ts';
import type {Painter} from '../render/painter.ts';
import type {RenderContext} from '../render/render_context.ts';

/**
 * lookup table which layers should rendered to texture
 */
const LAYERS_TO_TEXTURES: { [keyof in StyleLayer['type']]?: boolean } = {
    background: true,
    fill: true,
    line: true,
    raster: true,
    hillshade: true,
    'color-relief': true
};

/**
 * Scenic Roads: whether a layer is drawn into the terrain's textures: the types above but elevated
 * lines and fills (`*-z-offset`, `*-elevation-reference`: drawn in 3D over the terrain), and a
 * custom layer with a `drape` method.
 */
export function drapes(layer: StyleLayer): boolean {
    if (LAYERS_TO_TEXTURES[layer.type]) return !isElevated(layer);
    return isCustomStyleLayer(layer) && typeof layer.implementation.drape === 'function';
}

const isElevated = (layer: StyleLayer): boolean =>
    typeof (layer as {elevationReference?: () => number}).elevationReference === 'function' &&
    (layer as unknown as {elevationReference: () => number}).elevationReference() > 0;

/** Scenic Roads: the key of a custom layer's drape among the sources' fingerprints. */
const customDrapeKey = (layer: StyleLayer) => `custom:${layer.id}`;

/**
 * @internal
 * Renders RTT-eligible layers into per-tile cached textures, then drapes
 * them onto the terrain mesh. Slots live on each Tile so their lifetime
 * tracks the tile itself; the underlying FBO+texture handles are recycled
 * via the painter's RTT pool.
 */
export class RenderToTexture {
    painter: Painter;
    terrain: Terrain;
    /** RTT texture dimension in pixels (tile size × terrain quality factor). */
    rttSize: number;
    /**
     * coordsAscending contains a list of all tiles which should be rendered for one render-to-texture tile
     * e.g. render 4 raster-tiles with size 256px to the 512px render-to-texture tile
     */
    _coordsAscending: {[_: string]: {[_:string]: OverscaledTileID[]}};
    /**
     * This frame's fingerprint for each render-to-texture tile, keyed by source id
     * then tile key. A tile whose stored fingerprint differs has stale textures.
     */
    _rttFingerprints: Record<string, Record<string, RTTFingerprint>>;
    /**
     * store for render-stacks
     * a render stack is a set of layers which should be rendered into one texture
     * every stylesheet can have multiple stacks. A new stack is created if layers which should
     * not rendered to texture sit between layers which should rendered to texture. e.g. hillshading or symbols
     */
    _stacks: string[][];
    /**
     * remember the previous processed layer to check if a new stack is needed
     */
    _prevDraped: boolean;
    /**
     * a list of tiles that can potentially rendered
     */
    _renderableTiles: Tile[];
    /**
     * a list of tiles that should be rendered to screen in the next render-call
     */
    _rttTiles: Tile[];
    /**
     * a list of all layer-ids which should be rendered
     */
    _renderableLayerIds: string[];
    /**
     * the zoom of the previous prepareForRender call
     */
    _lastPrepareZoom: number;
    /**
     * whether the render loop needs a follow-up frame to refresh cached textures that were kept this frame:
     * textures rendered at another zoom, and stale textures beyond the per-frame budget.
     */
    needsFollowUpFrame: boolean = false;
    constructor(painter: Painter, terrain: Terrain) {
        this.painter = painter;
        this.terrain = terrain;
        this.rttSize = terrain.tileManager.tileSize * terrain.qualityFactor;
    }

    getTexture(tile: Tile): Texture {
        return tile.getRTT(this._stacks.length - 1).texture;
    }

    /**
     * Collects the frame's tiles, layers and source fingerprints and releases the textures that need
     * re-rendering. Textures that differ only by zoom are kept while the zoom is changing or the map is moving
     * and re-rendered one tile per frame once it has stopped, nearest to the camera first; textures with other
     * source tiles under them are re-rendered one per frame too, `needsFollowUpFrame` bringing in the rest.
     * Textures whose visible layer set changed (a layer entered or left its zoom range) are kept while the zoom
     * is changing and then all re-rendered in the same frame, since a tile-by-tile change would show; a source
     * data change re-renders immediately.
     */
    prepareForRender(style: Style, zoom: number): void {
        const zoomChanged = zoom !== this._lastPrepareZoom;
        this._lastPrepareZoom = zoom;
        this._stacks = [];
        this._prevDraped = false;
        this._rttTiles = [];
        this._renderableTiles = this.terrain.tileManager.getRenderableTiles();
        this._renderableLayerIds = style._order.filter(id => !style._layers[id].isHidden(zoom));
        // Scenic Roads: which layers are draped is part of what a texture holds (a line lifted off
        // the terrain, or set back down, changes it), marked by a '^' after the layer's id.
        const visibleLayerIds = this._renderableLayerIds.map(id => drapes(style._layers[id]) ? `${id}^` : id).join();

        // (Scenic Roads: the sources of the layers draped, drapes(): a source used only by lifted
        // layers has no fingerprint, else every texture would be drawn again every frame.)
        const rttSourceIds = new Set<string>();
        for (const layerId of this._renderableLayerIds) {
            const layer = style._layers[layerId];
            const source = layer.source;
            if (source && drapes(layer)) rttSourceIds.add(source);
        }

        this._coordsAscending = {};
        this._rttFingerprints = {};
        for (const sourceId of rttSourceIds) {
            const tileManager = style.tileManagers[sourceId];
            if (!tileManager) continue;

            this._coordsAscending[sourceId] = {};
            const coordsAscending = this._coordsAscending[sourceId];
            const source = tileManager.getSource();
            const terrainTileRanges = source instanceof ImageSource ? source.terrainTileRanges : null;
            for (const tileID of tileManager.getVisibleCoordinates()) {
                const keys = this.terrain.tileManager.getTerrainCoords(tileID, terrainTileRanges);
                for (const key in keys) {
                    coordsAscending[key] ||= [];
                    coordsAscending[key].push(keys[key]);
                }
            }

            this._rttFingerprints[sourceId] = {};
            const fingerprints = this._rttFingerprints[sourceId];
            const revision = tileManager.getState().revision;
            for (const key in coordsAscending)
                fingerprints[key] = new RTTFingerprint(coordsAscending[key], revision, zoom, visibleLayerIds);
        }

        // Scenic Roads: a custom layer's drape is drawn again when its revision changes (and, as the
        // sources' are, when the zoom or the visible layers do).
        for (const layerId of this._renderableLayerIds) {
            const layer = style._layers[layerId];
            if (!isCustomStyleLayer(layer) || !drapes(layer)) continue;
            const fingerprints = this._rttFingerprints[customDrapeKey(layer)] = {};
            const revision = layer.implementation.drapeRevision ?? 0;
            for (const tile of this._renderableTiles)
                fingerprints[tile.tileID.key] = new RTTFingerprint([tile.tileID], revision, zoom, visibleLayerIds);
        }

        // check tiles to render
        this.needsFollowUpFrame = false;
        const moving = zoomChanged || this.painter.options.moving;
        let staleTileReleased = false;
        for (const tile of this._renderableTiles) {
            const difference = this._textureDifference(tile);
            if (difference === 'none') continue;
            if ((difference === 'zoom' && moving) || (difference === 'visibleLayers' && zoomChanged)) {
                this.needsFollowUpFrame = true;
            } else if (difference === 'zoom' || difference === 'sourceTiles') {
                if (staleTileReleased) this.needsFollowUpFrame = true;
                else tile.releaseRTT(this.painter);
                staleTileReleased = true;
            } else {
                tile.releaseRTT(this.painter);
            }
        }
    }

    /**
     * The most severe difference, over the sources rendered to texture, between the tile's cached textures
     * and what this frame would render into them.
     */
    _textureDifference(tile: Tile): RTTDifference {
        let worst: RTTDifference = 'none';
        for (const source in this._rttFingerprints) {
            const frameFingerprint = this._rttFingerprints[source][tile.tileID.key];
            if (!frameFingerprint) continue;
            const difference = frameFingerprint.difference(tile.rttFingerprint[source]);
            if (RTT_DIFFERENCES.indexOf(difference) > RTT_DIFFERENCES.indexOf(worst)) worst = difference;
        }
        return worst;
    }

    /**
     * due that switching textures is relatively slow, the render
     * layer-by-layer context is not practicable. To bypass this problem
     * this lines of code stack all layers and later render all at once.
     * Because of the stylesheet possibility to mixing render-to-texture layers
     * and 'live'-layers (f.e. symbols) it is necessary to create more stacks. For example
     * a symbol-layer is in between of fill-layers.
     * @param layer - the layer to render
     * @param renderContext - shared state for the current render
     * @returns if true layer is rendered to texture, otherwise false
     */
    renderLayer(layer: StyleLayer, renderContext: RenderContext): boolean {
        if (layer.isHidden(this.painter.transform.zoom)) return false;

        const draped = drapes(layer);
        const painter = this.painter;
        const isLastLayer = this._renderableLayerIds[this._renderableLayerIds.length - 1] === layer.id;

        // remember background, fill, line & raster layer to render into a stack
        if (draped) {
            // create a new stack if previous layer was not rendered to texture (f.e. symbols)
            if (!this._prevDraped) this._stacks.push([]);
            // push current render-to-texture layer to render-stack
            this._prevDraped = true;
            this._stacks[this._stacks.length - 1].push(layer.id);
            // rendering is done later, all in once
            if (!isLastLayer) return true;
        }

        // in case a stack is finished render all collected stack-layers into a texture
        // (a draped last layer has set _prevDraped too)
        if (this._prevDraped) {
            this._prevDraped = draped;
            const stack = this._stacks.length - 1, layers = this._stacks[stack] || [];
            renderContext.isRenderingToTexture = true;
            // Scenic Roads: each tile's texture has its own stencil, cleared below, so its IDs start
            // at 1; the main framebuffer's counter is kept and restored after (a clearStencil here
            // on overflow reset it, so a later lifted layer's GEQUAL references fell below values an
            // earlier one wrote in the main framebuffer).
            const mainStencilID = painter.nextStencilID;
            for (const tile of this._renderableTiles) {
                this._rttTiles.push(tile);
                // Cache hit: this tile already has a RTT object for this stack from a previous frame.
                if (tile.getRTT(stack)) continue;
                const obj = tile.acquireRTT(painter, stack, this.rttSize);
                painter.bindRTT(obj);
                painter.context.clear({color: Color.transparent, stencil: 0});
                painter.currentStencilSource = undefined;
                painter.nextStencilID = 1;
                for (const layerId of layers) {
                    const layer = painter.style._layers[layerId];
                    const coords = layer.source ? this._coordsAscending[layer.source][tile.tileID.key] : [tile.tileID];
                    painter.context.viewport.set([0, 0, this.rttSize, this.rttSize]);
                    painter.renderTileClippingMasks(layer, coords);
                    painter.renderLayer(painter, painter.style.tileManagers[layer.source], layer, coords, renderContext);
                    if (layer.source) tile.rttFingerprint[layer.source] = this._rttFingerprints[layer.source][tile.tileID.key];
                    else if (isCustomStyleLayer(layer)) tile.rttFingerprint[customDrapeKey(layer)] = this._rttFingerprints[customDrapeKey(layer)][tile.tileID.key];
                }
                obj.texture.generateMipmap();
            }
            painter.nextStencilID = mainStencilID;
            painter.currentStencilSource = undefined;
            renderContext.isRenderingToTexture = false;
            drawTerrain(this.painter, this.terrain, this._rttTiles, renderContext);
            this._rttTiles = [];
            // Scenic Roads: the stencil masks drawn were the textures', not the frame's.
            painter.currentStencilSource = undefined;

            // Scenic Roads: the custom layers draped here draw in the main pass right after the
            // terrain carrying their drape, before the next layer.
            for (const layerId of layers) {
                const stacked = painter.style._layers[layerId];
                if (isCustomStyleLayer(stacked)) painter.renderLayer(painter, undefined, stacked, [], renderContext);
            }

            return draped;
        }

        return false;
    }

}
