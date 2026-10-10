/** The shaders' sources as strings, made from the .glsl files as they're imported (build/shaders_plugin.ts). */
declare module '*.glsl.g.ts' {
    const source: string;
    export default source;
}
