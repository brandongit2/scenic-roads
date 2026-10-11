import {patternUniformValues} from './pattern.ts';
import {
    Uniform1i,
    Uniform1f,
    Uniform2f,
    Uniform3f,
} from '../uniform_binding.ts';
import {extend} from '../../util/util.ts';

import type {Painter} from '../../render/painter.ts';
import type {UniformValues, UniformLocations} from '../uniform_binding.ts';
import type {Context} from '../../webgl/context.ts';
import type {CrossfadeParameters} from '../../style/evaluation_parameters.ts';
import type {Tile} from '../../tile/tile.ts';

export type FillUniformsType = {
    'u_fill_translate': Uniform2f;
    'u_elevation_reference': Uniform1f;
};

export type FillOutlineUniformsType = {
    'u_fill_translate': Uniform2f;
    'u_elevation_reference': Uniform1f;
};

export type FillPatternUniformsType = {
    // pattern uniforms:
    'u_texsize': Uniform2f;
    'u_image': Uniform1i;
    'u_pixel_coord_upper': Uniform2f;
    'u_pixel_coord_lower': Uniform2f;
    'u_scale': Uniform3f;
    'u_fade': Uniform1f;
    'u_sdf_pattern': Uniform1i;
    'u_fill_translate': Uniform2f;
    'u_elevation_reference': Uniform1f;
};

export type FillOutlinePatternUniformsType = {
    // pattern uniforms:
    'u_texsize': Uniform2f;
    'u_image': Uniform1i;
    'u_pixel_coord_upper': Uniform2f;
    'u_pixel_coord_lower': Uniform2f;
    'u_scale': Uniform3f;
    'u_fade': Uniform1f;
    'u_sdf_pattern': Uniform1i;
    'u_fill_translate': Uniform2f;
    'u_elevation_reference': Uniform1f;
};

const fillUniforms = (context: Context, locations: UniformLocations): FillUniformsType => ({
    'u_elevation_reference': new Uniform1f(context, locations.u_elevation_reference),
    'u_fill_translate': new Uniform2f(context, locations.u_fill_translate)
});

const fillPatternUniforms = (context: Context, locations: UniformLocations): FillPatternUniformsType => ({
    'u_image': new Uniform1i(context, locations.u_image),
    'u_texsize': new Uniform2f(context, locations.u_texsize),
    'u_pixel_coord_upper': new Uniform2f(context, locations.u_pixel_coord_upper),
    'u_pixel_coord_lower': new Uniform2f(context, locations.u_pixel_coord_lower),
    'u_scale': new Uniform3f(context, locations.u_scale),
    'u_fade': new Uniform1f(context, locations.u_fade),
    'u_sdf_pattern': new Uniform1i(context, locations.u_sdf_pattern),
    'u_elevation_reference': new Uniform1f(context, locations.u_elevation_reference),
    'u_fill_translate': new Uniform2f(context, locations.u_fill_translate)
});

const fillOutlineUniforms = (context: Context, locations: UniformLocations): FillOutlineUniformsType => ({
    'u_elevation_reference': new Uniform1f(context, locations.u_elevation_reference),
    'u_fill_translate': new Uniform2f(context, locations.u_fill_translate)
});

const fillOutlinePatternUniforms = (context: Context, locations: UniformLocations): FillOutlinePatternUniformsType => ({
    'u_image': new Uniform1i(context, locations.u_image),
    'u_texsize': new Uniform2f(context, locations.u_texsize),
    'u_pixel_coord_upper': new Uniform2f(context, locations.u_pixel_coord_upper),
    'u_pixel_coord_lower': new Uniform2f(context, locations.u_pixel_coord_lower),
    'u_scale': new Uniform3f(context, locations.u_scale),
    'u_fade': new Uniform1f(context, locations.u_fade),
    'u_sdf_pattern': new Uniform1i(context, locations.u_sdf_pattern),
    'u_elevation_reference': new Uniform1f(context, locations.u_elevation_reference),
    'u_fill_translate': new Uniform2f(context, locations.u_fill_translate)
});

const fillPatternUniformValues = (
    painter: Painter,
    crossfade: CrossfadeParameters,
    tile: Tile,
    translate: [number, number],
    isSdfPattern: boolean,
    elevationReference: number = 0
): UniformValues<FillPatternUniformsType> => extend(
    patternUniformValues(crossfade, painter, tile),
    {
        'u_fill_translate': translate,
        'u_elevation_reference': elevationReference,
        'u_sdf_pattern': isSdfPattern ? 1 : 0,
    }
);

// (Scenic Roads: elevationReference 0 draped or flat, 1 above the ground, 2 above sea level:
// FillStyleLayer.elevationReference.)
const fillUniformValues = (translate: [number, number], elevationReference: number = 0): UniformValues<FillUniformsType> => ({
    'u_fill_translate': translate,
    'u_elevation_reference': elevationReference,
});

const fillOutlineUniformValues = (translate: [number, number], elevationReference: number = 0): UniformValues<FillOutlineUniformsType> => ({
    'u_fill_translate': translate,
    'u_elevation_reference': elevationReference,
});

const fillOutlinePatternUniformValues = (
    painter: Painter,
    crossfade: CrossfadeParameters,
    tile: Tile,
    translate: [number, number],
    isSdfPattern: boolean,
    elevationReference: number = 0
): UniformValues<FillOutlinePatternUniformsType> => fillPatternUniformValues(painter, crossfade, tile, translate, isSdfPattern, elevationReference);

export {
    fillUniforms,
    fillPatternUniforms,
    fillOutlineUniforms,
    fillOutlinePatternUniforms,
    fillUniformValues,
    fillPatternUniformValues,
    fillOutlineUniformValues,
    fillOutlinePatternUniformValues
};
