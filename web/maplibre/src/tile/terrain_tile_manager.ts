import {Tile} from './tile.ts';
import {EXTENT} from '../data/extent.ts';
import {mat4} from 'gl-matrix';
import {Evented} from '../util/evented.ts';
import {now} from '../util/time_control.ts';
import {coveringTiles, coveringZoomLevel, focalLengthPx, tileFootprintPx} from '../geo/projection/covering_tiles.ts';
import {cameraMercatorCoordinate} from '../geo/projection/mercator_utils.ts';
import {mercatorZfromAltitude} from '../geo/mercator_coordinate.ts';
import {createMat4f64} from '../util/util.ts';

import type {Terrain} from '../render/terrain.ts';
import type {CanonicalTileRange} from '../source/image_source.ts';
import type {Source} from '../source/source.ts';
import type {TileManager} from './tile_manager.ts';
import type {OverscaledTileID} from './tile_id.ts';
import type {ITransform} from '../geo/transform_interface.ts';

/**
 * Scenic Roads: the texture size for a tile spanning `px` device pixels: the power of two at or above
 * it, between `min` and `max`.
 */
export function rttSizeForFootprint(px: number, min: number, max: number): number {
    if (!(px < max)) return max;
    let size = min;
    while (size < px) size *= 2;
    return Math.min(size, max);
}

/**
 * @internal
 * This class is a helper for the Terrain-class, it:
 *
 * - loads raster-dem tiles
 * - manages all renderToTexture tiles.
 * - caches previous rendered tiles.
 * - finds all necessary renderToTexture tiles for a OverscaledTileID area
 * - finds the corresponding raster-dem tile for OverscaledTileID
 */
export class TerrainTileManager extends Evented {
    /**
     * tile manager for the raster-dem source.
     */
    tileManager: TileManager;
    /**
     * stores all render-to-texture tiles.
     */
    _tiles: {[_: string]: Tile};
    /**
     * contains a list of tileID-keys for the current scene. (only for performance)
     */
    _renderableTilesKeys: string[];
    /**
     * raster-dem-tile for a TileID cache.
     */
    _sourceTileCache: {[_: string]: string};
    /**
     * minimum zoomlevel to render the terrain.
     */
    minzoom: number;
    /**
     * maximum zoomlevel to render the terrain.
     */
    maxzoom: number;
    /**
     * render-to-texture tileSize in scene.
     */
    tileSize: number;
    /**
     * raster-dem tiles will load for performance the actualZoom - deltaZoom zoom-level.
     */
    deltaZoom: number;
    /**
     * used to determine whether the depth framebuffer needs updating
     */
    _lastTilesetChange: number = now();
    /**
     * Scenic Roads: whether the render-to-texture tiles' zoom counts the terrain's elevations and
     * their depth along the view (covering_tiles.ts), as every source's cover does, so the drapes
     * near the camera and at the screen's sides are as sharp as what stands on them. True by default;
     * false gives upstream's zoom (a switch for comparing the two).
     */
    terrainAwareZoom: boolean = true;
    /**
     * Scenic Roads: whether each render-to-texture tile's texture is sized by its footprint on screen
     * (`Tile.rttTargetSize`, the power of two at or above the device pixels its edge spans at its
     * nearest point, from `minRttSize` to the full size; only tiles deeper than the view's own zoom,
     * the lifted ones) and the terrain's gain stops splitting a tile
     * once its full-size texture is no coarser than the screen's pixels (covering_tiles' texelParity).
     * True by default with terrainAwareZoom; false gives every tile the full size (for comparing).
     */
    footprintSizing: boolean = true;
    /** Scenic Roads: the smallest texture footprintSizing gives a tile. */
    minRttSize: number = 128;
    /**
     * Scenic Roads: draped textures redrawn at a steady pace. A source tile's load (`releaseRTT`)
     * notes the terrain tiles under it (`_dirtyRTT`) for the next frame instead of dropping their
     * textures at once; the map leaves the textures alone for a tile of a source with no layer
     * draped (render_to_texture.ts `drapes`) shown; and while the camera moves at most
     * `movingRedraws` existing textures are redrawn a frame, whatever the reason, the others kept a
     * few frames longer (`RenderToTexture.prepareForRender`). A terrain tile without textures is drawn
     * at once (while moving, within standInFrames frames if a retired ancestor can stand in); at rest, as upstream. True by default; false gives upstream's drops.
     */
    pacedDrapes: boolean = true;
    /** Scenic Roads: with pacedDrapes, the textures redrawn a frame while the camera moves. */
    movingRedraws: number = 2;
    /** Scenic Roads: with pacedDrapes, the terrain tiles (by key) whose textures are to be redrawn. */
    _dirtyRTT: Set<string> = new Set();
    /**
     * Scenic Roads: with pacedDrapes, the most frames a new terrain tile's textures wait while the
     * camera moves, an ancestor's texture standing in (`RenderToTexture.prepareForRender`): when the
     * cover changes level (a group of tiles splits), its new tiles' textures are drawn over this many
     * frames and the next instead of all in one. 0: drawn at once, as upstream.
     */
    standInFrames: number = 2;
    /**
     * Scenic Roads: with pacedDrapes, tiles that left the cover with their textures, kept as stand-ins
     * for their descendants still without textures (released once none needs them: prepareForRender),
     * and taken back with their textures if the cover returns to them meanwhile.
     */
    _retired: {[_: string]: Tile} = {};

