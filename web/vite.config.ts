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

// In development the Rust backend serves data; Vite serves the app with HMR.
const backend = 'http://127.0.0.1:8080';

export default defineConfig({
  plugins: [maplibreTerrainVisibility(), maplibreSlopeColours()],
  server: {
    port: 5173,
    proxy: { '/api': backend, '/tiles': backend, '/fonts': backend },
  },
  // MapLibre v6 loads its worker relative to its own module; pre-bundling breaks that.
  optimizeDeps: { exclude: ['maplibre-gl'] },
  worker: { format: 'es' },
  build: { target: 'es2022', sourcemap: true, chunkSizeWarningLimit: 2000 },
});
