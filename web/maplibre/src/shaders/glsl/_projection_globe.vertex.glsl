#define GLOBE_RADIUS 6371008.8

// Custom layers get this prelude without _prelude.vertex.glsl, which defines PROJECTION_UBO, so they set these as plain uniforms.
#ifndef PROJECTION_UBO
uniform highp vec4 u_projection_tile_mercator_coords;
uniform highp vec4 u_projection_clipping_plane;
uniform highp float u_projection_transition;
uniform mat4 u_projection_fallback_matrix;
#endif

// The in-tile X coordinate of the projected vertex, used by clipAntimeridian() in the fragment shader (see _projection_globe.fragment.glsl).
out highp float v_projection_tile_x;

vec3 globeRotateVector(vec3 vec, vec2 angles) {
    vec3 axisRight = vec3(vec.z, 0.0, -vec.x); // Equivalent to cross(vec3(0.0, 1.0, 0.0), vec)
    vec3 axisUp = cross(axisRight, vec);
    axisRight = normalize(axisRight);
    axisUp = normalize(axisUp);
    vec2 t = tan(angles);
    return normalize(vec + axisRight * t.x + axisUp * t.y);
}

mat3 globeGetRotationMatrix(vec3 spherePos) {
    vec3 axisRight = vec3(spherePos.z, 0.0, -spherePos.x); // Equivalent to cross(vec3(0.0, 1.0, 0.0), vec)
    vec3 axisDown = cross(axisRight, spherePos);
    axisRight = normalize(axisRight);
    axisDown = normalize(axisDown);
    return mat3(
        axisRight,
        axisDown,
        spherePos
    );
}

// Consider this private, do not use in other shaders directly!
// Use `projectLineThickness` or `projectCircleRadius` instead.
float circumferenceRatioAtTileY(float tileY) {
    float mercator_pos_y = u_projection_tile_mercator_coords.y + u_projection_tile_mercator_coords.w * tileY;
    // cos(spherical_y) derived algebraically from exp(), see projectToSphere below.
    float t = exp(PI - (mercator_pos_y * PI * 2.0));
    return (2.0 * t) / (t * t + 1.0);
}

float projectLineThickness(float tileY) {
    float thickness = 1.0 / circumferenceRatioAtTileY(tileY);
    if (u_projection_transition < 0.999) {
        return mix(1.0, thickness, u_projection_transition);
    } else {
        return thickness;
    }
}

// Get position inside the tile in range 0..8192 and project it onto the surface of a unit sphere.
// Additionally project special Y values to the poles.
// - translatedPos: tile-space vertex position, optionally with user-specified translation already applied
// - rawPos: the original tile-space vertex position *without translation* - needed because we would not be able to detect pole vertices from coordinates modified by translation.
vec3 projectToSphere(vec2 translatedPos, vec2 rawPos) {
    // Compute position in range 0..1 of the base tile of web mercator
    vec2 mercator_pos = u_projection_tile_mercator_coords.xy + u_projection_tile_mercator_coords.zw * translatedPos;

    // Now compute angular coordinates on the surface of a perfect sphere
    float spherical_x = mercator_pos.x * PI * 2.0 + PI;

    // sin_sy/cos_sy are sin(spherical_y)/cos(spherical_y), where
    // spherical_y = 2*atan(exp(PI - mercator_pos.y*2*PI)) - PI/2 (the Gudermannian
    // function, converting mercator Y to latitude). The direct formula subtracts
    // PI/2 from a value near PI/2, destroying most float32 mantissa bits near the
    // equator, and some GPUs (e.g. Mali) have imprecise atan/sin/cos besides.
    // Substituting t = exp(PI - mercator_pos.y*2*PI) and expanding via the
    // tangent half-angle (Weierstrass) identities eliminates atan, sin and cos
    // entirely, leaving only exp() and rational arithmetic:
    //   sin(2*atan(t) - PI/2) = (t^2 - 1) / (t^2 + 1)
    //   cos(2*atan(t) - PI/2) =    2*t    / (t^2 + 1)
    float t = exp(PI - (mercator_pos.y * PI * 2.0));
    float t2 = t * t;
    float denom = t2 + 1.0;
    float sin_sy = (t2 - 1.0) / denom;
    float cos_sy = (2.0 * t) / denom;

    vec3 pos = vec3(
        sin(spherical_x) * cos_sy,
        sin_sy,
        cos(spherical_x) * cos_sy
    );

    // North pole
    if (rawPos.y < -32767.5) {
        pos = vec3(0.0, 1.0, 0.0);
    }
    // South pole
    if (rawPos.y > 32766.5) {
        pos = vec3(0.0, -1.0, 0.0);
    }

    return pos;
}

vec3 projectToSphere(vec2 posInTile) {
    return projectToSphere(posInTile, vec2(0.0, 0.0));
}

float globeComputeClippingZ(vec3 spherePos) {
    return (1.0 - (dot(spherePos, u_projection_clipping_plane.xyz) + u_projection_clipping_plane.w));
}