    constructor(tileManager: TileManager) {
        super();
        this.tileManager = tileManager;
        this._tiles = {};
        this._renderableTilesKeys = [];
        this._sourceTileCache = {};
        this.minzoom = 0;
        this.maxzoom = 22;
        this.deltaZoom = 1;
        this.tileSize = tileManager._source.tileSize * 2 ** this.deltaZoom;
        tileManager.usedForTerrain = true;
        tileManager.tileSize = this.tileSize;
    }

    destruct(): void {
        this.tileManager.usedForTerrain = false;
        this.tileManager.tileSize = null;
        this.releaseAllRTT();
    }

    getSource(): Source {
        return this.tileManager._source;
    }

    /**
     * Load Terrain Tiles, create internal render-to-texture tiles, free GPU memory.
     * @param transform - the operation to do
     * @param terrain - the terrain
     * @returns true when the set of renderable tiles changed
     */
    update(transform: ITransform, terrain: Terrain): boolean {
        // load raster-dem tiles for the current scene.
        this.tileManager.update(transform, terrain);
        // create internal render-to-texture tiles for the current scene.
        this._renderableTilesKeys = [];
        const keys = {};
        let changed = false;
        const painter = this.tileManager.map?.painter;
        const pixelRatio = painter?.pixelRatio ?? 1;
        const fullSize = this.tileSize * (terrain?.qualityFactor ?? 1);
        const sizing = !!terrain && this.footprintSizing && this.terrainAwareZoom;
        for (const tileID of coveringTiles(transform, {
            tileSize: this.tileSize,
            minzoom: this.minzoom,
            maxzoom: this.maxzoom,
            reparseOverscaled: false,
            terrain,
            terrainAwareZoom: this.terrainAwareZoom,
            texelParity: sizing ? {texels: fullSize, pixelRatio} : undefined
        })) {
            keys[tileID.key] = true;
            this._renderableTilesKeys.push(tileID.key);
            if (!this._tiles[tileID.key] && this._retired[tileID.key]) {
                // Scenic Roads: back in the cover before its textures were released.
                this._tiles[tileID.key] = this._retired[tileID.key];
                delete this._retired[tileID.key];
                this._lastTilesetChange = now();
                changed = true;
            } else if (!this._tiles[tileID.key]) {
                tileID.terrainRttPosMatrix32f = new Float32Array(16);
                mat4.ortho(tileID.terrainRttPosMatrix32f, 0, EXTENT, EXTENT, 0, 0, 1);
                this._tiles[tileID.key] = new Tile(tileID, this.tileSize);
                this._lastTilesetChange = now();
                changed = true;
            }
        }
        // Scenic Roads: each tile's texture size by its footprint (footprintSizing)
        const camera = sizing ? cameraMercatorCoordinate(transform) : null;
        const focal = sizing ? focalLengthPx(transform, pixelRatio) : 0;
        const perMeter = sizing ? mercatorZfromAltitude(1, transform.center.lat) : 0;
        // (only tiles finer than the view's own zoom, the ones the zoom over the terrain and by depth
        // lifted, are sized: a tile at or above it keeps the full size upstream gives it)
        const centerZ = sizing ? coveringZoomLevel(transform, {tileSize: this.tileSize}) : 0;
        for (const key of this._renderableTilesKeys) {
            const tile = this._tiles[key];
            if (!sizing || tile.tileID.canonical.z <= centerZ) { tile.rttTargetSize = fullSize; continue; }
            const {minElevation, maxElevation} = terrain.getMinMaxElevation(tile.tileID);
            const px = tileFootprintPx(camera, tile.tileID.canonical, tile.tileID.wrap,
                (minElevation ?? 0) * perMeter, (maxElevation ?? 0) * perMeter, focal);
            tile.rttTargetSize = rttSizeForFootprint(px, this.minRttSize, fullSize);
        }
        // free unused tiles
        for (const key in this._tiles) {
            if (!keys[key]) {
                // Scenic Roads: kept as a stand-in a frame (pacedDrapes, standInFrames)
                const tile = this._tiles[key];
                if (this.pacedDrapes && this.standInFrames > 0 && tile.rttObjects.length) this._retired[key] = tile;
                else tile.releaseRTT(this.tileManager.map.painter);
                delete this._tiles[key];
                changed = true;
            }
        }
        return changed;
    }

