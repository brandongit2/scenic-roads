// The demo page's cases (demo.html): each adds its layers to the map once it has loaded.

const EXTENT = 8192;

/** A point's tile at zoom z and its position in it (0..EXTENT). */
function tileAt(lng, lat, z) {
  const n = 2 ** z;
  const x = ((lng + 180) / 360) * n;
  const s = Math.sin((lat * Math.PI) / 180);
  const y = (0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI)) * n;
  return {z, x: Math.floor(x), y: Math.floor(y), px: (x - Math.floor(x)) * EXTENT, py: (y - Math.floor(y)) * EXTENT};
}

function program(gl, vs, fs) {
  const p = gl.createProgram();
  for (const [type, src] of [[gl.VERTEX_SHADER, vs], [gl.FRAGMENT_SHADER, fs]]) {
    const s = gl.createShader(type);
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s));
    gl.attachShader(p, s);
  }
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p));
  return p;
}

const uniformsOf = (gl, p, names) => Object.fromEntries(names.map((n) => [n, gl.getUniformLocation(p, n)]));

/**
 * A custom layer that drapes stripes over the terrain (its drape hook, in every terrain tile's
 * texture) and stands a column on a summit after the terrain is drawn (render), testing the frame's
 * depth in its shader: green where it's in front of the terrain, red where the terrain hides it.
 */
class DrapeAndColumn {
  constructor(lng, lat) {
    this.id = 'drape-and-column';
    this.type = 'custom';
    this.renderingMode = '3d';
    this.readsSceneDepth = true;
    this.drapeRevision = 0;
    this.at = tileAt(lng, lat, 14);
    this.programs = {};
    this.drapes = 0;
    this.renders = 0;
  }

  onAdd(map, gl) {
    this.map = map;
    // The drape: a quad over the terrain tile, stripes in tile space.
    this.drapeProgram = program(gl, `#version 300 es
      uniform mat4 u_matrix;
      layout(location = 0) in vec2 a_pos;
      out vec2 v_pos;
      void main() { v_pos = a_pos; gl_Position = u_matrix * vec4(a_pos, 0.0, 1.0); }`, `#version 300 es
      precision highp float;
      in vec2 v_pos;
      out vec4 fragColor;
      void main() {
        float s = step(0.5, fract((v_pos.x + v_pos.y) / 1024.0));
        float edge = step(min(min(v_pos.x, v_pos.y), min(8192.0 - v_pos.x, 8192.0 - v_pos.y)), 48.0);
        float a = max(s * 0.18, edge * 0.8);
        fragColor = vec4(vec3(0.95, 0.75, 0.2) * a, a);
      }`);
    this.quad = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, this.quad);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([0, 0, EXTENT, 0, 0, EXTENT, EXTENT, EXTENT]), gl.STATIC_DRAW);
    // The column: four sides, 300 m across, from 0 to 25 km (rendered).
    const r = 300 / (40075016.686 * Math.cos((45.83 * Math.PI) / 180) / 2 ** 14 / EXTENT) / 2;
    const {px, py} = this.at;
    const c = [[px - r, py - r], [px + r, py - r], [px + r, py + r], [px - r, py + r]];
    const v = [];
    for (let i = 0; i < 4; i++) {
      const a = c[i], b = c[(i + 1) % 4];
      v.push(...a, 0, ...b, 0, ...a, 25000, ...a, 25000, ...b, 0, ...b, 25000);
    }
    this.column = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, this.column);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(v), gl.STATIC_DRAW);
  }

  drape(gl, args) {
    this.drapes++;
    const p = this.drapeProgram;
    gl.useProgram(p);
    gl.uniformMatrix4fv(gl.getUniformLocation(p, 'u_matrix'), false, args.getDrapeMatrix(args.tileID));
    gl.bindBuffer(gl.ARRAY_BUFFER, this.quad);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
  }

  columnProgram(gl, shaderData) {
    if (this.programs[shaderData.variantName]) return this.programs[shaderData.variantName];
    const p = program(gl, `#version 300 es
      ${shaderData.define}
      ${shaderData.vertexShaderPrelude}
      layout(location = 0) in vec3 a_pos;
      out float v_h;
      void main() {
        v_h = a_pos.z;
        gl_Position = projectTileFor3D(a_pos.xy, a_pos.z);
      }`, `#version 300 es
      precision highp float;
      uniform highp sampler2D u_scene;
      uniform vec2 u_size;
      in float v_h;
      out vec4 fragColor;
      void main() {
        float scene = texelFetch(u_scene, ivec2(gl_FragCoord.xy), 0).r;
        bool hidden = gl_FragCoord.z > scene;
        fragColor = hidden ? vec4(0.5, 0.05, 0.05, 0.5) : vec4(0.1, 0.8, 0.3, 1.0);
      }`);
    const u = uniformsOf(gl, p, ['u_projection_matrix', 'u_projection_tile_mercator_coords', 'u_projection_clipping_plane', 'u_projection_transition', 'u_projection_fallback_matrix', 'u_scene', 'u_size']);
    return (this.programs[shaderData.variantName] = {p, u});
  }

  render(gl, args) {
    this.renders++;
    if (!args.sceneDepth) return;
    const {p, u} = this.columnProgram(gl, args.shaderData);
    gl.useProgram(p);
    const pd = args.getProjectionData({tileID: {wrap: 0, canonical: {z: 14, x: this.at.x, y: this.at.y}}, applyGlobeMatrix: true});
    gl.uniformMatrix4fv(u.u_projection_matrix, false, pd.mainMatrix);
    gl.uniform4fv(u.u_projection_tile_mercator_coords, pd.tileMercatorCoords);
    gl.uniform4fv(u.u_projection_clipping_plane, pd.clippingPlane);
    gl.uniform1f(u.u_projection_transition, pd.projectionTransition);
    gl.uniformMatrix4fv(u.u_projection_fallback_matrix, false, pd.fallbackMatrix);
    gl.activeTexture(gl.TEXTURE5);
    gl.bindTexture(gl.TEXTURE_2D, args.sceneDepth.texture);
    gl.uniform1i(u.u_scene, 5);
    gl.uniform2f(u.u_size, args.sceneDepth.width, args.sceneDepth.height);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.column);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 0, 0);
    gl.disable(gl.CULL_FACE);
    gl.drawArrays(gl.TRIANGLES, 0, 24);
    gl.bindTexture(gl.TEXTURE_2D, null);
  }
}

