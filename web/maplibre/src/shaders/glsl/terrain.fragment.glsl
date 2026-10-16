uniform sampler2D u_texture;
// Scenic Roads: distance fog (Sky#fogRange), on the globe as on the flat map: the depth 0 at the
// map's centre and 1 at the horizon, none before u_fog.x, u_fog.z's opacity from u_fog.y.
uniform vec3 u_fog;
uniform vec4 u_fog_color;
uniform vec4 u_horizon_color;
uniform float u_horizon_fog_blend;

in vec2 v_texture_pos;
in float v_fog_depth;

const float gamma = 2.2;

vec4 gammaToLinear(vec4 color) {
    return pow(color, vec4(gamma));
}

vec4 linearToGamma(vec4 color) {
    return pow(color, vec4(1.0 / gamma));
}

void main() {
    vec4 surface_color = texture(u_texture, vec2(v_texture_pos.x, 1.0 - v_texture_pos.y));

    float f = clamp((v_fog_depth - u_fog.x) / (u_fog.y - u_fog.x), 0.0, 1.0);
    if (u_fog.z > 0.0 && f > 0.0) {
        vec4 surface_color_linear = gammaToLinear(surface_color);
        float blend_color = smoothstep(0.0, 1.0, max((f - u_horizon_fog_blend) / max(1.0 - u_horizon_fog_blend, 1e-3), 0.0));
        vec4 fog_horizon_color_linear = mix(gammaToLinear(u_fog_color), gammaToLinear(u_horizon_color), blend_color);
        fragColor = linearToGamma(mix(surface_color_linear, fog_horizon_color_linear, f * f * u_fog.z));
    } else {
        fragColor = surface_color;
    }
}