    /**
     * Release the RTT objects for `tileID` (and its ancestors/descendants),
     */
    releaseRTT(tileID: OverscaledTileID): void {
        for (const key in this._tiles) {
            const tile = this._tiles[key];
            if (tile.tileID.equals(tileID) || tile.tileID.isChildOf(tileID) || tileID.isChildOf(tile.tileID)) {
                // Scenic Roads: with pacedDrapes, noted for the next frame (prepareForRender).
                if (this.pacedDrapes) this._dirtyRTT.add(key);
                else tile.releaseRTT(this.tileManager.map.painter);
            }
        }
        if (this.pacedDrapes) this.tileManager.map?.triggerRepaint();
    }

    /**
     * Release the A RTT objects for all tiles.
     */
    releaseAllRTT(): void {
        for (const key in this._tiles) {
            const tile = this._tiles[key];
            tile.releaseRTT(this.tileManager.map.painter);
        }
        this.releaseRetired(() => false);
    }

    /**
     * Scenic Roads: releases the retired tiles' textures but those `keep` names.
     */
    releaseRetired(keep: (key: string) => boolean): void {
        for (const key in this._retired) {
            if (keep(key)) continue;
            this._retired[key].releaseRTT(this.tileManager.map.painter);
            delete this._retired[key];
        }
    }

    /**
     * Scenic Roads: the nearest retired ancestor of a tile with textures, if any.
     */
    retiredAncestor(tileID: OverscaledTileID): Tile | undefined {
        for (let z = tileID.canonical.z - 1; z >= 0; z--) {
            const tile = this._retired[tileID.scaledTo(z).key];
            if (tile?.rttObjects.length) return tile;
        }
        return undefined;
    }

    /**
     * get a list of tiles, which are loaded and should be rendered in the current scene
     * @returns the renderable tiles
     */
    getRenderableTiles(): Tile[] {
        return this._renderableTilesKeys.map(key => this.getTileByID(key));
    }

    /**
     * get terrain tile by the TileID key
     * @param id - the tile id
     * @returns the tile
     */
    getTileByID(id: string): Tile {
        return this._tiles[id];
    }

