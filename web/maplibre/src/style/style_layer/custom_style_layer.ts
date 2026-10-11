import {StyleLayer} from '../style_layer.ts';
import {ValidationError} from '@maplibre/maplibre-gl-style-spec';

import type {Map} from '../../ui/map.ts';
import type {mat4} from 'gl-matrix';
import type {LayerSpecification} from '@maplibre/maplibre-gl-style-spec';
import type {CustomLayerProjectionData, RendererProjectionData} from '../../geo/projection/projection_data.ts';

/**
 * Type for an object literal that specifies a map tile.
 */
export type UnwrappedTileIDLiteral = {
    /**
     * An optional wrap values.
     * Useful in scenarios when multiple world copies are visible, such as a zoomed out map or a map centered around the antimeridian.
     * Tiles from each world copy should have different wrap values, with wrap increasing for each copy from west to east.
     */
    wrap?: number;
    /**
     * The tile's XY coordinates and zoom level.
     */
    canonical: {
        x: number;
        y: number;
        z: number;
    };
};

/**
 * Parameters object for the {@link CustomRenderMethodInput.getProjectionData} function.
 * Contains the requested tile ID and more.
 */
export type CustomLayerProjectionDataParams = {
    /**
     * The coordinates of the current tile.
     */
    tileID: UnwrappedTileIDLiteral | null;
    /**
     * Set to true if a pixel-aligned matrix should be used, if possible.
     * This flag is mostly used for raster tiles under mercator projection.
     */
    aligned?: boolean;
    /**
     * Set to true if the terrain matrix should be applied when pre-rendering tiles into textures for 3D terrain.
     */
    applyTerrainMatrix?: boolean;
    /**
     * Set to true if the globe matrix should be applied when using globe projection.
     */
    applyGlobeMatrix?: boolean;
};

