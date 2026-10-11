uniform vec2 u_pixel_coord_upper;
uniform vec2 u_pixel_coord_lower;
uniform vec3 u_scale;
uniform vec2 u_fill_translate;
// Scenic Roads: 0 draped (or flat), 1 z_offset above the ground, 2 above sea level.
uniform float u_elevation_reference;
// (the vertex in its tile, for the fragment shader's clip of a lifted fill)
out highp vec2 v_tile_pos;

layout(location = 0) in vec2 a_pos;

out vec2 v_pos_a;
out vec2 v_pos_b;

#pragma maplibre: define lowp float opacity
#pragma maplibre: define highp float z_offset
#pragma maplibre: define highp vec4 color
#pragma maplibre: define lowp vec4 pattern_from
#pragma maplibre: define lowp vec4 pattern_to
#pragma maplibre: define lowp float pixel_ratio_from
#pragma maplibre: define lowp float pixel_ratio_to

void main() {
    #pragma maplibre: initialize lowp float opacity
    #pragma maplibre: initialize highp float z_offset
    #pragma maplibre: initialize highp vec4 color
    #pragma maplibre: initialize mediump vec4 pattern_from
    #pragma maplibre: initialize mediump vec4 pattern_to
    #pragma maplibre: initialize lowp float pixel_ratio_from
    #pragma maplibre: initialize lowp float pixel_ratio_to

    // Move vertex outside clip space to discard triangle when opacity is negligible
    if (opacity < 0.01) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    float tileZoomRatio = u_scale.x;
    float fromScale = u_scale.y;
    float toScale = u_scale.z;

    vec2 display_size_a = (pattern_br_a - pattern_tl_a) / pixel_ratio_from;
    vec2 display_size_b = (pattern_br_b - pattern_tl_b) / pixel_ratio_to;

    // Scenic Roads: an elevated fill (fill-z-offset, fill-elevation-reference) is drawn in 3D at its
    // elevation, with the perspective depth the terrain's is drawn with; else as upstream.
    float ele = 0.0;
    v_tile_pos = a_pos + u_fill_translate;
    if (u_elevation_reference > 0.5) {
        ele = elevationOf(a_pos, z_offset, u_elevation_reference);
        gl_Position = projectTileFor3D(a_pos + u_fill_translate, ele);
        #ifndef GLOBE
        // (pole vertices killed, as projectTile kills them on mercator)
        if (a_pos.y < -32767.5 || a_pos.y > 32766.5) gl_Position.z = -10000000.0;
        #endif
    } else {
        gl_Position = projectTile(a_pos + u_fill_translate, a_pos);
    }

    v_pos_a = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, a_pos);
    v_pos_b = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, toScale * display_size_b, tileZoomRatio, a_pos);
}