    /**
     * Searches for the corresponding current renderable terrain-tiles
     * @param tileID - the tile to look for
     * @returns the tiles that were found
     */
    getTerrainCoords(
        tileID: OverscaledTileID,
        terrainTileRanges?: {[zoom: string]: CanonicalTileRange}
    ): Record<string, OverscaledTileID> {
        if (terrainTileRanges) {
            return this._getTerrainCoordsForTileRanges(tileID, terrainTileRanges);
        } else {
            return this._getTerrainCoordsForRegularTile(tileID);
        }
    }

    /**
     * Searches for the corresponding current renderable terrain-tiles.
     * Includes terrain tiles that are either:
     * - the same as the tileID
     * - a parent of the tileID
     * - a child of the tileID
     * @param tileID - the tile to look for
     * @returns the tiles that were found
     */
    _getTerrainCoordsForRegularTile(tileID: OverscaledTileID): Record<string, OverscaledTileID> {
        const coords: Record<string, OverscaledTileID> = {};
        for (const key of this._renderableTilesKeys) {
            const terrainTileID = this._tiles[key].tileID;
            const coord = tileID.clone();
            const mat = createMat4f64();
            if (terrainTileID.canonical.equals(tileID.canonical)) {
                mat4.ortho(mat, 0, EXTENT, EXTENT, 0, 0, 1);
            } else if (terrainTileID.canonical.isChildOf(tileID.canonical)) {
                const dz = terrainTileID.canonical.z - tileID.canonical.z;
                const dx = terrainTileID.canonical.x - (terrainTileID.canonical.x >> dz << dz);
                const dy = terrainTileID.canonical.y - (terrainTileID.canonical.y >> dz << dz);
                const size = EXTENT >> dz;
                mat4.ortho(mat, 0, size, size, 0, 0, 1); // Note: we are using `size` instead of `EXTENT` here
                mat4.translate(mat, mat, [-dx * size, -dy * size, 0]);
            } else if (tileID.canonical.isChildOf(terrainTileID.canonical)) {
                const dz = tileID.canonical.z - terrainTileID.canonical.z;
                const dx = tileID.canonical.x - (tileID.canonical.x >> dz << dz);
                const dy = tileID.canonical.y - (tileID.canonical.y >> dz << dz);
                const size = EXTENT >> dz;
                mat4.ortho(mat, 0, EXTENT, EXTENT, 0, 0, 1);
                mat4.translate(mat, mat, [dx * size, dy * size, 0]);
                mat4.scale(mat, mat, [1 / (2 ** dz), 1 / (2 ** dz), 0]);
            } else {
                continue;
            }
            coord.terrainRttPosMatrix32f = new Float32Array(mat);
            coords[key] = coord;
        }
        return coords;
    }