vec4 interpolateProjection(vec2 posInTile, vec3 spherePos, float elevation) {
    v_projection_tile_x = posInTile.x;
    vec3 elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    vec4 globePosition = u_projection_matrix * vec4(elevatedPos, 1.0);
    // Z is overwritten by glDepthRange anyway - use a custom z value to clip geometry on the invisible side of the sphere.
    globePosition.z = globeComputeClippingZ(elevatedPos) * globePosition.w;

    if (u_projection_transition > 0.999) {
        // Simple case - no transition, only globe projection
        return globePosition;
    }

    // Blend between globe and mercator projections.
    vec4 flatPosition = u_projection_fallback_matrix * vec4(posInTile, elevation, 1.0);
    // Only interpolate to globe's Z for the last 50% of the animation.
    // (globe Z hides anything on the backfacing side of the planet)
    const float z_globeness_threshold = 0.2;
    vec4 result = globePosition;
    result.z = mix(0.0, globePosition.z, clamp((u_projection_transition - z_globeness_threshold) / (1.0 - z_globeness_threshold), 0.0, 1.0));
    result.xyw = mix(flatPosition.xyw, globePosition.xyw, u_projection_transition);
    // Gradually hide poles during transition
    if ((posInTile.y < -32767.5) || (posInTile.y > 32766.5)) {
        result = globePosition;
        const float poles_hidden_anim_percentage = 0.02; // Only draw poles in the last 2% of the animation.
        result.z = mix(globePosition.z, 100.0, pow(max((1.0 - u_projection_transition) / poles_hidden_anim_percentage, 0.0), 8.0));
    }
    return result;
}

// Scenic Roads: the globe's 3D positions without float32's noise. Multiplying a unit-sphere point by
// the view matrix in float32 lands each vertex up to a metre or two off, a different way for every
// vertex and camera: the terrain was bumpy at its mesh's spacing and a wall's foot a saw of spikes
// (the app's task #115). Here a position is the tile's flat (fallback) projection, exact in small
// numbers, plus the globe-minus-flat difference interpolated across the z14 cell the vertex is in
// from its four corners: a corner's mercator coordinates are exact, so every vertex of every layer
// near it gets the same difference and the float32 error moves terrain and buildings together.
// (Bilinear across a z14 cell, the sphere's own curve is off by ~0.1 m at most.) The elevation's
// share is added from the vertex's own sphere point. Placing each vertex from one anchor in the
// cell instead didn't help: the shader compiler is free to fold the two matrix products into one.

// The lattice: z14 cells.
const float LATTICE = 16384.0;

// A lattice point's globe position (its mercator coordinates exact).
vec4 globeAtLattice(vec2 merc) {
    float sx = merc.x * PI * 2.0 + PI;
    float t = exp(PI - (merc.y * PI * 2.0));
    float t2 = t * t;
    float den = t2 + 1.0;
    return u_projection_matrix * vec4(sin(sx) * (2.0 * t) / den, (t2 - 1.0) / den, cos(sx) * (2.0 * t) / den, 1.0);
}

// Unlike interpolateProjection, this variant of the function preserves the Z value of the final vector.
vec4 interpolateProjectionFor3D(vec2 posInTile, vec3 spherePos, float elevation) {
    v_projection_tile_x = posInTile.x;
    vec4 globePosition;
    if ((posInTile.y < -32767.5) || (posInTile.y > 32766.5)) {
        // The poles' vertices.
        globePosition = u_projection_matrix * vec4(spherePos * (1.0 + elevation / GLOBE_RADIUS), 1.0);
    } else {
        vec2 o = u_projection_tile_mercator_coords.xy;
        vec2 k = u_projection_tile_mercator_coords.zw;
        vec2 g = floor((o + k * posInTile) * LATTICE);
        vec2 f = ((o - g / LATTICE) + k * posInTile) * LATTICE;
        vec2 c0 = (g / LATTICE - o) / k;
        vec2 c1 = ((g + 1.0) / LATTICE - o) / k;
        mat4 F = u_projection_fallback_matrix;
        vec4 a00 = globeAtLattice(g / LATTICE) - F * vec4(c0.x, c0.y, 0.0, 1.0);
        vec4 a10 = globeAtLattice(vec2(g.x + 1.0, g.y) / LATTICE) - F * vec4(c1.x, c0.y, 0.0, 1.0);
        vec4 a01 = globeAtLattice(vec2(g.x, g.y + 1.0) / LATTICE) - F * vec4(c0.x, c1.y, 0.0, 1.0);
        vec4 a11 = globeAtLattice((g + 1.0) / LATTICE) - F * vec4(c1.x, c1.y, 0.0, 1.0);
        vec4 a = mix(mix(a00, a10, f.x), mix(a01, a11, f.x), f.y);
        globePosition = F * vec4(posInTile, 0.0, 1.0) + a + (elevation / GLOBE_RADIUS) * (u_projection_matrix * vec4(spherePos, 0.0));
    }

    if (u_projection_transition > 0.999) {
        return globePosition;
    }

    // Blend between globe and mercator projections.
    vec4 fallbackPosition = u_projection_fallback_matrix * vec4(posInTile, elevation, 1.0);
    return mix(fallbackPosition, globePosition, u_projection_transition);
}

// Computes screenspace projection
// and **replaces Z** with a custom value that clips geometry
// on the backfacing side of the planet.
vec4 projectTile(vec2 posInTile) {
    return interpolateProjection(posInTile, projectToSphere(posInTile), 0.0);
}

// A variant that supports special pole vertices.
vec4 projectTile(vec2 posInTile, vec2 rawPos) {
    return interpolateProjection(posInTile, projectToSphere(posInTile, rawPos), 0.0);
}

// Uses elevation to compute final screenspace projection
// and **replaces Z** with a custom value that clips geometry
// on the backfacing side of the planet.
vec4 projectTileWithElevation(vec2 posInTile, float elevation) {
    return interpolateProjection(posInTile, projectToSphere(posInTile), elevation);
}

// Projects the tile coordinates+elevation while **preserving Z** value from multiplication with the projection matrix.
// Applies pole vertices.
vec4 projectTileFor3D(vec2 posInTile, float elevation) {
    vec3 spherePos = projectToSphere(posInTile, posInTile);
    return interpolateProjectionFor3D(posInTile, spherePos, elevation);
}
