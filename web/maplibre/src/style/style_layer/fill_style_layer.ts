import {type QueryIntersectsFeatureParams, StyleLayer} from '../style_layer.ts';
import {FillBucket} from '../../data/bucket/fill_bucket.ts';
import {polygonIntersectsMultiPolygon} from '../../util/intersection_tests.ts';
import {translateDistance, translate} from '../query_utils.ts';
import properties, {type FillLayoutPropsPossiblyEvaluated, type FillPaintPropsPossiblyEvaluated} from './fill_style_layer_properties.g.ts';

import type {Transitionable, Transitioning, Layout, PossiblyEvaluated} from '../properties.ts';
import type {LayerSpecification} from '@maplibre/maplibre-gl-style-spec';
import type {BucketParameters} from '../../data/bucket.ts';
import type {FillLayoutProps, FillPaintProps} from './fill_style_layer_properties.g.ts';
import type {EvaluationParameters} from '../evaluation_parameters.ts';

export const isFillStyleLayer = (layer: StyleLayer): layer is FillStyleLayer => layer.type === 'fill';

export class FillStyleLayer extends StyleLayer {
    _unevaluatedLayout: Layout<FillLayoutProps>;
    layout: PossiblyEvaluated<FillLayoutProps, FillLayoutPropsPossiblyEvaluated>;

    _transitionablePaint: Transitionable<FillPaintProps>;
    _transitioningPaint: Transitioning<FillPaintProps>;
    paint: PossiblyEvaluated<FillPaintProps, FillPaintPropsPossiblyEvaluated>;

    /**
     * Scenic Roads: how the fill is drawn: 0 draped on the terrain (or flat on the map), as
     * upstream; 1 in 3D, `fill-z-offset` above the ground under each vertex; 2 in 3D,
     * `fill-z-offset` above sea level (style_spec_extensions.ts): the water's surface at 0 m or a
     * lake's level over a sea floor.
     */
    elevationReference(): number {
        if (this.paint.get('fill-elevation-reference') === 'sea') return 2;
        return this.paint.get('fill-z-offset').constantOr(1) !== 0 ? 1 : 0;
    }

    is3D(): boolean {
        return this.elevationReference() > 0;
    }

    constructor(layer: LayerSpecification, globalState: Record<string, any>) {
        super(layer, properties, globalState);
    }

    recalculate(parameters: EvaluationParameters, availableImages: string[]): void {
        super.recalculate(parameters, availableImages);

        const outlineColor = this.paint._values['fill-outline-color'];
        if (outlineColor.value.kind === 'constant' && outlineColor.value.value === undefined) {
            this.paint._values['fill-outline-color'] = this.paint._values['fill-color'];
        }
    }

    createBucket(parameters: BucketParameters<any>): FillBucket {
        return new FillBucket(parameters);
    }

    queryRadius(): number {
        return translateDistance(this.paint.get('fill-translate'));
    }

    queryIntersectsFeature({
        queryGeometry,
        geometry,
        transform,
        pixelsToTileUnits}: QueryIntersectsFeatureParams
    ): boolean {
        const translatedPolygon = translate(queryGeometry,
            this.paint.get('fill-translate'),
            this.paint.get('fill-translate-anchor'),
            -transform.bearingInRadians, pixelsToTileUnits);
        return polygonIntersectsMultiPolygon(translatedPolygon, geometry);
    }

    isTileClipped(): boolean {
        return true;
    }
}
