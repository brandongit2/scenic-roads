import {StyleLayer} from '../style_layer.ts';
import properties, {type RasterPaintPropsPossiblyEvaluated} from './raster_style_layer_properties.g.ts';

import type {Transitionable, Transitioning, PossiblyEvaluated} from '../properties.ts';
import type {RasterPaintProps} from './raster_style_layer_properties.g.ts';
import type {LayerSpecification} from '@maplibre/maplibre-gl-style-spec';

export const isRasterStyleLayer = (layer: StyleLayer): layer is RasterStyleLayer => layer.type === 'raster';

export class RasterStyleLayer extends StyleLayer {
    _transitionablePaint: Transitionable<RasterPaintProps>;
    _transitioningPaint: Transitioning<RasterPaintProps>;
    paint: PossiblyEvaluated<RasterPaintProps, RasterPaintPropsPossiblyEvaluated>;

    /**
     * Scenic Roads: how the raster is drawn: 0 draped on the terrain (or flat on the map), as
     * upstream; 1 in 3D, `raster-z-offset` above the ground under each vertex of its tiles' meshes;
     * 2 in 3D, a level surface `raster-z-offset` above sea level (style_spec_extensions.ts): the
     * app's water raster at the water's surface over a sea floor.
     */
    elevationReference(): number {
        if (this.paint.get('raster-elevation-reference') === 'sea') return 2;
        return this.paint.get('raster-z-offset') !== 0 ? 1 : 0;
    }

    is3D(): boolean {
        return this.elevationReference() > 0;
    }

    constructor(layer: LayerSpecification, globalState: Record<string, any>) {
        super(layer, properties, globalState);
    }
}
