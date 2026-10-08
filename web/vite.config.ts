import { defineConfig, type Plugin } from 'vite';

/**
 * MapLibre fades circles and symbols behind 3D terrain by comparing their depth with its terrain
 * depth texture. On the globe the texture holds perspective depth (projectTileFor3D) while the
 * points' depth comes from projectTileWithElevation, which there returns MapLibre's clipping-plane
 * depth instead, so the comparison is meaningless: in tilted, zoomed-in views every point and
 * label was "behind the terrain" and vanished. Compute the points' visibility depth the way the
 * texture was drawn. (On the flat map the two functions are the same.)
 */
function maplibreTerrainVisibility(): Plugin {
  const fixes: [string, string][] = [
    ['calculate_visibility(projectTileWithElevation(circle_center,ele))', 'calculate_visibility(projectTileFor3D(circle_center,ele))'],
    ['calculate_visibility(projectedPoint)', 'calculate_visibility(projectTileFor3D(translated_a_pos,ele))'],
  ];
  return {
    name: 'maplibre-terrain-visibility',
    enforce: 'pre',
    transform(code, id) {
      if (!/maplibre-gl\/dist\/maplibre-gl(-dev)?\.mjs/.test(id)) return null;
      let out = code;
      for (const [from, to] of fixes) {
        if (!out.includes(from)) this.warn(`maplibre-terrain-visibility: "${from}" not found (MapLibre changed?)`);
        out = out.split(from).join(to);
      }
      return { code: out, map: null };
    },
  };
}

/**
 * The slope tint's colours averaged as the eye would (roadcore::slope). Its tiles hold four slopes
 * a pixel, the quarters of the ground beneath it; MapLibre's colour-relief shader colours one value
 * a pixel, so zoomed out it showed the colour of the mean slope: the mean of a white cliff band and
 * the yellow slopes beside it was a slightly brighter yellow, where on screen the band reads white
 * (white z8 streaks mostly gone by z5.5). For the slope source (known by its encoding's base shift,
 * basemap.ts SLOPE4_SHIFT) each of the four is coloured through the ramp and the colours averaged
 * in linear light, the way a screen's light adds up; every other colour-relief layer as before.
 */
function maplibreSlopeColours(): Plugin {
  const from = 'void main() {float el=getElevation(v_pos);int r=(u_color_ramp_size-1);int l=0;float el_l=getElevationStop(l);float el_r=getElevationStop(r);while(r-l > 1){int m=(r+l)/2;float el_m=getElevationStop(m);if(el < el_m){r=m;el_r=el_m;}else\n{l=m;el_l=el_m;}}float x=(float(l)+(el-el_l)/(el_r-el_l)+0.5)/float(u_color_ramp_size);fragColor=u_opacity*texture(u_color_stops,vec2(x,0));';
  const to = [
    'vec4 rampAt(float el){int r=(u_color_ramp_size-1);int l=0;float el_l=getElevationStop(l);float el_r=getElevationStop(r);while(r-l > 1){int m=(r+l)/2;float el_m=getElevationStop(m);',
    'if(el < el_m){r=m;el_r=el_m;}else{l=m;el_l=el_m;}}float x=(float(l)+(el-el_l)/(el_r-el_l)+0.5)/float(u_color_ramp_size);return texture(u_color_stops,vec2(x,0));}',
    'vec3 linOf(vec4 c){return c.a > 0.0 ? pow(c.rgb/c.a,vec3(2.2))*c.a : vec3(0.0);}',
    'void main() {vec4 col;if(abs(u_unpack.w-32768.5) < 0.01){vec4 q=texture(u_image,v_pos);vec4 s=q*q*400.0;',
    'vec4 c0=rampAt(s.x);vec4 c1=rampAt(s.y);vec4 c2=rampAt(s.z);vec4 c3=rampAt(s.w);float a=0.25*(c0.a+c1.a+c2.a+c3.a);',
    'vec3 lin=0.25*(linOf(c0)+linOf(c1)+linOf(c2)+linOf(c3));col=a > 0.0 ? vec4(pow(lin/a,vec3(1.0/2.2))*a,a) : vec4(0.0);}',
    'else{col=rampAt(getElevation(v_pos));}fragColor=u_opacity*col;',
  ].join('');
  return {
    name: 'maplibre-slope-colours',
    enforce: 'pre',
    transform(code, id) {
      if (!/maplibre-gl\/dist\/maplibre-gl(-dev)?\.mjs/.test(id)) return null;
      if (!code.includes(from)) {
        this.warn('maplibre-slope-colours: the colour-relief shader not found (MapLibre changed?)');
        return null;
      }
      return { code: code.split(from).join(to), map: null };
    },
  };
}

