import fs from 'fs';
import path from 'path';

/**
 * The shaders, made from the .glsl files as they're imported (this fork's change: upstream wrote
 * them out as `src/shaders/glsl/*.glsl.g.ts` with `npm run generate-shaders`, which went stale
 * whenever a shader was edited without it). `src/shaders/shaders.ts` imports `./glsl/<name>.glsl.g.ts`;
 * this plugin resolves each such import to its .glsl file's text, minified a little, as the module's
 * default export. It is a Rollup-style plugin, used alike by the app's Vite build and dev server
 * (web/vite.config.ts), this package's Rolldown build (rolldown.config.ts) and its Vitest configs.
 * `src/shaders/glsl_modules.d.ts` gives the imports their type.
 */
export function glslToJs(code: string): string {
    code = code
        .trim() // strip whitespace at the start/end
        .replace(/\s*\/\/[^\n]*\n/g, '\n') // strip double-slash comments
        .replace(/\n+/g, '\n') // collapse multi line breaks
        .replace(/\n\s+/g, '\n') // strip indentation
        .replace(/\s?([+-\/*=,])\s?/g, '$1') // strip whitespace around operators
        .replace(/^([^#\n][^\n]*[;\(\),\{\}])\n(?=[^#])/gm, '$1'); // strip more line breaks (Scenic Roads: never after a directive)
    return `export default ${JSON.stringify(code)};\n`;
}

const SUFFIX = '.glsl.g.ts';

type PluginContext = {addWatchFile?: (file: string) => void};

export function shadersPlugin() {
    return {
        name: 'maplibre-shaders',
        enforce: 'pre' as const,
        resolveId(id: string, importer?: string): string | null {
            if (!id.endsWith(SUFFIX) || !importer) return null;
            return path.resolve(path.dirname(importer.split('?')[0]), id);
        },
        load(this: PluginContext, id: string): string | null {
            const file = id.split('?')[0];
            if (!file.endsWith(SUFFIX)) return null;
            const glsl = file.slice(0, -'.g.ts'.length);
            this.addWatchFile?.(glsl);
            return glslToJs(fs.readFileSync(glsl, 'utf8'));
        },
    };
}
