#ifdef GL_ES
precision highp float;
#endif

uniform sampler2D u_image;
uniform vec4 u_unpack;
uniform sampler2D u_elevation_stops;
uniform sampler2D u_color_stops;
uniform int u_color_ramp_size;
uniform float u_opacity;

in vec2 v_pos;

float getElevation(vec2 coord) {
    // Convert encoded elevation value to meters
    vec4 data = texture(u_image, coord) * 255.0;
    data.a = -1.0;
    return dot(data, u_unpack);
}

float getElevationStop(int stop) {
    // Convert encoded elevation value to meters
    vec4 data = texelFetch(u_elevation_stops, ivec2(stop, 0), 0) * 255.0;
    data.a = -1.0;
    return dot(data, u_unpack);
}

// The colour of an elevation through the ramp (binary search over its stops).
vec4 rampAt(float el) {
    int r = (u_color_ramp_size - 1);
    int l = 0;
    float el_l = getElevationStop(l);
    float el_r = getElevationStop(r);
    while(r - l > 1)
    {
        int m = (r + l) / 2;
        float el_m = getElevationStop(m);
        if(el < el_m)
        {
            r = m;
            el_r = el_m;
        }
        else
        {
            l = m;
            el_l = el_m;
        }
    }

    float x = (float(l) + (el - el_l) / (el_r - el_l) + 0.5)/float(u_color_ramp_size);
    return texture(u_color_stops, vec2(x, 0));
}

// A premultiplied colour in linear light.
vec3 linOf(vec4 c) {
    return c.a > 0.0 ? pow(c.rgb / c.a, vec3(2.2)) * c.a : vec3(0.0);
}

void main() {
    vec4 col;
    // Scenic Roads: the slope tint's tiles (known by their encoding's base shift, 32768.5: the app's
    // basemap.ts SLOPE4_SHIFT) hold four slopes a pixel, the quarters of the ground beneath it
    // (each channel sqrt(slope / 400)). Each is coloured through the ramp and the colours averaged
    // in linear light, the way a screen's light adds up; zoomed out a white cliff band beside
    // yellow slopes then reads as on screen, not as the colour of their mean slope.
    if (abs(u_unpack.w - 32768.5) < 0.01) {
        vec4 q = texture(u_image, v_pos);
        vec4 s = q * q * 400.0;
        vec4 c0 = rampAt(s.x);
        vec4 c1 = rampAt(s.y);
        vec4 c2 = rampAt(s.z);
        vec4 c3 = rampAt(s.w);
        float a = 0.25 * (c0.a + c1.a + c2.a + c3.a);
        vec3 lin = 0.25 * (linOf(c0) + linOf(c1) + linOf(c2) + linOf(c3));
        col = a > 0.0 ? vec4(pow(lin / a, vec3(1.0 / 2.2)) * a, a) : vec4(0.0);
    } else {
        col = rampAt(getElevation(v_pos));
    }
    fragColor = u_opacity * col;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