// Lake Annecy, roughly (447 m), and lines over it.
const LAKE = [[6.14, 45.905], [6.17, 45.89], [6.19, 45.86], [6.215, 45.84], [6.235, 45.81], [6.24, 45.79], [6.225, 45.795],
  [6.205, 45.82], [6.18, 45.85], [6.16, 45.87], [6.135, 45.885], [6.13, 45.895], [6.14, 45.905]];
const line = (coords, props) => ({type: 'Feature', properties: props, geometry: {type: 'LineString', coordinates: coords}});

export const cases = {
  custom: {
    view: '11.6/45.80/6.80/60/72',
    note: 'custom layer: stripes and tile edges draped on the terrain (drape); a column on Mont Blanc drawn after the terrain, green in front of it, red behind (render, reading the frame\'s depth)',
    add(map) {
      const layer = new DrapeAndColumn(6.8652, 45.8326);
      map.addLayer(layer);
      window.__layer = layer;
    },
  },
  lines: {
    view: '12.3/45.83/6.13/75/70',
    note: 'line-z-offset: white draped (as upstream); cyan the "ferry", line-elevation-reference sea at the feature\'s 600 m (floating over the lake, cut by the hills in front); orange a "bridge", ground +300 m, data-driven',
    add(map) {
      map.addSource('lines', {type: 'geojson', data: {type: 'FeatureCollection', features: [
        line([[6.15, 45.895], [6.21, 45.835], [6.232, 45.80]], {kind: 'ferry', z: 600}),
        line([[6.10, 45.86], [6.16, 45.86], [6.26, 45.86]], {kind: 'bridge', z: 300}),
        line([[6.10, 45.84], [6.26, 45.84]], {kind: 'draped'}),
      ]}});
      map.addLayer({id: 'draped', type: 'line', source: 'lines', filter: ['==', ['get', 'kind'], 'draped'], paint: {'line-color': '#fff', 'line-width': 3}});
      map.addLayer({id: 'ferry', type: 'line', source: 'lines', filter: ['==', ['get', 'kind'], 'ferry'],
        paint: {'line-color': '#4fd8ff', 'line-width': 4, 'line-elevation-reference': 'sea', 'line-z-offset': ['get', 'z']}});
      map.addLayer({id: 'bridge', type: 'line', source: 'lines', filter: ['==', ['get', 'kind'], 'bridge'],
        paint: {'line-color': '#ff9a3c', 'line-width': 4, 'line-dasharray': [2, 1], 'line-z-offset': ['get', 'z']}});
    },
  },
  rasterwater: {
    view: '11.6/45.855/6.19/20/62',
    note: 'raster-elevation-reference sea: the app\'s water raster (the backend\'s /tiles/water) as a level surface at 600 m (raster-z-offset), over the terrain, the hills cutting it',
    add(map) {
      map.addSource('water', {type: 'raster', tiles: [`${location.origin}/tiles/water/{z}/{x}/{y}?c=1b5fa8,2a7fd4`], tileSize: 256 / Math.SQRT2, maxzoom: 18});
      map.addLayer({id: 'water', type: 'raster', source: 'water',
        paint: {'raster-opacity': 0.85, 'raster-fade-duration': 0, 'raster-elevation-reference': 'sea', 'raster-z-offset': 600}});
    },
  },
  water: {
    view: '11.8/45.80/6.10/60/72',
    note: 'fill-elevation-reference sea: the lake as a see-through surface at 600 m (fill-z-offset, data-driven), over the terrain, the hills cutting it; outlined; its name on the surface (symbol-height-anchor sea)',
    add(map) {
      map.addSource('lake', {type: 'geojson', data: {type: 'Feature', properties: {level: 600}, geometry: {type: 'Polygon', coordinates: [LAKE]}}});
      map.addLayer({id: 'lake', type: 'fill', source: 'lake',
        paint: {'fill-color': '#2a7fd4', 'fill-opacity': 0.55, 'fill-outline-color': '#bfe3ff', 'fill-elevation-reference': 'sea', 'fill-z-offset': ['get', 'level']}});
      // The lake's name on its surface: symbol-height-anchor sea, the same 600 m (exaggerated as the terrain is).
      map.addSource('lake-name', {type: 'geojson', data: {type: 'Feature', properties: {name: 'Lac d\'Annecy (600 m, sea)', level: 600}, geometry: {type: 'Point', coordinates: [6.19, 45.855]}}});
      map.addLayer({id: 'lake-name', type: 'symbol', source: 'lake-name',
        layout: {'text-field': ['get', 'name'], 'text-font': ['Noto Sans Regular'], 'text-size': 15, 'symbol-height-anchor': 'sea', 'symbol-height-offset': ['get', 'level']},
        paint: {'text-color': '#fff', 'text-halo-color': '#0b2a4a', 'text-halo-width': 1.5}});
    },
  },
};
