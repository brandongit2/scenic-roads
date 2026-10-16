layout(location = 0) in vec3 a_pos3d;

// Scenic Roads: the distance fog (terrain.fragment.glsl): clip w × x − y is 0 at the map's centre,
// 1 at the horizon (terrain_program.ts fogDepth).
uniform vec2 u_fog_depth;
uniform float u_ele_delta;

out vec2 v_texture_pos;
out float v_fog_depth;

void main() {
    float ele = get_elevation(a_pos3d.xy);
    float ele_delta = a_pos3d.z == 1.0 ? u_ele_delta : 0.0;
    v_texture_pos = a_pos3d.xy / 8192.0;
    gl_Position = projectTileFor3D(a_pos3d.xy, ele - ele_delta);
    // (clip w: the depth from the camera, in pixels on the globe and the flat map alike)
    v_fog_depth = gl_Position.w * u_fog_depth.x - u_fog_depth.y;
}
