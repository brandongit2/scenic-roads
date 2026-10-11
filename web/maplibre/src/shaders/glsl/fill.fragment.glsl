// Scenic Roads: a lifted layer (u_elevation_reference > 0) is clipped to its tile here, its tile's
// stencil mask being drawn flat at sea level.
uniform highp float u_elevation_reference;
in highp vec2 v_tile_pos;
#pragma maplibre: define highp vec4 color
#pragma maplibre: define lowp float opacity

void main() {
    if (u_elevation_reference > 0.5 && (v_tile_pos.x < 0.0 || v_tile_pos.y < 0.0 || v_tile_pos.x > 8192.0 || v_tile_pos.y > 8192.0)) discard;
    #pragma maplibre: initialize highp vec4 color
    #pragma maplibre: initialize lowp float opacity

    fragColor = color * opacity;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