/** Applies `fixes` (from, to, how many times it must occur) to MapLibre's bundle, warning when one
 * isn't found exactly so (MapLibre changed: package.json pins it). */
function maplibrePatch(name: string, fixes: [string, string, number][]): Plugin {
  return {
    name,
    enforce: 'pre',
    transform(code, id) {
      if (!/maplibre-gl\/dist\/maplibre-gl(-dev)?\.mjs/.test(id)) return null;
      let out = code;
      for (const [from, to, n] of fixes) {
        const found = out.split(from).length - 1;
        if (found !== n) {
          this.warn(`${name}: "${from.slice(0, 80)}" found ${found} times, not ${n} (MapLibre changed?): left as it was`);
          continue;
        }
        out = out.split(from).join(to);
      }
      return { code: out, map: null };
    },
  };
}

/**
 * The globe's 3D positions without float32's noise (docs/buildings3d.md §4.3). On the globe (below
 * zoom 16.5 here) MapLibre places the terrain's mesh and the extrusions on the unit sphere and
 * multiplies by the view matrix in float32, so each vertex lands up to a metre or two off, a
 * different way for every vertex and for every camera: the terrain's surface is bumpy at its
 * mesh's spacing, and where a wall meets it the edge was a saw of spikes that changed as the camera
 * moved (task #115; the flat map, from 16.5, never had it: its tiles' matrices take small numbers).
 * Here the globe's position is the tile's flat (fallback) projection, which is exact in small
 * numbers, plus the difference between the two, interpolated across the z14 cell the vertex is in
 * from its four corners: a corner's mercator coordinates are exact, so every vertex of every layer
 * near it gets the same difference, and its float32 error moves terrain and buildings together, a
 * smooth fraction of a metre. (Bilinear across a z14 cell, the sphere's own curve is off by ~0.1 m
 * at most.) The elevation's share is added from the vertex's own sphere point, in small numbers.
 * Placing each vertex from one anchor in the cell instead didn't help (the shader compiler is free
 * to fold its two matrix products back into one). Used by everything 3D that MapLibre draws through
 * `interpolateProjectionFor3D`: the terrain and its depth, the extrusions, the circles' and
 * symbols' visibility (as patched above).
 */
const GLOBE_PRECISE = [
  // The lattice: z14 cells.
  'const float LATTICE=16384.0;',
  // A lattice point's globe position (its mercator coordinates exact).
  'vec4 globeAtLattice(vec2 merc) {float sx=merc.x*PI*2.0+PI;float t=exp(PI-(merc.y*PI*2.0));float t2=t*t;float den=t2+1.0;',
  'return u_projection_matrix*vec4(sin(sx)*(2.0*t)/den,(t2-1.0)/den,cos(sx)*(2.0*t)/den,1.0);}',
  'vec4 interpolateProjectionFor3D(vec2 posInTile,vec3 spherePos,float elevation) {v_projection_tile_x=posInTile.x;',
  'vec4 globePosition;',
  'if ((posInTile.y <-32767.5) || (posInTile.y > 32766.5)) {globePosition=u_projection_matrix*vec4(spherePos*(1.0+elevation/GLOBE_RADIUS),1.0);} else {',
  'vec2 o=u_projection_tile_mercator_coords.xy;vec2 k=u_projection_tile_mercator_coords.zw;',
  'vec2 g=floor((o+k*posInTile)*LATTICE);',
  'vec2 f=((o-g/LATTICE)+k*posInTile)*LATTICE;',
  'vec2 c0=(g/LATTICE-o)/k;vec2 c1=((g+1.0)/LATTICE-o)/k;',
  'mat4 F=u_projection_fallback_matrix;',
  'vec4 a00=globeAtLattice(g/LATTICE)-F*vec4(c0.x,c0.y,0.0,1.0);',
  'vec4 a10=globeAtLattice(vec2(g.x+1.0,g.y)/LATTICE)-F*vec4(c1.x,c0.y,0.0,1.0);',
  'vec4 a01=globeAtLattice(vec2(g.x,g.y+1.0)/LATTICE)-F*vec4(c0.x,c1.y,0.0,1.0);',
  'vec4 a11=globeAtLattice((g+1.0)/LATTICE)-F*vec4(c1.x,c1.y,0.0,1.0);',
  'vec4 a=mix(mix(a00,a10,f.x),mix(a01,a11,f.x),f.y);',
  'globePosition=F*vec4(posInTile,0.0,1.0)+a+(elevation/GLOBE_RADIUS)*(u_projection_matrix*vec4(spherePos,0.0));}',
].join('');

