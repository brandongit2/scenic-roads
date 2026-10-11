// floor(127 / 2) == 63.0
// the maximum allowed miter limit is 2.0 at the moment. the extrude normal is
// stored in a byte (-128..127). we scale regular normals up to length 63, but
// there are also "special" normals that have a bigger length (of up to 126 in
// this case).
// #define scale 63.0
#define scale 0.015873016

// We scale the distance before adding it to the buffers so that we can store
// long distances for long segments. Use this value to unscale the distance.
#define LINE_DISTANCE_SCALE 2.0

layout(location = 0) in ivec2 a_pos_normal;
layout(location = 1) in uvec4 a_data;

uniform vec2 u_translation;
// Scenic Roads: 0 draped (or flat), 1 z_offset above the ground, 2 above sea level.
uniform float u_elevation_reference;
uniform mediump float u_ratio;
uniform float u_tileratio;
uniform float u_crossfade_from;
uniform float u_crossfade_to;
uniform float u_lineatlas_height;

out vec2 v_normal;
flat out vec2 v_width2;
out vec2 v_tex_a;
out vec2 v_tex_b;
out float v_gamma_scale;
#ifdef GLOBE
out float v_depth;
#endif

#pragma maplibre: define highp vec4 color
#pragma maplibre: define lowp float blur
#pragma maplibre: define lowp float opacity
#pragma maplibre: define mediump float gapwidth
#pragma maplibre: define lowp float offset
#pragma maplibre: define mediump float width
#pragma maplibre: define highp float z_offset
#pragma maplibre: define lowp float floorwidth
#pragma maplibre: define mediump vec4 dasharray_from
#pragma maplibre: define mediump vec4 dasharray_to

void main() {
    #pragma maplibre: initialize highp vec4 color
    #pragma maplibre: initialize lowp float blur
    #pragma maplibre: initialize lowp float opacity
    #pragma maplibre: initialize mediump float gapwidth
    #pragma maplibre: initialize lowp float offset
    #pragma maplibre: initialize mediump float width
    #pragma maplibre: initialize highp float z_offset
    #pragma maplibre: initialize lowp float floorwidth
    #pragma maplibre: initialize mediump vec4 dasharray_from
    #pragma maplibre: initialize mediump vec4 dasharray_to

    // Move vertex outside clip space to discard triangle when opacity is negligible
    if (opacity < 0.01) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    float ANTIALIASING = 1.0 / u_device_pixel_ratio / 2.0;

    vec2 a_extrude = vec2(ivec2(a_data.xy) - 128);
    float a_direction = float(int(a_data.z & 3u) - 1);
    float a_linesofar = float((a_data.z >> 2u) + a_data.w * 64u) * LINE_DISTANCE_SCALE;

    vec2 pos = vec2(a_pos_normal >> 1);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of a_pos_normal
    mediump vec2 normal = vec2(a_pos_normal & 1);
    normal.y = normal.y * 2.0 - 1.0;
    v_normal = normal;

    // these transformations used to be applied in the JS and native code bases.
    // moved them into the shader for clarity and simplicity.
    gapwidth = gapwidth / 2.0;
    float halfwidth = width / 2.0;
    offset = -1.0 * offset;

    float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width
    // of this vertex.
    mediump vec2 dist = outset * a_extrude * scale;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    mediump float u = 0.5 * a_direction;
    mediump float t = 1.0 - abs(u);
    mediump vec2 offset2 = offset * a_extrude * scale * normal.y * mat2(t, -u, u, t);

    float adjustedThickness = projectLineThickness(pos.y);
    vec2 pos_no_extrude = pos + offset2 / u_ratio * adjustedThickness + u_translation;
    vec2 pos_with_extrude = pos_no_extrude + dist / u_ratio * adjustedThickness;
    vec4 projected_no_extrude;
    vec4 projected_with_extrude;
    // Scenic Roads: an elevated line (line-z-offset, line-elevation-reference) is drawn in 3D at its
    // elevation, with the perspective depth the terrain's is drawn with; else as upstream.
    bool elevated = u_elevation_reference > 0.5;
    if (elevated) {
        float ele = elevationOf(pos, z_offset, u_elevation_reference);
        projected_no_extrude = projectTileFor3D(pos_no_extrude, ele);
        projected_with_extrude = projectTileFor3D(pos_with_extrude, ele);
    } else {
        projected_no_extrude = projectTile(pos_no_extrude);
        projected_with_extrude = projectTile(pos_with_extrude);
    }
    gl_Position = projected_with_extrude;
    #ifdef GLOBE
    if (elevated) {
        // (the horizon's clipping depth, for the fragment shader's back-of-the-globe test)
        vec4 clip = projectTileWithElevation(pos_with_extrude, elevationOf(pos, z_offset, u_elevation_reference));
        v_depth = clip.z / clip.w;
    } else {
        v_depth = gl_Position.z / gl_Position.w;
    }
    #endif

    // calculate how much the perspective view squishes or stretches the extrude
    #ifdef TERRAIN3D
    if (!elevated) {
        v_gamma_scale = 1.0; // not needed, because this is done automatically via the mesh
    } else
    #endif
    {
        float extrude_length_without_perspective = length(dist);
        float extrude_length_with_perspective = length((projected_with_extrude.xy - projected_no_extrude.xy) / projected_with_extrude.w * u_units_to_pixels);
        v_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;
    }

    float u_patternscale_a_x = u_tileratio / dasharray_from.w / u_crossfade_from;
    float u_patternscale_a_y = -dasharray_from.z / 2.0 / u_lineatlas_height;
    float u_patternscale_b_x = u_tileratio / dasharray_to.w / u_crossfade_to;
    float u_patternscale_b_y = -dasharray_to.z / 2.0 / u_lineatlas_height;

    v_tex_a = vec2(a_linesofar * u_patternscale_a_x / floorwidth, normal.y * u_patternscale_a_y + (float(dasharray_from.y) + 0.5) / u_lineatlas_height);
    v_tex_b = vec2(a_linesofar * u_patternscale_b_x / floorwidth, normal.y * u_patternscale_b_y + (float(dasharray_to.y) + 0.5) / u_lineatlas_height);
    v_width2 = vec2(outset, inset);
}
