#ifdef LIFTED
// Scenic Roads: a lifted layer's program clips it to its tile here (its tile's stencil mask is
// drawn flat at sea level); the flat programs are upstream's, without the discard.
in highp vec2 v_tile_pos;
#endif
#pragma maplibre: define highp vec4 color
#pragma maplibre: define lowp float opacity

void main() {
    #ifdef LIFTED
    if (v_tile_pos.x < 0.0 || v_tile_pos.y < 0.0 || v_tile_pos.x > 8192.0 || v_tile_pos.y > 8192.0) discard;
    #endif
    #pragma maplibre: initialize highp vec4 color
    #pragma maplibre: initialize lowp float opacity

    fragColor = color * opacity;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
