uniform vec2 u_fill_translate;
#ifdef LIFTED
// Scenic Roads: a lifted fill's program (painter.useProgram's LIFTED define): 1 z_offset above the
// ground, 2 above sea level; the vertex in its tile for the fragment shader's clip.
uniform float u_elevation_reference;
out highp vec2 v_tile_pos;
#endif

layout(location = 0) in vec2 a_pos;

out vec2 v_pos;
#ifdef GLOBE
out float v_depth;
#endif

#pragma maplibre: define highp vec4 outline_color
#pragma maplibre: define lowp float opacity
#pragma maplibre: define highp float z_offset

void main() {
    #pragma maplibre: initialize highp vec4 outline_color
    #pragma maplibre: initialize lowp float opacity
    #pragma maplibre: initialize highp float z_offset

    // Move vertex outside clip space to discard triangle when opacity is negligible
    if (opacity < 0.01) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }

    // Scenic Roads: a lifted fill (fill-z-offset, fill-elevation-reference) is drawn in 3D at its
    // elevation, with the perspective depth the terrain's is drawn with; else as upstream.
    #ifdef LIFTED
    float ele = elevationOf(a_pos, z_offset, u_elevation_reference);
    gl_Position = projectTileFor3D(a_pos + u_fill_translate, ele);
    v_tile_pos = a_pos + u_fill_translate;
    #ifndef GLOBE
    // (pole vertices killed, as projectTile kills them on mercator)
    if (a_pos.y < -32767.5 || a_pos.y > 32766.5) gl_Position.z = -10000000.0;
    #endif
    #else
    gl_Position = projectTile(a_pos + u_fill_translate, a_pos);
    #endif

    v_pos = (gl_Position.xy / gl_Position.w + 1.0) / 2.0 * u_world_size;
    #ifdef GLOBE
    #ifdef LIFTED
    // (the horizon's clipping depth, for the fragment shader's back-of-the-globe test)
    vec4 clip = projectTileWithElevation(a_pos + u_fill_translate, ele);
    v_depth = clip.z / clip.w;
    #else
    v_depth = gl_Position.z / gl_Position.w;
    #endif
    #endif
}