/**
* Input arguments exposed by custom render function.
*/
export type CustomRenderMethodInput = {
    /**
     * This value represents the distance from the camera to the far clipping plane.
     * It is used in the calculation of the projection matrix to determine which objects are visible.
     * farZ should be larger than nearZ.
     */
    farZ: number;
    /**
     * This value represents the distance from the camera to the near clipping plane.
     * It is used in the calculation of the projection matrix to determine which objects are visible.
     * nearZ should be smaller than farZ.
     */
    nearZ: number;
    /**
     * Vertical field of view in radians.
     */
    fov: number;
    /**
    * Model view projection matrix.
    * Represents the matrix converting from world space to clip space.
    * https://learnopengl.com/Getting-started/Coordinate-Systems
    * **/
    modelViewProjectionMatrix: mat4;
    /**
    * Projection matrix.
    * Represents the matrix converting from view space to clip space.
    * https://learnopengl.com/Getting-started/Coordinate-Systems
    */
    projectionMatrix: mat4;
    /**
     * Data required for picking and compiling a custom shader for the current projection.
     */
    shaderData: {
        /**
         * Name of the shader variant that should be used.
         * Depends on current projection.
         * Whenever the other shader properties change, this string changes as well,
         * and can be used as a key with which to cache compiled shaders.
         */
        variantName: string;
        /**
         * The prelude code to add to the vertex shader to access MapLibre's `projectTile` projection function.
         * Depends on current projection.
         * @example
         * ```
         * const vertexSource = `#version 300 es
         * ${shaderData.vertexShaderPrelude}
         * ${shaderData.define}
         * in vec2 a_pos;
         * void main() {
         *     gl_Position = projectTile(a_pos);
         * }`;
         * ```
         */
        vertexShaderPrelude: string;
        /**
         * Defines to add to the shader code.
         * Depends on current projection.
         * @example
         * ```
         * const vertexSource = `#version 300 es
         * ${shaderData.vertexShaderPrelude}
         * ${shaderData.define}
         * in vec2 a_pos;
         * void main() {
         *     gl_Position = projectTile(a_pos);
         *     #ifdef GLOBE
         *     // Do globe-specific things
         *     #endif
         * }`;
         * ```
         */
        define: string;
    };
    /**
     * Uniforms that should be passed to the vertex shader, if MapLibre's projection code is used.
     * For more details of this object's internals, see its doc comments in `src/geo/projection/projection_data.ts`.
     *
     * These uniforms are set so that `projectTile` in shader accepts a vec2 in range 0..1 in web mercator coordinates.
     * Use `getProjectionData({overscaledTileID: tileID})` to get uniforms for a given tile and pass vec2 in tile-local range 0..EXTENT instead.
     *
     * For projection 3D features, use `projectTileFor3D` in the shader.
     *
     * If you just need a projection matrix, use `defaultProjectionData.mainMatrix`.
     * A projection matrix is sufficient for simple custom layers that only support mercator projection.
     *
     * Under mercator projection, when these uniforms are used, the shader's `projectTile` function projects spherical mercator
     * coordinates to gl clip space coordinates. The spherical mercator coordinate `[0, 0]` represents the
     * top left corner of the mercator world and `[1, 1]` represents the bottom right corner. When
     * the `renderingMode` is `"3d"`, the z coordinate is conformal. A box with identical x, y, and z
     * lengths in mercator units would be rendered as a cube. {@link MercatorCoordinate.fromLngLat}
     * can be used to project a `LngLat` to a mercator coordinate.
     *
     * Under globe projection, when these uniforms are used, the `elevation` parameter
     * passed to `projectTileFor3D` in the shader is elevation in meters above "sea level",
     * or more accurately for globe, elevation above the surface of the perfect sphere used to render the planet.
     */
    defaultProjectionData: CustomLayerProjectionData;

    /**
     * Generates a {@link ProjectionData} instance to be used while rendering a given tile.
     * In custom layers, this function is only needed when rendering tiles in a completely custom way and with shaders that are compatible with both projections.
     *
     * @see [Add a custom layer with tiles to a globe](https://maplibre.org/maplibre-gl-js/docs/examples/add-a-custom-layer-with-tiles-to-a-globe)
     * @param params - Parameters for the projection data generation.
     */
    getProjectionData: (params: CustomLayerProjectionDataParams) => RendererProjectionData;

    /**
     * Scenic Roads: the framebuffer this call draws into (`null`: the canvas). A layer that binds
     * framebuffers of its own binds this one again before it returns, never `null` blindly.
     */
    framebuffer: WebGLFramebuffer | null;

    /**
     * Scenic Roads: the depth range MapLibre's 3D layers draw in (what `gl.depthRange` is set to for
     * a `"3d"` layer's `render`): the frame's 3D window depths lie in it.
     */
    depthRangeFor3D: [number, number];

    /**
     * Scenic Roads: the terrain's depth, while 3D terrain is on (else `null`): MapLibre draws the
     * terrain's mesh into this texture whenever the camera moves or terrain tiles arrive, for the
     * symbols' and circles' visibility. It is read in place (no copy is made for the layer). The
     * depth is packed into RGBA8; `unpack` is a GLSL function that reads it at a point of the
     * frame (`uv` 0..1 across the frame, `gl_FragCoord.xy / viewport size`), giving the terrain's
     * normalized device depth there (`gl_Position.z / gl_Position.w` of `projectTileFor3D`), about
     * 1 where there's no terrain.
     */
    terrainDepth: CustomTerrainDepth | null;

    /**
     * Scenic Roads: the frame's depth so far, a `DEPTH24_STENCIL8` texture of the drawing buffer's
     * size: in `render`, for a layer whose `readsSceneDepth` is true (else `null`). Everything drawn
     * before the layer wrote its depth here (the terrain's mesh, the extrusions, earlier 3D layers).
     * The layer draws into `framebuffer`, which has the frame's colour but no depth attached
     * (sampling a texture attached to the framebuffer drawn into is an error in WebGL), so it tests
     * depth in its shader: `if (gl_FragCoord.z > texelFetch(depth, ivec2(gl_FragCoord.xy), 0).r) discard;`.
     */
    sceneDepth: CustomSceneDepth | null;
};

/**
 * Scenic Roads: the terrain's depth texture, see {@link CustomRenderMethodInput.terrainDepth}.
 */
export type CustomTerrainDepth = {
    texture: WebGLTexture;
    /** Its size in pixels (the frame's size in CSS pixels). */
    width: number;
    height: number;
    /** Changes whenever the texture is drawn again. */
    revision: number;
    /** GLSL: `highp float unpackTerrainDepth(highp sampler2D depth, vec2 uv)`. */
    unpack: string;
};

/**
 * Scenic Roads: the frame's depth so far, see {@link CustomRenderMethodInput.sceneDepth}.
 */
export type CustomSceneDepth = {
    texture: WebGLTexture;
    /** Its size in pixels (the drawing buffer's). */
    width: number;
    height: number;
};

/**
 * Scenic Roads: the input of {@link CustomLayerInterface.drape}: the frame's, and the terrain tile
 * whose texture is being drawn.
 */
