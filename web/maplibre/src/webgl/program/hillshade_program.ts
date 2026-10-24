import {
    Uniform1i,
    Uniform1f,
    Uniform3f,
    UniformColor,
    UniformFloatArray,
    UniformColorArray
} from '../uniform_binding.ts';

import type {Context} from '../../webgl/context.ts';
import type {UniformValues, UniformLocations} from '../uniform_binding.ts';
import type {Painter} from '../../render/painter.ts';
import type {HillshadeStyleLayer} from '../../style/style_layer/hillshade_style_layer.ts';
import type {MaskCell} from '../draw/water_mask.ts';

export type HillshadeUniformsType = {
    'u_image': Uniform1i;
    'u_exaggeration': Uniform1f;
    'u_altitudes': UniformFloatArray;
    'u_azimuths': UniformFloatArray;
    'u_accent': UniformColor;
    'u_method': Uniform1i;
    'u_shadows': UniformColorArray;
    'u_highlights': UniformColorArray;
    'u_cell': Uniform3f;
    'u_mask_cell': Uniform3f;
    'u_mask': Uniform1i;
    'u_masked': Uniform1f;
};

const hillshadeUniforms = (context: Context, locations: UniformLocations): HillshadeUniformsType => ({
    'u_image': new Uniform1i(context, locations.u_image),
    'u_exaggeration': new Uniform1f(context, locations.u_exaggeration),
    'u_altitudes': new UniformFloatArray(context, locations.u_altitudes),
    'u_azimuths': new UniformFloatArray(context, locations.u_azimuths),
    'u_accent': new UniformColor(context, locations.u_accent),
    'u_method': new Uniform1i(context, locations.u_method),
    'u_shadows': new UniformColorArray(context, locations.u_shadows),
    'u_highlights': new UniformColorArray(context, locations.u_highlights),
    'u_cell': new Uniform3f(context, locations.u_cell),
    'u_mask_cell': new Uniform3f(context, locations.u_mask_cell),
    'u_mask': new Uniform1i(context, locations.u_mask),
    'u_masked': new Uniform1f(context, locations.u_masked)
});

const hillshadeUniformValues = (
    painter: Painter,
    layer: HillshadeStyleLayer,
    cell?: MaskCell
): UniformValues<HillshadeUniformsType> => {
    const accent = layer.paint.get('hillshade-accent-color');
    let method;
    switch (layer.paint.get('hillshade-method')) {
        case 'basic':
            method = 4;
            break;
        case 'combined':
            method = 1;
            break;
        case 'igor':
            method = 2;
            break;
        case 'multidirectional':
            method = 3;
            break;
        case 'standard':
        default:
            method = 0;
            break;
    }

    const illumination = layer.getIlluminationProperties();

    for (let i = 0; i < illumination.directionRadians.length; i++) {
        // modify azimuthal angle by map rotation if light is anchored at the viewport
        if (layer.paint.get('hillshade-illumination-anchor') === 'viewport') {
            illumination.directionRadians[i] += painter.transform.bearingInRadians;
        }
    }
    return {
        'u_image': 0,
        'u_exaggeration': layer.paint.get('hillshade-exaggeration'),
        'u_altitudes': illumination.altitudeRadians,
        'u_azimuths': illumination.directionRadians,
        'u_accent': accent,
        'u_method': method,
        'u_highlights': illumination.highlightColor,
        'u_shadows': illumination.shadowColor,
        'u_cell': cell ? cell.demCell : [0, 0, 1],
        'u_mask_cell': cell ? cell.maskCell : [0, 0, 1],
        'u_mask': 2,
        'u_masked': cell ? 1 : 0
    };
};

export {
    hillshadeUniforms,
    hillshadeUniformValues
};
