uniform vec2 u_dimension;
// Scenic Roads: the part drawn's place in the DEM tile and in the mask's (water_mask.ts): x, y, scale.
uniform vec3 u_cell;
uniform vec3 u_mask_cell;

layout(location = 0) in vec2 a_pos;

out vec2 v_pos;
out vec2 v_mask;

void main() {
    gl_Position = projectTile(a_pos, a_pos);
    highp vec2 epsilon = 2.0 / u_dimension;
    float scale = (u_dimension.x - 4.0) / u_dimension.x;
    v_pos = (u_cell.xy + (a_pos / 8192.0) * u_cell.z) * scale + epsilon;
    v_mask = u_mask_cell.xy + (a_pos / 8192.0) * u_mask_cell.z;
    // North pole
    if (a_pos.y < -32767.5) {
        v_pos.y = 0.0;
    }
    // South pole
    if (a_pos.y > 32766.5) {
        v_pos.y = 1.0;
    }
}