export type CustomDrapeMethodInput = CustomRenderMethodInput & {
    /** The terrain tile whose texture is drawn into. */
    tileID: {z: number; x: number; y: number; wrap: number};
    /** The texture's size in pixels (the viewport is set to it). */
    size: number;
    /**
     * The matrix that takes a tile's coordinates (0..8192 across, any zoom) to clip space in this
     * texture: `gl_Position = matrix * vec4(posInTile, 0.0, 1.0)`. `null` for a tile that doesn't
     * meet this terrain tile.
     */
    getDrapeMatrix: (tile: {z: number; x: number; y: number}) => Float32Array | null;
};

/**
 * @param gl - The map's gl context.
 * @param options - Argument object with render inputs like camera properties.
 */
export type CustomRenderMethod = (gl: WebGL2RenderingContext, options: CustomRenderMethodInput) => void;

/**
 * Scenic Roads: see {@link CustomLayerInterface.drape}.
 */
export type CustomDrapeMethod = (gl: WebGL2RenderingContext, options: CustomDrapeMethodInput) => void;

/**
 * Interface for custom style layers. This is a specification for
 * implementers to model: it is not an exported method or class.
 *
 * Custom layers allow a user to render directly into the map's GL context using the map's camera.
 * These layers can be added between any regular layers using {@link Map.addLayer}.
 *
 * Custom layers must have a unique `id` and must have the `type` of `"custom"`.
 * They must implement `render` and may implement `prerender`, `onAdd` and `onRemove`.
 * They can trigger rendering using {@link Map.triggerRepaint}
 * and they should appropriately handle {@link MapContextEvent} with `webglcontextlost` and `webglcontextrestored`.
 *
 * The `renderingMode` property controls whether the layer is treated as a `"2d"` or `"3d"` map layer. Use:
 *
 * - `"renderingMode": "3d"` to use the depth buffer and share it with other layers
 * - `"renderingMode": "2d"` to add a layer with no depth. If you need to use the depth buffer for a `"2d"` layer you must use an offscreen
 *   framebuffer and {@link CustomLayerInterface.prerender}
 *
 * @example
 * Custom layer implemented as ES6 class
 * ```ts
 * class NullIslandLayer {
 *     constructor() {
 *         this.id = 'null-island';
 *         this.type = 'custom';
 *         this.renderingMode = '2d';
 *     }
 *
 *      onAdd(map: maplibregl.Map, gl: WebGL2RenderingContext) {
 *         const vertexSource = `
 *         uniform mat4 u_matrix;
 *         void main() {
 *             gl_Position = u_matrix * vec4(0.5, 0.5, 0.0, 1.0);
 *             gl_PointSize = 20.0;
 *         }`;
 *
 *         const fragmentSource = `
 *         void main() {
 *             fragColor = vec4(1.0, 0.0, 0.0, 1.0);
 *         }`;
 *
 *         const vertexShader = gl.createShader(gl.VERTEX_SHADER);
 *         gl.shaderSource(vertexShader, vertexSource);
 *         gl.compileShader(vertexShader);
 *         const fragmentShader = gl.createShader(gl.FRAGMENT_SHADER);
 *         gl.shaderSource(fragmentShader, fragmentSource);
 *         gl.compileShader(fragmentShader);
 *
 *         this.program = gl.createProgram();
 *         gl.attachShader(this.program, vertexShader);
 *         gl.attachShader(this.program, fragmentShader);
 *         gl.linkProgram(this.program);
 *     }
 *
 *     render({
 *      gl,
 *      modelViewProjectionMatrix: matrix
 *      }: {
 *      gl: WebGL2RenderingContext;
 *      modelViewProjectionMatrix: Float32Array;
 *      }) {
 *         gl.useProgram(this.program);
 *         gl.uniformMatrix4fv(gl.getUniformLocation(this.program, "u_matrix"), false, matrix);
 *         gl.drawArrays(gl.POINTS, 0, 1);
 *     }
 * }
 *
 * map.on('load', () => {
 *     map.addLayer(new NullIslandLayer());
 * });
 * ```
 */