    /**
     * Searches for the corresponding current renderable terrain-tiles.
     * Includes terrain tiles that are within terrain tile ranges.
     * @param tileID - the tile to look for
     * @returns the tiles that were found
     */
    _getTerrainCoordsForTileRanges(
        tileID: OverscaledTileID,
        terrainTileRanges: {[zoom: string]: CanonicalTileRange}
    ): Record<string, OverscaledTileID> {
        const coords: Record<string, OverscaledTileID> = {};
        for (const key of this._renderableTilesKeys) {
            const terrainTileID = this._tiles[key].tileID;
            if (!this._isWithinTileRanges(terrainTileID, terrainTileRanges)) {
                continue;
            }

            const coord = tileID.clone();
            const mat = createMat4f64();
            if (terrainTileID.canonical.z === tileID.canonical.z) {
                const dx = tileID.canonical.x - terrainTileID.canonical.x
                    + tileID.wrap * (1 << tileID.canonical.z); // include wrap shift
                const dy = tileID.canonical.y - terrainTileID.canonical.y;
                mat4.ortho(mat, 0, EXTENT, EXTENT, 0, 0, 1);
                mat4.translate(mat, mat, [dx * EXTENT, dy * EXTENT, 0]);
            } else if (terrainTileID.canonical.z > tileID.canonical.z) {
                const dz = terrainTileID.canonical.z - tileID.canonical.z;
                // this translation is needed to project tileID to terrainTileID zoom level
                const dx = terrainTileID.canonical.x - (terrainTileID.canonical.x >> dz << dz)
                    + tileID.wrap * (1 << terrainTileID.canonical.z); // include wrap shift
                const dy = terrainTileID.canonical.y - (terrainTileID.canonical.y >> dz << dz);
                // this translation is needed if terrainTileID is not a parent of tileID
                const dx2 = tileID.canonical.x - (terrainTileID.canonical.x >> dz);
                const dy2 = tileID.canonical.y - (terrainTileID.canonical.y >> dz);

                const size = EXTENT >> dz;
                mat4.ortho(mat, 0, size, size, 0, 0, 1);
                mat4.translate(mat, mat, [-dx * size + dx2 * EXTENT, -dy * size + dy2 * EXTENT, 0]);
            } else { // terrainTileID.canonical.z < tileID.canonical.z
                const dz = tileID.canonical.z - terrainTileID.canonical.z;
                // this translation is needed to project tileID to terrainTileID zoom level
                const dx = tileID.canonical.x - (tileID.canonical.x >> dz << dz)
                    + tileID.wrap * (1 << tileID.canonical.z); // include wrap shift
                const dy = tileID.canonical.y - (tileID.canonical.y >> dz << dz);
                // this translation is needed if terrainTileID is not a parent of tileID
                const dx2 = (tileID.canonical.x >> dz) - terrainTileID.canonical.x;
                const dy2 = (tileID.canonical.y >> dz) - terrainTileID.canonical.y;

                const size = EXTENT << dz;
                mat4.ortho(mat, 0, size, size, 0, 0, 1);
                mat4.translate(mat, mat, [dx * EXTENT + dx2 * size, dy * EXTENT + dy2 * size, 0]);
            }
            coord.terrainRttPosMatrix32f = new Float32Array(mat);
            coords[key] = coord;
        }
        return coords;
    }

    /**
     * find the covering raster-dem tile
     * @param tileID - the tile to look for
     * @param searchForDEM - Optional parameter to search for (parent) source tiles with loaded dem.
     * @returns the tile
     */
    getSourceTile(tileID: OverscaledTileID, searchForDEM?: boolean): Tile | undefined {
        const source = this.tileManager._source;
        let z = tileID.overscaledZ - this.deltaZoom;
        if (z > source.maxzoom) z = source.maxzoom;
        if (z < source.minzoom) return undefined;
        // cache for tileID to terrain-tileID
        this._sourceTileCache[tileID.key] ||= tileID.scaledTo(z).key;
        let tile = this.findTileInCaches(this._sourceTileCache[tileID.key]);
        // during tile-loading phase look if parent tiles (with loaded dem) are available.
        if (!tile?.dem && searchForDEM) {
            while (z >= source.minzoom && !tile?.dem)
                tile = this.findTileInCaches(tileID.scaledTo(z--).key);
        }
        return tile;
    }

    findTileInCaches(key: string): Tile | undefined {
        let tile = this.tileManager.getTileByID(key);
        if (tile) {
            return tile;
        }
        tile = this.tileManager._outOfViewCache.getByKey(key);
        return tile;
    }

    /**
     * gets whether any tiles were loaded after a specific time. This is used to update the depth framebuffer.
     * @param time - the time
     * @returns true if any tiles came into view at or after the specified time
     */
    anyTilesAfterTime(time: number = now()): boolean {
        return this._lastTilesetChange >= time;
    }

    /**
     * Checks whether a tile is within the canonical tile ranges.
     * @param tileID - Tile to check
     * @param canonicalTileRanges - Canonical tile ranges
     * @returns
     */
    private _isWithinTileRanges(
        tileID: OverscaledTileID,
        canonicalTileRanges: {[zoom: string]: CanonicalTileRange}
    ): boolean {
        const range = canonicalTileRanges[tileID.canonical.z];

        return !!range && (
            tileID.wrap > range.minWrap || tileID.wrap < range.maxWrap ||
            tileID.canonical.x >= range.minTileXWrapped &&
            tileID.canonical.x <= range.maxTileXWrapped &&
            tileID.canonical.y >= range.minTileY &&
            tileID.canonical.y <= range.maxTileY
        );
    }
}