const maplibreGlobePrecision = () =>
  maplibrePatch('maplibre-globe-precision', [
    [
      'vec4 interpolateProjectionFor3D(vec2 posInTile,vec3 spherePos,float elevation) {v_projection_tile_x=posInTile.x;vec3 elevatedPos=spherePos*(1.0+elevation/GLOBE_RADIUS);vec4 globePosition=u_projection_matrix*vec4(elevatedPos,1.0);',
      GLOBE_PRECISE,
      1,
    ],
  ]);

/**
 * Each wall on the terrain under its own corner (docs/buildings3d.md §4.3): MapLibre stands a
 * building on the terrain at its centroid, its base (when 0) sunk 10 m, so on a slope its uphill
 * walls are buried to their foot and its downhill ones float until the 10 m runs out. Here a base
 * of 0 is the ground under each corner (the same DEM sampling the terrain's mesh takes) less 2 m,
 * never above the roof; the roof stays level, at the centroid's ground plus the height. A part
 * with a base of its own (a tower's setback) keeps it above its centroid's ground, as before.
 */
const maplibreBuildingFeet = () =>
  maplibrePatch('maplibre-building-feet', [
    [
      'float base_terrain3d_offset=height_terrain3d_offset-(base > 0.0 ? 0.0 : 10.0);',
      'float base_terrain3d_offset=base > 0.0 ? height_terrain3d_offset : min(get_elevation(a_pos)-2.0,height_terrain3d_offset+max(0.0,height));',
      2,
    ],
  ]);

/**
 * Fog on the extrusions (docs/buildings3d.md §1, Fog): MapLibre fogs the terrain toward the horizon
 * (the sky's fog-ground-blend, from 60° of pitch, on the flat map: on the globe it fogs nothing),
 * and its fill-extrusion shader drew the colour alone, so a far skyline stood sharp against a
 * fogged ground. Here the extrusions take the terrain's fog: its uniforms bound and set as the
 * terrain's are (the same fog matrix of the tile, colours, blends and opacity), its fog depth from
 * the vertex, its blend in the fragment shader, in linear light, on the colour before its opacity.
 */
const EXTRUSION_FOG_FS = [
  'uniform vec4 u_fog_color;uniform vec4 u_horizon_color;uniform float u_fog_ground_blend;uniform float u_fog_ground_blend_opacity;',
  'uniform float u_horizon_fog_blend;uniform float u_is_globe_mode;in float v_fog_depth;',
  'in vec4 v_color;void main() {fragColor=v_color;',
  'if (u_is_globe_mode < 0.5 && u_fog_ground_blend_opacity > 0.0 && v_fog_depth > u_fog_ground_blend && v_color.a > 0.0) {',
  'vec3 c=pow(v_color.rgb/v_color.a,vec3(2.2));',
  'float blend_color=smoothstep(0.0,1.0,max((v_fog_depth-u_horizon_fog_blend)/(1.0-u_horizon_fog_blend),0.0));',
  'vec3 fog=mix(pow(u_fog_color.rgb,vec3(2.2)),pow(u_horizon_color.rgb,vec3(2.2)),blend_color);',
  'float f=max(v_fog_depth-u_fog_ground_blend,0.0)/(1.0-u_fog_ground_blend);',
  'fragColor=vec4(pow(mix(c,fog,pow(f,2.0)*u_fog_ground_blend_opacity),vec3(1.0/2.2))*v_color.a,v_color.a);}',
].join('');