export interface CustomLayerInterface {
    /**
     * A unique layer id.
     */
    id: string;
    /**
     * The layer's type. Must be `"custom"`.
     */
    type: 'custom';
    /**
     * Either `"2d"` or `"3d"`. Defaults to `"2d"`.
     */
    renderingMode?: '2d' | '3d';
    /**
     * Called during a render frame allowing the layer to draw into the GL context.
     *
     * The layer can assume blending and depth state is set to allow the layer to properly
     * blend and clip other layers. The layer cannot make any other assumptions about the
     * current GL state.
     *
     * If the layer needs to render to a texture, it should implement the `prerender` method
     * to do this and only use the `render` method for drawing directly into the main framebuffer.
     *
     * The blend function is set to `gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA)`. This expects
     * colors to be provided in premultiplied alpha form where the `r`, `g` and `b` values are already
     * multiplied by the `a` value. If you are unable to provide colors in premultiplied form you
     * may want to change the blend function to
     * `gl.blendFuncSeparate(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA, gl.ONE, gl.ONE_MINUS_SRC_ALPHA)`.
     */
    render: CustomRenderMethod;
    /**
     * Optional method called during a render frame to allow a layer to prepare resources or render into a texture.
     *
     * The layer cannot make any assumptions about the current GL state and must bind a framebuffer before rendering.
     */
    prerender?: CustomRenderMethod;
    /**
     * Scenic Roads: optional method that draws the layer onto the 3D terrain, as MapLibre drapes its
     * fills and lines: while 3D terrain is on, it's called during the terrain's pass, once for each
     * terrain tile whose texture is drawn, into that texture (the viewport set to it, blending as
     * for `render`, no depth), in the layer's order among the draped layers. The textures are kept
     * from frame to frame: they're drawn again when the tiles in view or the zoom change, or when
     * `drapeRevision` does. Without 3D terrain it isn't called.
     *
     * With 3D terrain `render` is still called, in the main pass right after the terrain carrying
     * this layer's drape is drawn (and before the layers above it), for whatever the layer draws
     * above the ground; a layer with nothing more to draw there returns from it at once.
     */
    drape?: CustomDrapeMethod;
    /**
     * Scenic Roads: what the layer's drape shows: a new value has the terrain's textures holding it
     * drawn again (`drape` called for every tile) in the next frame.
     */
    drapeRevision?: number;
    /**
     * Scenic Roads: the layer reads the frame's depth in `render` ({@link CustomRenderMethodInput.sceneDepth}).
     * While a visible layer asks, MapLibre draws the frame into textures and copies its colour to the
     * canvas at the end (a blit of the frame's colour, once a frame); while none asks, it draws
     * straight to the canvas.
     */
    readsSceneDepth?: boolean;
    /**
     * Optional method called when the layer has been added to the Map with {@link Map.addLayer}. This
     * gives the layer a chance to initialize gl resources and register event listeners.
     *
     * @param map - The Map this custom layer was just added to.
     * @param gl - The gl context for the map.
     */
    onAdd?(map: Map, gl: WebGL2RenderingContext): void;
    /**
     * Optional method called when the layer has been removed from the Map with {@link Map.removeLayer}. This
     * gives the layer a chance to clean up gl resources and event listeners.
     *
     * @param map - The Map this custom layer was just added to.
     * @param gl - The gl context for the map.
     */
    onRemove?(map: Map, gl: WebGL2RenderingContext): void;
}

export function validateCustomStyleLayer(layerObject: CustomLayerInterface): ValidationError[] {
    const errors: ValidationError[] = [];
    const id = layerObject.id;

    if (id === undefined) {
        errors.push(new ValidationError(`layers.${id}`, null, 'missing required property "id"'));
    }

    if (layerObject.render === undefined) {
        errors.push(new ValidationError(`layers.${id}`, null, 'missing required method "render"'));
    }

    if (layerObject.drape !== undefined && typeof layerObject.drape !== 'function') {
        errors.push(new ValidationError(`layers.${id}`, null, 'property "drape" must be a function'));
    }

    if (layerObject.renderingMode &&
        layerObject.renderingMode !== '2d' &&
        layerObject.renderingMode !== '3d') {
        errors.push(new ValidationError(`layers.${id}`, null, 'property "renderingMode" must be either "2d" or "3d"'));
    }

    return errors;
}

export const isCustomStyleLayer = (layer: StyleLayer): layer is CustomStyleLayer => layer.type === 'custom';

export class CustomStyleLayer extends StyleLayer {

    implementation: CustomLayerInterface;

    constructor(implementation: CustomLayerInterface, globalState: Record<string, any>) {
        super(implementation, {}, globalState);
        this.implementation = implementation;
    }

    is3D(): boolean {
        return this.implementation.renderingMode === '3d';
    }

    hasOffscreenPass(): boolean {
        return this.implementation.prerender !== undefined;
    }

    recalculate(): void {}
    updateTransitions(): void {}
    hasTransition(): boolean { return false; }

    serialize(): LayerSpecification {
        throw new Error('Custom layers cannot be serialized');
    }

    onAdd: (map: Map) => void = (map: Map) => {
        if (this.implementation.onAdd) {
            this.implementation.onAdd(map, map.painter.context.gl);
        }
    };

    onRemove: (map: Map) => void = (map: Map) => {
        if (this.implementation.onRemove) {
            this.implementation.onRemove(map, map.painter.context.gl);
        }
    };
}
