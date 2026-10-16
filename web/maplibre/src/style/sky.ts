import {type PossiblyEvaluated, Transitionable, type Transitioning, type TransitionParameters} from './properties.ts';
import {Evented} from '../util/evented.ts';
import {EvaluationParameters} from './evaluation_parameters.ts';
import {validateStyle, validateAndEmit, type Validator} from './validate_style.ts';
import {getProperties, type SkyProps, type SkyPropsPossiblyEvaluated} from './sky_properties.g.ts';
import {earthRadius} from '../geo/lng_lat.ts';

import type {Mesh} from '../render/mesh.ts';
import type {SkySpecification} from '@maplibre/maplibre-gl-style-spec';
import type {StyleSetterOptions} from './style.ts';

/** Scenic Roads: the distance fog's range (Sky#fogRange). */
export type FogRange = {start: number; end: number; opacity: number};

/** What the distance fog needs of the transform. */
export type FogTransform = {
    readonly cameraToCenterDistance: number;
    readonly pixelsPerMeter: number;
    getCameraAltitude(): number;
};

/**
 * The distance fog's depth (Sky#fogRange): a vertex's clip w (its distance from the camera along
 * the view, in the projection's pixels, on the globe as on the flat map) × [0] − [1] is 0 at the
 * map's centre and 1 at the horizon: the distance from the camera's altitude to where a line of
 * sight grazes the sea-level sphere.
 */
export function fogDepth(tr: FogTransform): [number, number] {
    const h = Math.max(tr.getCameraAltitude(), 1);
    const horizon = Math.sqrt(h * (2 * earthRadius + h)) * tr.pixelsPerMeter;
    const c = tr.cameraToCenterDistance;
    const span = Math.max(horizon - c, c * 0.05, 1e-9);
    return [1 / span, c / span];
}

export class Sky extends Evented {
    properties: PossiblyEvaluated<SkyProps, SkyPropsPossiblyEvaluated>;

    /**
     * This is used to cache the gl mesh for the sky, it should be initialized only once.
     */
    mesh: Mesh | undefined;
    atmosphereMesh: Mesh | undefined;
    _transitionable: Transitionable<SkyProps>;
    _transitioning: Transitioning<SkyProps>;

    constructor(sky: SkySpecification | undefined, globalState: Record<string, any>) {
        super();
        this._transitionable = new Transitionable(getProperties(), 'sky', globalState);
        this.setSky(sky);
        this._transitioning = this._transitionable.untransitioned();
        this.recalculate(new EvaluationParameters(0));
    }

    /**
     * Validates and applies the sky. Returns false, after firing `error`, when
     * the value was rejected and nothing changed.
     */
    setSky(sky?: SkySpecification, options: StyleSetterOptions = {}): boolean {
        if (this._validate(validateStyle.sky, sky, options)) return false;

        sky ||= {
            'sky-color': 'transparent',
            'horizon-color': 'transparent',
            'fog-color': 'transparent',
            'fog-ground-blend': 1,
            'atmosphere-blend': 0,
        };

        this._transitionable.setValues(sky);
        return true;
    }

    getSky(): SkySpecification {
        return this._transitionable.serialize();
    }

    updateTransitions(parameters: TransitionParameters): void {
        this._transitioning = this._transitionable.transitioned(parameters, this._transitioning);
    }

    hasTransition(): boolean {
        return this._transitioning.hasTransition();
    }

    applyGlobalStateChange(refs: string[], priorGlobalState: Record<string, any>, parameters: TransitionParameters): void {
        this._transitioning = this._transitionable.applyGlobalStateChange(refs, priorGlobalState, this._transitioning, parameters);
    }

    recalculate(parameters: EvaluationParameters): void {
        this.properties = this._transitioning.possiblyEvaluate(parameters);
    }

    _validate(validate: Validator, value: unknown, options: StyleSetterOptions = {}): boolean {
        return validateAndEmit(this, validate, {value}, options);
    }

    /**
     * Scenic Roads: distance fog, in place of MapLibre's (which fogged the terrain alone, on the
     * flat map alone, from 60° of pitch, by a depth running from the map's centre to the far plane).
     * The terrain, the extrusions (and the app's buildings field) fade toward the sky's `fog-color`,
     * then its `horizon-color` (past `horizon-fog-blend` of the ramp), by their distance from the
     * camera measured from the map's centre (0) to the horizon (1; terrain_program.ts fogDepth):
     * none nearer than `start`, the full `opacity` from `end`, the ramp between squared. The same
     * on the globe and the flat map at any pitch (a view straight down sees little beyond its
     * centre, so it stays clear). `fog-ground-blend` is no longer read. Not a style property: set
     * with `Map#setFogRange`.
     */
    fogRange: FogRange = {start: 0.02, end: 0.45, opacity: 0.9};

    /** fogDepth, for a custom layer that fogs alike (the app's buildings field). */
    fogDepth(tr: FogTransform): [number, number] {
        return fogDepth(tr);
    }

    /** The fragment shaders' `u_fog`: start, end (0 the map's centre, 1 the horizon), opacity. */
    fogUniform(): [number, number, number] {
        const r = this.fogRange;
        return [r.start, Math.max(r.end, r.start + 1e-3), Math.min(Math.max(r.opacity, 0), 1)];
    }
}
