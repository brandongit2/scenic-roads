// Scenic Roads: the terrain's fog on the extrusions. MapLibre fogs the terrain toward the horizon
// (the sky's fog-ground-blend, from 60 degrees of pitch, on the flat map; on the globe it fogs
// nothing) and drew the extrusions in their colour alone, so a far skyline stood sharp against a
// fogged ground. Here they take the terrain's fog: its uniforms set as the terrain's are (the same
// fog matrix of the tile, colours, blends and opacity), its depth from the vertex, the blend in
// linear light on the colour before its opacity.
uniform vec4 u_fog_color;
uniform vec4 u_horizon_color;
uniform float u_fog_ground_blend;
uniform float u_fog_ground_blend_opacity;
uniform float u_horizon_fog_blend;
uniform float u_is_globe_mode;
in float v_fog_depth;

in vec4 v_color;

void main() {
    fragColor = v_color;
    if (u_is_globe_mode < 0.5 && u_fog_ground_blend_opacity > 0.0 && v_fog_depth > u_fog_ground_blend && v_color.a > 0.0) {
        vec3 c = pow(v_color.rgb / v_color.a, vec3(2.2));
        float blend_color = smoothstep(0.0, 1.0, max((v_fog_depth - u_horizon_fog_blend) / (1.0 - u_horizon_fog_blend), 0.0));
        vec3 fog = mix(pow(u_fog_color.rgb, vec3(2.2)), pow(u_horizon_color.rgb, vec3(2.2)), blend_color);
        float f = max(v_fog_depth - u_fog_ground_blend, 0.0) / (1.0 - u_fog_ground_blend);
        fragColor = vec4(pow(mix(c, fog, pow(f, 2.0) * u_fog_ground_blend_opacity), vec3(1.0 / 2.2)) * v_color.a, v_color.a);
    }

    #ifdef OVERDRAW_INSPECTOR
        fragColor = vec4(1.0);
    #endif
}
