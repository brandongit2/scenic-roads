import {
    Uniform1i,
    Uniform1f,
    Uniform2f,
    Uniform3f,
    Uniform4f,
    UniformColor
} from '../uniform_binding.ts';
import {Color} from '@maplibre/maplibre-gl-style-spec';
import {fogDepth} from '../../style/sky.ts';

import type {FogTransform, Sky} from '../../style/sky.ts';
import type {UniformValues, UniformLocations} from '../uniform_binding.ts';
import type {Context} from '../../webgl/context.ts';

export type TerrainPreludeUniformsType = {
    'u_depth': Uniform1i;
    'u_terrain': Uniform1i;
};

// Scenic Roads: the distance fog (Sky#fogRange); the extrusions take the same uniforms.
export type FogUniformsType = {
    'u_fog_depth': Uniform2f;
    'u_fog': Uniform3f;
    'u_fog_color': UniformColor;
    'u_horizon_color': UniformColor;
    'u_horizon_fog_blend': Uniform1f;
};

export type TerrainUniformsType = {
    'u_texture': Uniform1i;
    'u_ele_delta': Uniform1f;
    // Scenic Roads: the part of the texture the tile draws (u, v offset and scale; a stand-in's
    // part of an ancestor's texture: render_to_texture.ts _pickStandIns), else 0, 0, 1, 1.
    'u_texture_rect': Uniform4f;
} & FogUniformsType;

export type TerrainDepthUniformsType = {
    'u_ele_delta': Uniform1f;
};

const terrainPreludeUniforms = (context: Context, locations: UniformLocations): TerrainPreludeUniformsType => ({
    'u_depth': new Uniform1i(context, locations.u_depth),
    'u_terrain': new Uniform1i(context, locations.u_terrain)
});

const fogUniforms = (context: Context, locations: UniformLocations): FogUniformsType => ({
    'u_fog_depth': new Uniform2f(context, locations.u_fog_depth),
    'u_fog': new Uniform3f(context, locations.u_fog),
    'u_fog_color': new UniformColor(context, locations.u_fog_color),
    'u_horizon_color': new UniformColor(context, locations.u_horizon_color),
    'u_horizon_fog_blend': new Uniform1f(context, locations.u_horizon_fog_blend),
});

const terrainUniforms = (context: Context, locations: UniformLocations): TerrainUniformsType => ({
    'u_texture': new Uniform1i(context, locations.u_texture),
    'u_ele_delta': new Uniform1f(context, locations.u_ele_delta),
    'u_texture_rect': new Uniform4f(context, locations.u_texture_rect),
    ...fogUniforms(context, locations),
});

const terrainDepthUniforms = (context: Context, locations: UniformLocations): TerrainDepthUniformsType => ({
    'u_ele_delta': new Uniform1f(context, locations.u_ele_delta)
});

/**
 * The distance fog's uniforms (Sky#fogRange): `u_fog_depth` (sky.ts fogDepth); `u_fog` the start, end
 * and opacity (none without a sky, or with a transparent fog colour).
 */
const fogUniformValues = (sky: Sky | null | undefined, tr: FogTransform): UniformValues<FogUniformsType> => {
    const fogColor = sky ? sky.properties.get('fog-color') : Color.transparent;
    const [start, end, opacity] = sky ? sky.fogUniform() : [0, 1, 0];
    return {
        'u_fog_depth': fogDepth(tr),
        'u_fog': [start, end, opacity * fogColor.a],
        'u_fog_color': fogColor,
        'u_horizon_color': sky ? sky.properties.get('horizon-color') : Color.transparent,
        'u_horizon_fog_blend': sky ? sky.properties.get('horizon-fog-blend') : 1,
    };
};

const terrainUniformValues = (
    eleDelta: number,
    sky: Sky | null | undefined,
    tr: FogTransform,
    textureRect: [number, number, number, number] = [0, 0, 1, 1]): UniformValues<TerrainUniformsType> => ({
    'u_texture': 0,
    'u_ele_delta': eleDelta,
    'u_texture_rect': textureRect,
    ...fogUniformValues(sky, tr),
});

const terrainDepthUniformValues = (
    eleDelta: number
): UniformValues<TerrainDepthUniformsType> => ({
    'u_ele_delta': eleDelta
});

export {terrainUniforms, terrainDepthUniforms, terrainPreludeUniforms, terrainUniformValues, terrainDepthUniformValues, fogUniforms, fogUniformValues};