const maplibreExtrusionFog = () =>
  maplibrePatch('maplibre-extrusion-fog', [
    ['fillExtrusion:Y(`in vec4 v_color;void main() {fragColor=v_color;', 'fillExtrusion:Y(`' + EXTRUSION_FOG_FS, 1],
    ['out vec4 v_color;\n#pragma maplibre: define highp float base', 'out vec4 v_color;uniform mat4 u_fog_matrix;out float v_fog_depth;\n#pragma maplibre: define highp float base', 1],
    [
      '#else\ngl_Position=u_projection_matrix*vec4(posInTile,elevation,1.0);\n#endif\nfloat colorvalue',
      '#else\ngl_Position=u_projection_matrix*vec4(posInTile,elevation,1.0);\n#endif\nvec4 fog_pos=u_fog_matrix*vec4(posInTile,elevation,1.0);v_fog_depth=fog_pos.z/fog_pos.w*0.5+0.5;\nfloat colorvalue',
      1,
    ],
    // Its uniforms bound (as the terrain's: kr a mat4, le a colour, V a float) and set.
    [
      'u_opacity:new V(e,t.u_opacity),u_fill_translate:new L(e,t.u_fill_translate)}),au=(e,t)=>',
      'u_opacity:new V(e,t.u_opacity),u_fill_translate:new L(e,t.u_fill_translate),u_fog_matrix:new kr(e,t.u_fog_matrix),u_fog_color:new le(e,t.u_fog_color),u_fog_ground_blend:new V(e,t.u_fog_ground_blend),u_fog_ground_blend_opacity:new V(e,t.u_fog_ground_blend_opacity),u_horizon_color:new le(e,t.u_horizon_color),u_horizon_fog_blend:new V(e,t.u_horizon_fog_blend),u_is_globe_mode:new V(e,t.u_is_globe_mode)}),au=(e,t)=>',
      1,
    ],
    [
      'w=f?su(e,C,m,S,d,p,r):ou(e,C,m,S);',
      'w=f?su(e,C,m,S,d,p,r):ou(e,C,m,S);if(!f){let G=!!s.isRenderingGlobe,K=e.style.sky;Object.assign(w,{u_fog_matrix:G?new Float32Array(16):g.calculateFogMatrix(d.toUnwrapped()),u_fog_color:K?K.properties.get(`fog-color`):z.white,u_fog_ground_blend:K?K.properties.get(`fog-ground-blend`):1,u_fog_ground_blend_opacity:G||!K?0:K.calculateFogBlendOpacity(g.pitch),u_horizon_color:K?K.properties.get(`horizon-color`):z.white,u_horizon_fog_blend:K?K.properties.get(`horizon-fog-blend`):1,u_is_globe_mode:+G})}',
      1,
    ],
  ]);

// In development the Rust backend serves data; Vite serves the app with HMR.
const backend = 'http://127.0.0.1:8080';

export default defineConfig({
  plugins: [maplibreTerrainVisibility(), maplibreSlopeColours(), maplibreGlobePrecision(), maplibreBuildingFeet(), maplibreExtrusionFog()],
  server: {
    port: 5173,
    proxy: { '/api': backend, '/tiles': backend, '/fonts': backend },
  },
  // MapLibre v6 loads its worker relative to its own module; pre-bundling breaks that.
  optimizeDeps: { exclude: ['maplibre-gl'] },
  worker: { format: 'es' },
  build: { target: 'es2022', sourcemap: true, chunkSizeWarningLimit: 2000 },
});
