import {latest} from '@maplibre/maplibre-gl-style-spec';

/**
 * Scenic Roads: the style properties this fork adds to MapLibre's style spec. They're added to the
 * spec's own object (`latest`) when this module loads, so validation, expressions and the generated
 * property code (build/generate-style-code.ts imports the spec from here) know them as any other.
 *
 * Elevated lines and fills (one mechanism for both, after Mapbox GL v3's `line-z-offset` and
 * `line-elevation-reference`): with a `*-z-offset` other than 0, or the reference `sea`, the layer
 * isn't draped on the terrain but drawn in 3D above it, depth-tested against it.
 * - `ground` (default): the offset is a height above the terrain under each vertex, in metres, not
 *   exaggerated (as an extrusion's height); 0 drapes the layer as before.
 * - `sea`: the offset is an elevation above sea level, on the terrain's scale (multiplied by its
 *   exaggeration, as the terrain's own heights are), so 0 is the sea's surface and a lake's level
 *   puts it on the lake.
 * Without 3D terrain the ground is at 0, so both are a height above the map.
 */
const elevationReference = (prefix: string) => ({
    type: 'enum',
    values: {
        ground: {doc: 'The z-offset is a height above the terrain under the feature.'},
        sea: {doc: 'The z-offset is an elevation above sea level, on the terrain\'s exaggerated scale.'},
    },
    default: 'ground',
    doc: `What \`${prefix}-z-offset\` is measured from (Scenic Roads).`,
    expression: {interpolated: false, parameters: ['zoom']},
    'property-type': 'data-constant',
});

/** A raster's (one per layer: data-constant). */
const rasterZOffset = {
    type: 'number',
    default: 0,
    units: 'meters',
    transition: false,
    doc: 'The raster\'s height above `raster-elevation-reference`: with it other than 0, or the reference `sea`, drawn in 3D over the terrain rather than draped (Scenic Roads).',
    expression: {interpolated: true, parameters: ['zoom']},
    'property-type': 'data-constant',
};

const zOffset = (prefix: string) => ({
    type: 'number',
    default: 0,
    units: 'meters',
    transition: true,
    doc: `The feature's height above \`${prefix}-elevation-reference\`: with it other than 0, or the reference \`sea\`, drawn in 3D over the terrain rather than draped (Scenic Roads).`,
    expression: {interpolated: true, parameters: ['zoom', 'feature', 'feature-state']},
    'property-type': 'data-driven',
});

export const SCENIC_PROPERTIES: Record<string, Record<string, unknown>> = {
    paint_line: {'line-z-offset': zOffset('line'), 'line-elevation-reference': elevationReference('line')},
    paint_fill: {'fill-z-offset': zOffset('fill'), 'fill-elevation-reference': elevationReference('fill')},
    paint_raster: {'raster-z-offset': rasterZOffset, 'raster-elevation-reference': elevationReference('raster')},
};

// Point labels on the same terms: `symbol-height-anchor` `sea`, its `symbol-height-offset` an
// elevation above sea level on the terrain's scale (times its exaggeration), where upstream's
// `absolute` isn't exaggerated (a label at a lake's level sank into exaggerated terrain).
const heightAnchor = (latest as unknown as {layout_symbol: Record<string, {values: Record<string, unknown>; doc?: string}>}).layout_symbol['symbol-height-anchor'];
if (heightAnchor && !heightAnchor.values.sea) {
    heightAnchor.values.sea = {doc: 'The offset is an elevation above sea level, on the terrain\'s exaggerated scale (Scenic Roads).'};
}

/** Scenic Roads: what a symbol's `symbol-height-offset` is multiplied by: the terrain's exaggeration for the anchor `sea`. */
export function symbolHeightScale(anchor: string, exaggeration: number | undefined): number {
    return anchor === 'sea' ? (exaggeration ?? 1) : 1;
}

for (const group in SCENIC_PROPERTIES) {
    const target = latest[group as keyof typeof latest] as unknown as Record<string, unknown>;
    for (const name in SCENIC_PROPERTIES[group]) {
        if (!(name in target)) target[name] = SCENIC_PROPERTIES[group][name];
    }
}

export {latest as styleSpec};
