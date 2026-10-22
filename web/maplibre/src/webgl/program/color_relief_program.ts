import {
    Uniform1i,
    Uniform1f,
    Uniform2f,
    Uniform3f,
    Uniform4f
} from '../uniform_binding.ts';

import type {Context} from '../../webgl/context.ts';
import type {UniformValues, UniformLocations} from '../uniform_binding.ts';
import type {ColorReliefStyleLayer} from '../../style/style_layer/color_relief_style_layer.ts';
import type {DEMData} from '../../data/dem_data.ts';
import type {MaskCell} from '../draw/water_mask.ts';

export type ColorReliefUniformsType = {
    'u_image': Uniform1i;
    'u_unpack': Uniform4f;
    'u_dimension': Uniform2f;
    'u_elevation_stops': Uniform1i;
    'u_color_stops': Uniform1i;
    'u_color_ramp_size': Uniform1i;
    'u_opacity': Uniform1f;
    'u_cell': Uniform3f;
    'u_mask_cell': Uniform3f;
    'u_mask': Uniform1i;
    'u_masked': Uniform1f;
};

const colorReliefUniforms = (context: Context, locations: UniformLocations): ColorReliefUniformsType => ({
    'u_image': new Uniform1i(context, locations.u_image),
    'u_unpack': new Uniform4f(context, locations.u_unpack),
    'u_dimension': new Uniform2f(context, locations.u_dimension),
    'u_elevation_stops': new Uniform1i(context, locations.u_elevation_stops),
    'u_color_stops': new Uniform1i(context, locations.u_color_stops),
    'u_color_ramp_size': new Uniform1i(context, locations.u_color_ramp_size),
    'u_opacity': new Uniform1f(context, locations.u_opacity),
    'u_cell': new Uniform3f(context, locations.u_cell),
    'u_mask_cell': new Uniform3f(context, locations.u_mask_cell),
    'u_mask': new Uniform1i(context, locations.u_mask),
    'u_masked': new Uniform1f(context, locations.u_masked)
});

const colorReliefUniformValues = (
    layer: ColorReliefStyleLayer,
    dem: DEMData,
    colorRampSize: number = 0,
    cell?: MaskCell
): UniformValues<ColorReliefUniformsType> => {

    return {
        'u_image': 0,
        'u_unpack': dem.getUnpackVector(),
        'u_dimension': [dem.stride, dem.stride],
        'u_elevation_stops': 1,
        'u_color_stops': 4,
        'u_color_ramp_size': colorRampSize,
        'u_opacity': layer.paint.get('color-relief-opacity'),
        'u_cell': cell ? cell.demCell : [0, 0, 1],
        'u_mask_cell': cell ? cell.maskCell : [0, 0, 1],
        'u_mask': 2,
        'u_masked': cell ? 1 : 0
    };
};

export {
    colorReliefUniforms,
    colorReliefUniformValues,
};
