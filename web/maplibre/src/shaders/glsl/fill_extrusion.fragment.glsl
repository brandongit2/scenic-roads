// Scenic Roads: the terrain's distance fog on the extrusions (terrain.fragment.glsl, Sky#fogRange),
// so a far skyline doesn't stand sharp against a fogged ground: the same uniforms, the depth from
// the vertex, the blend in linear light on the colour before its opacity.
uniform vec3 u_fog;
uniform vec4 u_fog_color;
uniform vec4 u_horizon_color;
uniform float u_horizon_fog_blend;
in float v_fog_depth;

in vec4 v_color;

void main() {
    fragColor = v_color;
    float f = clamp((v_fog_depth - u_fog.x) / (u_fog.y - u_fog.x), 0.0, 1.0);
    if (u_fog.z > 0.0 && f > 0.0 && v_color.a > 0.0) {
        vec3 c = pow(v_color.rgb / v_color.a, vec3(2.2));
        float blend_color = smoothstep(0.0, 1.0, max((f - u_horizon_fog_blend) / max(1.0 - u_horizon_fog_blend, 1e-3), 0.0));
        vec3 fog = mix(pow(u_fog_color.rgb, vec3(2.2)), pow(u_horizon_color.rgb, vec3(2.2)), blend_color);
        fragColor = vec4(pow(mix(c, fog, f * f * u_fog.z), vec3(1.0 / 2.2)) * v_color.a, v_color.a);
    }

    #ifdef OVERDRAW_INSPECTOR
        fragColor = vec4(1.0);
    #endif
}
